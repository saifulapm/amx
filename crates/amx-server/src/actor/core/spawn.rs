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
//! # Task ownership
//!
//! **V07** extends [`pty_command`] with the injected environment of D-M2-4 and
//! records the argv into pane state; the `env: Vec::new()` below is the empty
//! seam it fills.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_core::PaneId;
use amx_core::platform::{Pty, PtyCommand, WinSize};
use thiserror::Error;
use tokio::sync::oneshot;

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
    pub(super) fn spawn_pane(
        &self,
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
        let session = UnixPty.spawn(&pty_command(shell, cwd, command, size))?;
        let mut config = PaneHostConfig::new(pane, self.ctx.bus.clone(), size);
        config.core = Some(self.handle.clone());
        config.cancel = self.ctx.cancel.child_token();
        Ok(PaneHost::spawn(config, session)?)
    }
}

/// Build the command a freshly spawned pane runs.
fn pty_command(
    shell: OsString,
    cwd: PathBuf,
    command: Option<Vec<String>>,
    size: WinSize,
) -> PtyCommand {
    let mut argv = command.into_iter().flatten();
    let (program, args) = match argv.next() {
        Some(first) => (OsString::from(first), argv.map(OsString::from).collect()),
        None => (shell, Vec::new()),
    };
    PtyCommand {
        program,
        args,
        env: Vec::new(),
        cwd: Some(cwd),
        size,
    }
}
