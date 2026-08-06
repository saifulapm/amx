//! The `Core` actor: owns [`SessionState`], folds `Effect`, schedules output.
//!
//! 04 §2: "every message handler returns an `Effect` value ...; the loop folds
//! effects and schedules output." This module is that loop. State mutation
//! goes through the T07 handlers on [`SessionState`] exclusively — `Core`
//! itself never edits a workspace or pane directly — and every handler that
//! changed something publishes exactly one [`Event`] before its effect is
//! folded into the batch.
//!
//! `Core` also owns the session's live panes. Through [`Core::run`], every
//! verb that mints a pane also starts the process behind it: a split spawns
//! the new pane's command, and `workspace.create` (and the first-attach
//! seeding that reuses it) spawns a shell behind the fresh workspace's root
//! pane. The synchronous [`Core::absorb`] cannot do either — spawning a
//! `PaneHost` needs a Tokio runtime, and the two direct-`absorb` tests in
//! `tests/runtime.rs` run with none in scope — so each of those commands has
//! a `_no_spawn` fallback that mints state only, and [`Core::run`] routes the
//! real traffic to the `_live` handlers. A split is additionally `await`ed
//! individually: inheriting the source pane's *foreground process* cwd
//! (04 §7) means asking that pane's own actor over a mailbox, which `absorb`
//! cannot wait on. Everything else still folds through `absorb` a batch at a
//! time.
//!
//! Split by responsibility: this file is the struct, the mailbox loop and the
//! handlers with no dedicated domain (`ping`, pane reports); [`pane`] and
//! [`workspace`] hold the `pane.*`/`workspace.*` handlers themselves.

mod pane;
mod workspace;

use std::collections::HashMap;

use amx_core::{
    Ctx, Effect, EffectSet, Event, PaneId, Scheduled, SessionId, SessionState, ShortNumber,
    WorkspaceId,
};
use amx_proto::ServerInfo;
use amx_proto::control::session;
use amx_proto::rpc::RpcError;
use tokio::sync::mpsc;

use crate::actor::{
    ClientCall, CoreCommand, CoreHandle, PaneCall, PaneHandle, PaneReport, SessionCall,
    WorkspaceCall,
};

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
    /// Mailboxes of the panes currently backed by a live process.
    ///
    /// Every pane minted through [`Core::run`] has an entry (splits and
    /// workspace roots alike); one minted through a direct
    /// [`Core::absorb`] does not, since `absorb` spawns nothing. Removed
    /// when the pane exits or is closed, so a later close/kill never sends to
    /// a mailbox nobody is reading.
    panes: HashMap<PaneId, PaneHandle>,
    /// This actor's own mailbox, handed to every pane `Core` spawns so its
    /// reports have somewhere to go, and used to answer this same mailbox
    /// asynchronously when a command (a split) needs to await something
    /// before `absorb` can fold it.
    handle: CoreHandle,
}

impl Core {
    /// A fresh `Core` over an empty [`SessionState`], for the session `ctx`
    /// names, answering to its own mailbox at `handle`.
    ///
    /// `handle` must be the sending half of the very mailbox [`Core::run`] is
    /// later given — `Core` hands clones of it to every pane it spawns so
    /// [`amx_server::actor::PaneReport`]s and other pane-originated commands
    /// find their way back here.
    #[must_use]
    pub fn new(ctx: Ctx, handle: CoreHandle) -> Self {
        Self {
            ctx,
            state: SessionState::new(),
            effects: EffectSet::new(),
            session_id: SessionId::new_v4(),
            next_workspace_short: ShortNumber::FIRST.get(),
            next_pane_short: ShortNumber::FIRST.get(),
            panes: HashMap::new(),
            handle,
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

    /// The mailbox of the live actor backing `pane`, if a process was ever
    /// spawned for it and has not exited.
    ///
    /// Only reachable on an owned `Core` — before [`Core::run`] or after it
    /// hands the actor back — which is how a test asks a pane something
    /// (`PaneCommand::ForegroundCwd`, a snapshot) without a wire method for
    /// it.
    #[must_use]
    pub fn pane_handle(&self, pane: PaneId) -> Option<&PaneHandle> {
        self.panes.get(&pane)
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

    fn no_such_pane(pane: PaneId) -> RpcError {
        RpcError::new(RpcError::INVALID_PARAMS, format!("no such pane: {pane}"))
    }

    fn no_such_workspace(workspace: WorkspaceId) -> RpcError {
        RpcError::new(
            RpcError::INVALID_PARAMS,
            format!("no such workspace: {workspace}"),
        )
    }

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
            CoreCommand::Pane(PaneCall::Close { params, reply }) => {
                self.handle_pane_close(params, reply);
            }
            CoreCommand::Pane(PaneCall::Focus { params, reply }) => {
                self.handle_pane_focus(params, reply);
            }
            CoreCommand::Pane(PaneCall::Resize { params, reply }) => {
                self.handle_pane_resize(params, reply);
            }
            CoreCommand::Client(ClientCall::Viewport { params: _, reply }) => {
                let _ = reply.send(Ok(()));
            }
            CoreCommand::PaneReport { pane, report } => self.handle_pane_report(pane, report),
            CoreCommand::Shutdown => {}
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
                // The actor that reported this is on its way down (or already
                // gone) either way: nothing left to send it.
                self.panes.remove(&pane);
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
    /// into a single [`Scheduled`] output, never one per command (04 §2) —
    /// except the spawning commands, which [`Core::dispatch`] routes to their
    /// live handlers in place: see the type-level doc comment.
    ///
    /// Returns `self` once stopped, so a caller that needs to inspect the
    /// final state (a test, mainly — production callers run this under
    /// [`crate::runtime::Runtime::spawn`], which discards it) can.
    pub async fn run(
        mut self,
        mut mailbox: mpsc::Receiver<CoreCommand>,
        mut sink: impl OutputSink,
    ) -> Self {
        let mut scheduled = Scheduled::new();
        loop {
            let cmd = tokio::select! {
                () = self.ctx.cancel.cancelled() => break,
                received = mailbox.recv() => match received {
                    Some(cmd) => cmd,
                    None => break,
                },
            };
            let mut stop = self.dispatch(cmd).await;
            while let Ok(cmd) = mailbox.try_recv() {
                stop |= self.dispatch(cmd).await;
            }
            if !self.effects.is_empty() {
                self.drain_scheduled(&mut scheduled);
                sink.schedule(&scheduled);
            }
            if stop {
                break;
            }
        }
        self
    }

    /// Route one command to its live handler — the ones that spawn a process
    /// exist only on this path — or to [`Core::absorb`], and report whether
    /// it was [`CoreCommand::Shutdown`].
    async fn dispatch(&mut self, cmd: CoreCommand) -> bool {
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
            cmd => {
                let stop = matches!(cmd, CoreCommand::Shutdown);
                self.absorb(cmd);
                stop
            }
        }
    }
}

/// `Core`'s own program name, reported in [`session::PingReply`].
const SERVER_NAME: &str = "amx-server";
