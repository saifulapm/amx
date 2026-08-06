//! `pane.*` handlers: split, zoom, swap, move, close.
//!
//! Split carries the value T16 exists for: [`Core::handle_split_live`]
//! resolves the source pane's *foreground process* cwd (04 §7) before
//! spawning the new pane's backing process, and [`Core::handle_split_no_spawn`]
//! is the synchronous fallback [`super::Core::absorb`] uses when it cannot
//! await that resolution (see the module doc on `super`). Swap and move never
//! touch a pane's actor at all, which is what makes "never restarts the
//! process" true by construction rather than by care.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use amx_core::platform::{Pty, PtyCommand, WinSize};
use amx_core::{Direction, Event, PaneId};
use amx_proto::control::pane;
use amx_proto::rpc::RpcError;
use thiserror::Error;
use tokio::sync::oneshot;

use super::Core;
use crate::actor::{PaneCommand, PaneHandle, PaneHost, PaneHostConfig, PaneHostError, Reply};
use crate::platform::UnixPty;

/// The grid every freshly spawned pane starts at.
///
/// M0 has no negotiated terminal size yet (that is the client's job, T13/T14);
/// this is the traditional terminal default, matching
/// [`amx_core::state::workspace::DEFAULT_AREA`].
const DEFAULT_SIZE: WinSize = WinSize { rows: 24, cols: 80 };

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
    /// The synchronous fallback [`super::Core::absorb`] uses for a split:
    /// mints the pane and folds its effect, but starts nothing. See the
    /// module doc on `super` and [`Core::handle_split_live`], which is what
    /// [`super::Core::run`] actually calls.
    pub(super) fn handle_split_no_spawn(
        &mut self,
        params: pane::SplitParams,
        reply: Reply<pane::SplitReply>,
    ) {
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        match self
            .state
            .split(ws, params.pane, split_direction(params.direction), 0.5)
        {
            Ok((new_pane, effect)) => {
                self.effects.absorb(effect);
                let seq = self.publish(Event::PaneCreated {
                    pane: new_pane,
                    workspace: ws,
                });
                let short = self.next_pane_short();
                let _ = reply.send(Ok(pane::SplitReply {
                    pane: new_pane,
                    short,
                    seq,
                }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    /// The real split handler: [`super::Core::run`] awaits this directly
    /// rather than folding it through [`super::Core::absorb`], because
    /// resolving the cwd to inherit is an `await` (04 §7 — a split inherits
    /// the source pane's *foreground process* cwd, read by asking that
    /// pane's own actor).
    pub(super) async fn handle_split_live(
        &mut self,
        params: pane::SplitParams,
        reply: Reply<pane::SplitReply>,
    ) {
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        let cwd = match &params.cwd {
            Some(cwd) => cwd.clone(),
            None => self.resolve_split_cwd(params.pane).await,
        };
        let (new_pane, effect) =
            match self
                .state
                .split(ws, params.pane, split_direction(params.direction), 0.5)
            {
                Ok(ok) => ok,
                Err(err) => {
                    let _ = reply.send(Err(RpcError::new(
                        RpcError::INVALID_PARAMS,
                        err.to_string(),
                    )));
                    return;
                }
            };
        match self.spawn_pane(new_pane, cwd.clone(), params.command.clone()) {
            Ok(pane_handle) => {
                self.panes.insert(new_pane, pane_handle);
                // The pane was just minted: recording its cwd cannot fail.
                let _ = self.state.set_pane_cwd(new_pane, cwd);
                self.effects.absorb(effect);
                let seq = self.publish(Event::PaneCreated {
                    pane: new_pane,
                    workspace: ws,
                });
                let short = self.next_pane_short();
                let _ = reply.send(Ok(pane::SplitReply {
                    pane: new_pane,
                    short,
                    seq,
                }));
            }
            Err(err) => {
                // The state mutation succeeded but the process it was for
                // could not start: undo it rather than leave a pane in the
                // layout with nothing behind it.
                let _ = self.state.close(ws, new_pane);
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INTERNAL_ERROR,
                    err.to_string(),
                )));
            }
        }
    }

    /// The cwd a split with no explicit override should use: the source
    /// pane's live foreground-process cwd if one can be read, falling back to
    /// the source pane's own recorded cwd, and finally to this process's own
    /// cwd if even that was never recorded (04 §7).
    async fn resolve_split_cwd(&self, source: PaneId) -> PathBuf {
        if let Some(handle) = self.panes.get(&source) {
            let (tx, rx) = oneshot::channel();
            if handle.send(PaneCommand::ForegroundCwd(tx)).await.is_ok()
                && let Ok(Some(cwd)) = rx.await
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
    pub(super) fn spawn_pane(
        &self,
        pane: PaneId,
        cwd: PathBuf,
        command: Option<Vec<String>>,
    ) -> Result<PaneHandle, SpawnError> {
        let session = UnixPty.spawn(&pty_command(cwd, command))?;
        let mut config = PaneHostConfig::new(pane, self.ctx.bus.clone(), DEFAULT_SIZE);
        config.core = Some(self.handle.clone());
        config.cancel = self.ctx.cancel.child_token();
        let host = PaneHost::spawn(config, session)?;
        Ok(host.handle().clone())
    }

    pub(super) fn handle_pane_zoom(
        &mut self,
        params: pane::ZoomParams,
        reply: Reply<pane::ZoomReply>,
    ) {
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        let already_zoomed = self
            .state
            .workspace(ws)
            .is_some_and(|w| w.layout().zoomed() == Some(params.pane));
        let outcome = if already_zoomed {
            self.state.unzoom(ws)
        } else {
            self.state.zoom(ws, params.pane)
        };
        match outcome {
            Ok(effect) => {
                self.effects.absorb(effect);
                let seq = self.publish(Event::LayoutChanged { workspace: ws });
                let _ = reply.send(Ok(pane::ZoomReply {
                    zoomed: !already_zoomed,
                    seq,
                }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_pane_swap(
        &mut self,
        params: pane::SwapParams,
        reply: Reply<pane::SwapReply>,
    ) {
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        if self.workspace_of(params.with) != Some(ws) {
            let _ = reply.send(Err(Self::no_such_pane(params.with)));
            return;
        }
        match self.state.swap(ws, params.pane, params.with) {
            Ok(effect) => {
                self.effects.absorb(effect);
                let seq = self.publish(Event::LayoutChanged { workspace: ws });
                let _ = reply.send(Ok(pane::SwapReply { seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_pane_move(
        &mut self,
        params: pane::MoveParams,
        reply: Reply<pane::MoveReply>,
    ) {
        let Some(from) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        if self.state.workspace(params.to).is_none() {
            let _ = reply.send(Err(Self::no_such_workspace(params.to)));
            return;
        }
        // The picker chooses the destination workspace only (04 §7); the
        // target this pane splits against inside it is whatever that
        // workspace currently has focused, which `SessionState::move_pane`
        // requires only when the destination is non-empty.
        let target = self.state.workspace(params.to).and_then(|w| w.focus());
        match self
            .state
            .move_pane(params.pane, from, params.to, target, Direction::Right)
        {
            Ok(effect) => {
                self.effects.absorb(effect);
                let seq = self.publish(Event::LayoutChanged { workspace: from });
                if params.to != from {
                    self.publish(Event::LayoutChanged {
                        workspace: params.to,
                    });
                }
                let _ = reply.send(Ok(pane::MoveReply { seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_pane_focus(
        &mut self,
        params: pane::FocusParams,
        reply: Reply<pane::FocusReply>,
    ) {
        match self
            .state
            .move_focus(params.workspace, move_direction(params.direction))
        {
            Ok(effect) => {
                let moved = !matches!(effect, amx_core::Effect::Nothing);
                self.effects.absorb(effect);
                let pane = self
                    .state
                    .workspace(params.workspace)
                    .and_then(|w| w.focus());
                // A bump against the workspace edge changed nothing, so there
                // is no transition to publish; the reply still reports where
                // focus (already) is.
                let seq = if moved {
                    self.publish(Event::FocusChanged {
                        workspace: params.workspace,
                        pane,
                    })
                } else {
                    self.ctx.bus.head()
                };
                let _ = reply.send(Ok(pane::FocusReply { pane, seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_pane_resize(
        &mut self,
        params: pane::ResizeParams,
        reply: Reply<pane::ResizeReply>,
    ) {
        if !params.delta.is_finite() || params.delta < 0.0 {
            let _ = reply.send(Err(RpcError::new(
                RpcError::INVALID_PARAMS,
                format!(
                    "resize delta must be finite and non-negative, got {}",
                    params.delta
                ),
            )));
            return;
        }
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        let delta = match params.direction {
            pane::MoveDirection::Left | pane::MoveDirection::Up => -params.delta,
            pane::MoveDirection::Right | pane::MoveDirection::Down => params.delta,
        };
        match self.state.resize(ws, params.pane, delta) {
            Ok(effect) => {
                let resized = !matches!(effect, amx_core::Effect::Nothing);
                self.effects.absorb(effect);
                let seq = if resized {
                    self.publish(Event::LayoutChanged { workspace: ws })
                } else {
                    self.ctx.bus.head()
                };
                let _ = reply.send(Ok(pane::ResizeReply { resized, seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }

    pub(super) fn handle_pane_close(
        &mut self,
        params: pane::CloseParams,
        reply: Reply<pane::CloseReply>,
    ) {
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(Self::no_such_pane(params.pane)));
            return;
        };
        match self.state.close(ws, params.pane) {
            Ok(effect) => {
                self.effects.absorb(effect);
                if let Some(handle) = self.panes.remove(&params.pane) {
                    let _ = handle.try_send(PaneCommand::Kill);
                }
                let seq = self.publish(Event::LayoutChanged { workspace: ws });
                let _ = reply.send(Ok(pane::CloseReply { seq }));
            }
            Err(err) => {
                let _ = reply.send(Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    err.to_string(),
                )));
            }
        }
    }
}

fn split_direction(direction: pane::SplitDirection) -> Direction {
    match direction {
        pane::SplitDirection::Vertical => Direction::Right,
        pane::SplitDirection::Horizontal => Direction::Down,
    }
}

fn move_direction(direction: pane::MoveDirection) -> Direction {
    match direction {
        pane::MoveDirection::Left => Direction::Left,
        pane::MoveDirection::Down => Direction::Down,
        pane::MoveDirection::Up => Direction::Up,
        pane::MoveDirection::Right => Direction::Right,
    }
}

/// The user's shell, or a safe universal fallback.
///
/// Config (and so a configurable default command) is M1 (06 §1's dependency
/// table); `$SHELL` is the same default every terminal multiplexer uses
/// absent one.
fn default_shell() -> OsString {
    std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
}

/// Build the command a freshly spawned pane runs.
fn pty_command(cwd: PathBuf, command: Option<Vec<String>>) -> PtyCommand {
    let mut argv = command.into_iter().flatten();
    let (program, args) = match argv.next() {
        Some(first) => (OsString::from(first), argv.map(OsString::from).collect()),
        None => (default_shell(), Vec::new()),
    };
    PtyCommand {
        program,
        args,
        env: Vec::new(),
        cwd: Some(cwd),
        size: DEFAULT_SIZE,
    }
}
