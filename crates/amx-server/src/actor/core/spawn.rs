//! Spawning a pane's backing process, and choosing the directory it starts in.
//!
//! Split out of [`super::pane`] by V02 (`docs/08-m2-plan.md` R-M2-5) before M2
//! pressed it: `core/pane.rs` stood at 499 of a 500-line soft budget, and V07
//! adds argv recording, `AMX_*` environment injection and the per-spawn hook
//! token to exactly this code. The move is mechanical — nothing changed but
//! which file the lines are in.
//!
//! One spawn path, not two: a split, a `workspace.create`'s root pane, and the
//! startup restore all reach [`Core::spawn_pane`], which is what makes a
//! promise about how panes start ("its environment carries `AMX_PANE_ID`",
//! V07) true of every pane rather than of the ones somebody remembered.
//!
//! # What the child is told, and why the token is here
//!
//! D-M2-4: "every pane spawn injects `AMX_ENV=1`, `AMX_SESSION`, `AMX_SOCKET`,
//! `AMX_PANE_ID`, `AMX_WORKSPACE_ID`, and `AMX_HOOK_TOKEN`. Hook processes
//! inherit the agent's environment, the agent inherits the pane's shell's, so a
//! `claude` typed by hand is attributed exactly like one `agent start`
//! launched." V01 §3 M6 measured that chain end to end — all 185 hook
//! invocations in the spike's run carried all six variables verbatim, through
//! an interactive shell — so the scheme stands as designed, and the degraded
//! `session_id`-plus-process-tree branch the plan held in reserve is not needed.
//!
//! The variables travel on the [`PtyCommand`], never through this process's own
//! environment: `std::env::set_var` is a global mutation shared with every
//! other pane and every other thread, and two panes spawning at once would race
//! for one `AMX_PANE_ID`.
//!
//! The token is minted here, once per spawn, and recorded on the pane beside
//! the argv. Per-spawn and not per-pane: a pane id that came back from a
//! snapshot gets a fresh token, so a hook config left behind by the *previous*
//! process for that pane reports into the void instead of into the new one's
//! status. That is the whole of the guard — it is not a security boundary, the
//! socket is 0600 and any same-user process can already drive every pane.
//!
//! # Task ownership
//!
//! **V07** filled the `AMX_*` injection, the argv recording and the token mint.
//! The handoff to `AgentHub` — building a `SpawnedIdentity` out of the pane's
//! recorded argv and token and `try_send`ing it as `PaneStarted` — is the
//! `Core`↔hub seam, and belongs to whoever wires the two together (V08/V17).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_core::agent::HookToken;
use amx_core::platform::{Pty, PtyCommand, WinSize, pane_env};
use amx_core::{Ctx, PaneId, WorkspaceId};
use thiserror::Error;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::Core;
use crate::actor::{PaneCommand, PaneHost, PaneHostConfig, PaneHostError};
use crate::config_rt;
use crate::platform::UnixPty;

/// The grid every freshly spawned pane starts at.
///
/// M0 has no negotiated terminal size yet (that is the client's job, T13/T14);
/// this is the traditional terminal default, matching
/// [`amx_core::state::workspace::DEFAULT_AREA`].
const DEFAULT_SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// How long a split waits for the source pane to answer a foreground-cwd
/// read before falling back to the recorded cwd.
///
/// The `Core` must never park on another actor indefinitely — a pane wedged
/// behind a saturated mailbox would wedge the whole session with it — so the
/// wait is bounded and the fallback (04 §7) is the same one an unreadable
/// `/proc` takes.
const FOREGROUND_CWD_TIMEOUT: Duration = Duration::from_millis(250);

/// A pane's backing process could not be started.
#[derive(Debug, Error)]
pub(super) enum SpawnError {
    /// The pty itself could not be opened or the command could not run.
    #[error(transparent)]
    Pty(#[from] amx_core::platform::PlatformError),
    /// libghostty-vt or the pane actor's threads could not be started.
    #[error(transparent)]
    Host(#[from] PaneHostError),
}

impl Core {
    /// The cwd a split with no explicit override should use: the source
    /// pane's live foreground-process cwd if one can be read, falling back to
    /// the source pane's own recorded cwd, and finally to this process's own
    /// cwd if even that was never recorded (04 §7).
    ///
    /// The ask is `try_send` plus a bounded wait, never a blocking send: the
    /// `Core` waiting for capacity on a pane's mailbox while that pane waits
    /// for capacity on the `Core`'s is a deadlock with both mailboxes full,
    /// and a cwd default is not worth wedging the session over.
    pub(super) async fn resolve_split_cwd(&self, source: PaneId) -> PathBuf {
        if let Some(host) = self.panes.get(&source) {
            let (tx, rx) = oneshot::channel();
            if host
                .handle()
                .try_send(PaneCommand::ForegroundCwd(tx))
                .is_ok()
                && let Ok(Ok(Some(cwd))) = tokio::time::timeout(FOREGROUND_CWD_TIMEOUT, rx).await
            {
                return cwd;
            }
        }
        self.state
            .pane(source)
            .and_then(|p| p.cwd())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
    }

    /// Open a pty and start a `PaneHost` for a freshly minted pane.
    ///
    /// `pub(super)` because a live `workspace.create` spawns its root pane's
    /// shell through the same path a split uses — one spawn path, not two.
    ///
    /// The pane spawns at its projected cell size when a client has declared a
    /// viewport (04 §3 — the active client drives sizes), and at the 24x80
    /// default otherwise.
    ///
    /// `&mut self` because a spawn is also a *recording*: the argv the child
    /// really got and the token its environment really carries are written onto
    /// the pane before this returns, so nothing downstream has to remember to
    /// ask. The write happens after the pty is open and before the pane host
    /// starts, which is the only window in which both facts are known and no
    /// report about the pane can have been published yet.
    pub(super) fn spawn_pane(
        &mut self,
        pane: PaneId,
        cwd: PathBuf,
        command: Option<Vec<String>>,
    ) -> Result<PaneHost, SpawnError> {
        let size = self
            .planned_size(pane)
            .map_or(DEFAULT_SIZE, |(rows, cols)| WinSize { rows, cols });
        // Read here, once per spawn: an edit to `[terminal] shell` reaches the
        // next pane and no other, because a pane's process is never restarted
        // for a configuration change (D-M1-8).
        let shell = config_rt::shell(&self.config.borrow());
        let argv = effective_argv(shell, command);
        let token = mint_token();
        let env = pane_environment(&self.ctx, pane, self.workspace_of(pane), &token);
        let session = UnixPty.spawn(&pty_command(&argv, cwd, size, env))?;
        // Recorded from `argv` rather than from the caller's `command`, so the
        // pane remembers the shell a split with no command actually ran and not
        // the `None` it was asked for. Lossy only for a shell path that is not
        // UTF-8, which is the same lossiness `session.state` already has.
        let _ = self.state.set_pane_spawn(
            pane,
            argv.iter()
                .map(|word| word.to_string_lossy().into_owned())
                .collect(),
            token,
        );
        let mut config = PaneHostConfig::new(pane, self.ctx.bus.clone(), size);
        config.core = Some(self.handle.clone());
        config.cancel = self.ctx.cancel.child_token();
        Ok(PaneHost::spawn(config, session)?)
    }
}

/// The argv a pane really runs: what the caller asked for, or the configured
/// shell when it asked for nothing.
///
/// An empty `command` is the same as no command — a `pane.split` that sent an
/// empty vector meant "a shell", and spawning a program with no name is not an
/// interpretation anybody wanted.
fn effective_argv(shell: OsString, command: Option<Vec<String>>) -> Vec<OsString> {
    let argv: Vec<OsString> = command
        .unwrap_or_default()
        .into_iter()
        .map(OsString::from)
        .collect();
    if argv.is_empty() { vec![shell] } else { argv }
}

/// A fresh hook token.
///
/// A v4 UUID's 122 random bits, from the same generator every id in the tree is
/// minted with, written without hyphens because it travels through an
/// environment variable and back over the wire. The value is opaque: nothing
/// reads it, two of them are only ever compared.
fn mint_token() -> HookToken {
    HookToken::new(Uuid::new_v4().simple().to_string())
}

/// The `AMX_*` environment every pane child carries (D-M2-4).
///
/// `workspace` is an `Option` because a pane must be in the layout before its
/// workspace can be named. When it is `None` the variable is left out rather
/// than set to something empty: a reader can then tell "amx did not know" from
/// "amx said nothing", and every other variable — the ones identity actually
/// rides on — is still there.
fn pane_environment(
    ctx: &Ctx,
    pane: PaneId,
    workspace: Option<WorkspaceId>,
    token: &HookToken,
) -> Vec<(OsString, OsString)> {
    let mut env = vec![
        (
            OsString::from(pane_env::MARKER),
            OsString::from(pane_env::MARKER_VALUE),
        ),
        (
            OsString::from(pane_env::SESSION),
            OsString::from(ctx.session.as_str()),
        ),
        (
            OsString::from(pane_env::SOCKET),
            ctx.socket.clone().into_os_string(),
        ),
        (
            OsString::from(pane_env::PANE),
            OsString::from(pane.to_string()),
        ),
        (
            OsString::from(pane_env::TOKEN),
            OsString::from(token.as_str()),
        ),
    ];
    if let Some(workspace) = workspace {
        env.push((
            OsString::from(pane_env::WORKSPACE),
            OsString::from(workspace.to_string()),
        ));
    }
    env
}

/// Build the command a freshly spawned pane runs.
fn pty_command(
    argv: &[OsString],
    cwd: PathBuf,
    size: WinSize,
    env: Vec<(OsString, OsString)>,
) -> PtyCommand {
    let mut argv = argv.iter().cloned();
    // `effective_argv` never yields an empty vector, so the fallback is
    // unreachable; spelling it costs one line and avoids an `expect`.
    let program = argv.next().unwrap_or_default();
    PtyCommand {
        program,
        args: argv.collect(),
        env,
        cwd: Some(cwd),
        size,
    }
}
