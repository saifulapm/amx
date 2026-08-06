//! The `Core` actor: owns [`SessionState`], folds `Effect`, schedules output.
//!
//! 04 §2: "every message handler returns an `Effect` value ...; the loop folds
//! effects and schedules output." This module is that loop. State mutation
//! goes through the T07 handlers on [`SessionState`] exclusively — `Core`
//! itself never edits a workspace or pane directly — and every handler that
//! changed something publishes exactly one [`Event`] before its effect is
//! folded into the batch.

use amx_core::{
    Ctx, Direction, Effect, EffectSet, Event, PaneId, Scheduled, SessionId, SessionState,
    ShortNumber, WorkspaceId,
};
use amx_proto::ServerInfo;
use amx_proto::control::{pane, session, workspace};
use amx_proto::rpc::RpcError;
use tokio::sync::mpsc;

use crate::actor::{ClientCall, CoreCommand, PaneCall, PaneReport, SessionCall, WorkspaceCall};

/// `Core`'s own program name, reported in [`session::PingReply`].
const SERVER_NAME: &str = "amx-server";

/// Where a folded batch's output goes.
///
/// `Core` schedules output once per batch of drained commands, never once per
/// command (that would reintroduce the per-message render churn D2 exists to
/// remove). Nothing downstream exists yet in M0 — the connection writer that
/// turns a [`Scheduled`] into wire traffic is T10/T11 — so this is the seam
/// they attach to, and tests exercise it with a closure.
pub trait OutputSink: Send + 'static {
    /// Consume one batch's folded effects.
    fn schedule(&mut self, scheduled: &Scheduled);
}

impl<F> OutputSink for F
where
    F: FnMut(&Scheduled) + Send + 'static,
{
    fn schedule(&mut self, scheduled: &Scheduled) {
        self(scheduled);
    }
}

/// The session's authoritative state and the actor loop over it.
///
/// `Core` is not `Clone`: there is exactly one per session, owning the one
/// [`SessionState`] and the counters and identity that go with it.
#[derive(Debug)]
pub struct Core {
    ctx: Ctx,
    state: SessionState,
    effects: EffectSet,
    session_id: SessionId,
    /// Placeholder short-number issuance: monotonic, never reused.
    ///
    /// `amx_core::ShortNumbers::assign`/`resolve` (the lowest-free-number,
    /// reuse-after-release mapping 04 §6 specifies) are still `todo!()` and no
    /// task in the DAG claims `amx-core/src/id.rs`'s bodies yet, so `Core`
    /// cannot call them without panicking. This counter is a stand-in scoped
    /// to T09 only; swap it for `ShortNumbers` once that lands.
    next_workspace_short: u32,
    /// See [`Self::next_workspace_short`].
    next_pane_short: u32,
}

impl Core {
    /// A fresh `Core` over an empty [`SessionState`], for the session `ctx`
    /// names.
    #[must_use]
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            state: SessionState::new(),
            effects: EffectSet::new(),
            session_id: SessionId::new_v4(),
            next_workspace_short: ShortNumber::FIRST.get(),
            next_pane_short: ShortNumber::FIRST.get(),
        }
    }

    /// The session context this actor was built from.
    #[must_use]
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// The session state tree, read-only.
    #[must_use]
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// This server instance's identity, as carried in `Welcome`/`ping`.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: SERVER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn next_workspace_short(&mut self) -> ShortNumber {
        let n = self.next_workspace_short;
        self.next_workspace_short += 1;
        ShortNumber::new(n)
    }

    fn next_pane_short(&mut self) -> ShortNumber {
        let n = self.next_pane_short;
        self.next_pane_short += 1;
        ShortNumber::new(n)
    }

    /// The workspace whose layout currently holds `pane`, if any.
    fn workspace_of(&self, pane: PaneId) -> Option<WorkspaceId> {
        self.state
            .workspaces()
            .find(|ws| ws.layout().contains(pane))
            .map(|ws| ws.id())
    }

    fn publish(&self, event: Event) -> amx_core::Seq {
        self.ctx.bus.publish(event)
    }

    /// Apply one command: mutate state through a T07 handler if it names one,
    /// publish the event that transition produced, answer any reply channel,
    /// and fold the resulting [`Effect`] into this batch.
    ///
    /// Never blocks and never awaits — every reply channel is a `oneshot`
    /// whose `send` is synchronous, so a whole mailbox can be drained without
    /// yielding between commands, which is what makes batching possible.
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
            CoreCommand::Workspace(WorkspaceCall::Create { params, reply }) => {
                let (ws, _pane, effect) = self.state.open_workspace();
                self.effects.absorb(effect);
                let mut seq = self.publish(Event::WorkspaceCreated { workspace: ws });
                if let Some(label) = params.label {
                    // Freshly created: renaming it cannot fail.
                    if let Ok(rename_effect) = self.state.rename_workspace(ws, Some(label.clone()))
                    {
                        self.effects.absorb(rename_effect);
                        seq = self.publish(Event::WorkspaceRenamed {
                            workspace: ws,
                            label,
                        });
                    }
                }
                let short = self.next_workspace_short();
                let _ = reply.send(Ok(workspace::CreateReply {
                    workspace: ws,
                    short,
                    seq,
                }));
            }
            CoreCommand::Pane(PaneCall::Split { params, reply }) => {
                self.handle_split(params, reply);
            }
            CoreCommand::Client(ClientCall::Viewport { params: _, reply }) => {
                let _ = reply.send(Ok(()));
            }
            CoreCommand::PaneReport { pane, report } => self.handle_pane_report(pane, report),
            CoreCommand::Shutdown => {}
        }
    }

    fn handle_split(
        &mut self,
        params: pane::SplitParams,
        reply: crate::actor::Reply<pane::SplitReply>,
    ) {
        let Some(ws) = self.workspace_of(params.pane) else {
            let _ = reply.send(Err(RpcError::new(
                RpcError::INVALID_PARAMS,
                format!("no such pane: {}", params.pane),
            )));
            return;
        };
        let dir = match params.direction {
            pane::SplitDirection::Vertical => Direction::Right,
            pane::SplitDirection::Horizontal => Direction::Down,
        };
        match self.state.split(ws, params.pane, dir, 0.5) {
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

    fn handle_pane_report(&mut self, pane: PaneId, report: PaneReport) {
        match report {
            PaneReport::Damage { generation } => {
                self.effects.absorb(Effect::PaneDamage(pane));
                self.publish(Event::PaneDamage { pane, generation });
            }
            // The hashes ride the pane's delta stream, not the bus: 04 §3 puts
            // them next to the rows they describe, and the bus event is the
            // session-state fact that ids `range` now exist.
            PaneReport::Committed { range, .. } => {
                self.publish(Event::HistoryCommitted { pane, range });
            }
            PaneReport::Invalidated { from_row, cause } => {
                self.publish(Event::HistoryInvalidated {
                    pane,
                    from_row,
                    cause,
                });
            }
            PaneReport::Evicted { oldest_row } => {
                self.publish(Event::HistoryEvicted { pane, oldest_row });
            }
            PaneReport::Title(title) => {
                self.publish(Event::PaneTitle { pane, title });
            }
            // `Event` has no bell variant (T01's frozen enum): a bell is not
            // session state, so there is nothing to publish. Flagged in T09's
            // report rather than added here — extending `Event` is T01's file.
            PaneReport::Bell => {}
            PaneReport::Exited { status } => {
                self.effects.absorb(Effect::PaneDamage(pane));
                self.publish(Event::PaneExited { pane, status });
            }
        }
    }

    /// Move this batch's folded effects into `out` and reset for the next one.
    ///
    /// `out` keeps its buffer capacity across calls (`EffectSet::drain_into`'s
    /// contract), so a steady state of drain-a-mailbox / schedule / repeat
    /// performs no allocation once warmed up.
    pub fn drain_scheduled(&mut self, out: &mut Scheduled) {
        self.effects.drain_into(out);
    }

    /// Run the actor: drain the mailbox, batch effects, schedule output,
    /// until [`Ctx::cancel`] fires, the mailbox closes, or a batch contained
    /// [`CoreCommand::Shutdown`].
    ///
    /// One `recv().await` per batch, then a non-blocking drain via
    /// `try_recv` — everything already queued when the actor wakes is folded
    /// into a single [`Scheduled`] output, never one per command (04 §2).
    pub async fn run(
        mut self,
        mut mailbox: mpsc::Receiver<CoreCommand>,
        mut sink: impl OutputSink,
    ) {
        let mut scheduled = Scheduled::new();
        loop {
            let cmd = tokio::select! {
                () = self.ctx.cancel.cancelled() => break,
                received = mailbox.recv() => match received {
                    Some(cmd) => cmd,
                    None => break,
                },
            };
            let mut stop = matches!(cmd, CoreCommand::Shutdown);
            self.absorb(cmd);
            while let Ok(cmd) = mailbox.try_recv() {
                stop |= matches!(cmd, CoreCommand::Shutdown);
                self.absorb(cmd);
            }
            if !self.effects.is_empty() {
                self.drain_scheduled(&mut scheduled);
                sink.schedule(&scheduled);
            }
            if stop {
                break;
            }
        }
    }
}
