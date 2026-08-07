//! `pane.*` handlers: split, zoom, swap, move, rename, close.
//!
//! Split carries the value T16 exists for: [`Core::handle_split_live`]
//! resolves the source pane's *foreground process* cwd (04 §7) before
//! spawning the new pane's backing process, and [`Core::handle_split_no_spawn`]
//! is the synchronous fallback [`super::Core::absorb`] uses when it cannot
//! await that resolution (see the module doc on `super`). Swap and move never
//! touch a pane's actor at all, which is what makes "never restarts the
//! process" true by construction rather than by care.
//!
//! Opening the pty and starting the process is [`super::spawn`]'s, not this
//! file's: V02 moved it out ahead of M2 (R-M2-5), and V07 grows it there.

use amx_core::{Direction, Event};
use amx_proto::control::pane;
use amx_proto::rpc::RpcError;

use super::Core;
use crate::actor::Reply;

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
                let short = self.next_pane_short(new_pane);
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
            Ok(host) => {
                self.panes.insert(new_pane, host);
                // The pane was just minted: recording its cwd cannot fail.
                let _ = self.state.set_pane_cwd(new_pane, cwd);
                self.effects.absorb(effect);
                let seq = self.publish(Event::PaneCreated {
                    pane: new_pane,
                    workspace: ws,
                });
                let short = self.next_pane_short(new_pane);
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

    /// Give a pane a user-visible label.
    ///
    /// The counterpart of `workspace.rename`, and the reason a restored
    /// session comes back recognisable rather than as a row of identical
    /// shells: the label lives on the pane, rides `session.state` to every
    /// client, and is captured into the snapshot from there.
    pub(super) fn handle_pane_rename(
        &mut self,
        params: pane::RenameParams,
        reply: Reply<pane::RenameReply>,
    ) {
        match self
            .state
            .rename_pane(params.pane, Some(params.label.clone()))
        {
            Ok(effect) => {
                // Renaming a pane to the label it already carries is a legal
                // no-op with no transition to publish — the rule focus and
                // resize follow — and the reply still reports where it holds.
                let renamed = !matches!(effect, amx_core::Effect::Nothing);
                self.effects.absorb(effect);
                let seq = if renamed {
                    self.publish(Event::PaneRenamed {
                        pane: params.pane,
                        label: params.label,
                    })
                } else {
                    self.ctx.bus.head()
                };
                let _ = reply.send(Ok(pane::RenameReply { seq }));
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
                self.hang_up_pane(params.pane);
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
