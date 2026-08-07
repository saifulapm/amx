//! `Core`'s routing: which handler a command reaches.
//!
//! Split out of [`super`] by V02 (`docs/08-m2-plan.md` R-M2-5) before M2 added
//! a line to either file: `core/mod.rs` stood at 500 of a 500-line soft budget,
//! and the two responsibilities in it part cleanly — the struct, its accessors
//! and the mailbox loop stayed there; the match that says *which handler a
//! command reaches* moved here. The move is mechanical: the arms are the arms
//! that were there, in the order they were in.
//!
//! Both directions of the routing live here together on purpose.
//! [`Core::absorb`] is the synchronous fold every command can take, and
//! [`Core::dispatch`] is the live path [`Core::run`](super::Core::run) uses,
//! which routes the four commands that need a Tokio runtime (or an `await`) to
//! their `_live` handlers first and falls through to `absorb` for everything
//! else. Reading one without the other is how an arm gets added to one and
//! forgotten in the other.

use amx_proto::control::{client, session};
use amx_proto::rpc::RpcError;

use super::Core;
use crate::actor::{
    ClientCall, CoreCommand, PaneCall, PaneWiring, SessionCall, StreamCall, WorkspaceCall,
};

impl Core {
    /// Apply one command: mutate state through a T07 handler if it names one,
    /// publish the event that transition produced, answer any reply channel,
    /// and fold the resulting [`Effect`] into this batch.
    ///
    /// Never blocks and never awaits — every reply channel is a `oneshot`
    /// whose `send` is synchronous, so a whole mailbox can be drained without
    /// yielding between commands, which is what makes batching possible.
    /// The commands that spawn a process when they reach the `Core` through
    /// [`Core::run`] — a split, a create, an attach to an empty session —
    /// cannot spawn here: reaching `absorb` directly still mints the state
    /// and folds its effect, but starts nothing, since starting a `PaneHost`
    /// needs a Tokio runtime (and a split's foreground-cwd inheritance an
    /// `await`) this function cannot use.
    pub fn absorb(&mut self, cmd: CoreCommand) {
        match cmd {
            CoreCommand::Session(SessionCall::Ping { params: _, reply }) => {
                let seq = self.ctx.bus.head();
                let _ = reply.send(Ok(session::PingReply {
                    server: self.server_info(),
                    session: self.session_id,
                    seq,
                }));
            }
            CoreCommand::Session(SessionCall::Attached { reply }) => {
                self.handle_attached_no_spawn(reply);
            }
            CoreCommand::Session(SessionCall::State { params: _, reply }) => {
                let _ = reply.send(Ok(self.session_state()));
            }
            CoreCommand::Session(SessionCall::Capture { sidecars, reply }) => {
                self.handle_capture(sidecars, reply);
            }
            CoreCommand::Session(SessionCall::Report { params: _, reply }) => {
                self.handle_report(reply);
            }
            CoreCommand::Workspace(WorkspaceCall::Create { params, reply }) => {
                self.handle_workspace_create_no_spawn(params, reply);
            }
            CoreCommand::Workspace(WorkspaceCall::Rename { params, reply }) => {
                self.handle_workspace_rename(params, reply);
            }
            CoreCommand::Workspace(WorkspaceCall::Kill { params, reply }) => {
                self.handle_workspace_kill(params, reply);
            }
            CoreCommand::Workspace(WorkspaceCall::Switch { params, reply }) => {
                self.handle_workspace_switch(params, reply);
            }
            CoreCommand::Pane(PaneCall::Split { params, reply }) => {
                self.handle_split_no_spawn(params, reply);
            }
            CoreCommand::Pane(PaneCall::Zoom { params, reply }) => {
                self.handle_pane_zoom(params, reply);
            }
            CoreCommand::Pane(PaneCall::Swap { params, reply }) => {
                self.handle_pane_swap(params, reply);
            }
            CoreCommand::Pane(PaneCall::Move { params, reply }) => {
                self.handle_pane_move(params, reply);
            }
            CoreCommand::Pane(PaneCall::Rename { params, reply }) => {
                self.handle_pane_rename(params, reply);
            }
            CoreCommand::Pane(PaneCall::Close { params, reply }) => {
                self.handle_pane_close(params, reply);
            }
            CoreCommand::Pane(PaneCall::Focus { params, reply }) => {
                self.handle_pane_focus(params, reply);
            }
            CoreCommand::Pane(PaneCall::Resize { params, reply }) => {
                self.handle_pane_resize(params, reply);
            }
            CoreCommand::Client(ClientCall::Viewport { params, reply }) => {
                self.handle_viewport(&params);
                let _ = reply.send(Ok(client::ViewportReply {
                    seq: self.ctx.bus.head(),
                }));
            }
            CoreCommand::Stream(StreamCall::Wiring { pane, reply }) => {
                let wiring = self.panes.get(&pane).map(|host| PaneWiring {
                    handle: host.handle().clone(),
                    frames: host.frames(),
                });
                let _ = reply.send(wiring.ok_or_else(|| {
                    if self.state.pane(pane).is_some() {
                        RpcError::new(
                            RpcError::INVALID_PARAMS,
                            format!("pane {pane} has no live process to stream"),
                        )
                    } else {
                        Self::no_such_pane(pane)
                    }
                }));
            }
            CoreCommand::PaneReport { pane, report } => self.handle_pane_report(pane, report),
            CoreCommand::Shutdown => {}
        }
    }

    /// Route one command to its live handler — the ones that spawn a process
    /// exist only on this path — or to [`Core::absorb`], and report whether
    /// it was [`CoreCommand::Shutdown`].
    pub(super) async fn dispatch(&mut self, cmd: CoreCommand) -> bool {
        match cmd {
            CoreCommand::Pane(PaneCall::Split { params, reply }) => {
                self.handle_split_live(params, reply).await;
                false
            }
            CoreCommand::Workspace(WorkspaceCall::Create { params, reply }) => {
                self.handle_workspace_create_live(params, reply);
                false
            }
            CoreCommand::Session(SessionCall::Attached { reply }) => {
                self.handle_attached_live(reply);
                false
            }
            // A capture on the live path refreshes each pane's cwd first,
            // which is an `await` on the panes' own mailboxes and so cannot
            // happen inside `absorb`. Same split as a create or a split.
            CoreCommand::Session(SessionCall::Capture { sidecars, reply }) => {
                self.handle_capture_live(sidecars, reply).await;
                false
            }
            cmd => {
                let stop = matches!(cmd, CoreCommand::Shutdown);
                self.absorb(cmd);
                stop
            }
        }
    }
}
