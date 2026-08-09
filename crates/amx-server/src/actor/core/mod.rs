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
//! Split by responsibility: this file is the struct, its accessors and the
//! mailbox loop. Everything else lives beside it — [`route`] for the match
//! that decides which handler a command reaches, [`pane`] and [`workspace`]
//! for the `pane.*`/`workspace.*` verbs, [`spawn`] for opening a pty and
//! starting the process behind a freshly minted pane, [`report`] for the fold
//! over what pane actors report back, [`persist`] for the snapshot capture the
//! `Persist` actor asks for, [`restore`] for the startup rebuild that puts
//! yesterday's session back, and [`resume`] for the half of that rebuild which
//! puts each agent's *conversation* back (V15, D-M2-7).

mod agents;
mod handoff;
mod import;
mod pane;
mod persist;
mod report;
mod restore;
mod resume;
mod route;
mod spawn;
mod view;
mod workspace;

use std::collections::HashMap;
use std::time::Duration;

use amx_core::{
    Config, Ctx, EffectSet, Event, Level, PaneId, RowId, Scheduled, SessionId, SessionState,
    ShortNumber, ShortNumbers, WorkspaceId,
};
use amx_proto::ServerInfo;
use amx_proto::rpc::RpcError;
use tokio::sync::{mpsc, watch};

pub use self::import::{AdoptError, Adopted};
pub use self::persist::CAPTURE_PROBE_BUDGET;
use self::persist::Restored;
pub use self::restore::RestoreOptions;
use crate::actor::{
    AgentHandle, CoreCommand, CoreHandle, PaneHandle, PaneHost, PersistCommand, PersistHandle,
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
    /// The short number each workspace holds, for `session.state`.
    ///
    /// The lowest-free-number, reuse-after-release mapping of 04 §6: assigned
    /// at creation, adopted back from a snapshot or a manifest, and released
    /// by [`Core::release_departed_shorts`] when the object leaves the tree.
    workspace_shorts: ShortNumbers<WorkspaceId>,
    /// The short number each pane holds, for `session.state`. See
    /// [`Self::workspace_shorts`].
    pane_shorts: ShortNumbers<PaneId>,
    /// The panes currently backed by a live process, actor task included.
    ///
    /// Every pane minted through [`Core::run`] has an entry (splits and
    /// workspace roots alike); one minted through a direct
    /// [`Core::absorb`] does not, since `absorb` spawns nothing. Moved to
    /// [`Core::draining`] when the pane is closed or its child exits, so a
    /// later close/kill never sends to a mailbox nobody is reading — but the
    /// task handle itself is never dropped before it is joined (04 §2).
    panes: HashMap<PaneId, PaneHost>,
    /// Panes on their way down: hung up (or already exited) but not yet
    /// joined. Drained when [`Core::run`] exits, which is what makes "nothing
    /// detached, everything joined" hold for pane tasks.
    draining: Vec<PaneHost>,
    /// Each pane's history window, folded from its reports so `session.state`
    /// can answer synchronously: `head` is one past the newest committed row,
    /// `floor` the oldest still fetchable.
    history: HashMap<PaneId, (RowId, RowId)>,
    /// What each pane's application asks its terminal to report about the
    /// mouse, folded from its reports so `session.state` can answer
    /// synchronously — the history window's shape, for the history window's
    /// reason (`docs/notes/m4-mouse-path.md` §3). A pane with no entry asked
    /// for nothing.
    mouse: HashMap<PaneId, amx_proto::control::session::MouseMode>,
    /// Each tracked pane's fused agent status, mirrored here by `AgentHub`.
    ///
    /// The slow read model of `docs/08-m2-plan.md` §3. `session.state`, the
    /// status line and the final capture read *this*, never the hub — which is
    /// what lets the hub's shutdown send nothing to anybody (R-M1-2): whatever
    /// it knew is already here.
    agent_status: HashMap<PaneId, amx_core::agent::AgentSnapshot>,
    /// The attention queue in queue order, mirrored by `AgentHub` beside the
    /// statuses (D-M2-8 — `session.state` *is* the query for the queue).
    attention: Vec<PaneId>,
    /// The active client's declared projection — its terminal size, and the
    /// pane it is drawing when it declared exactly one (D14) — if any client
    /// has declared one. Last writer wins: the pane grid follows the
    /// most-recently-active client (04 §3). [`view`] is what reads it.
    viewport: Option<self::view::Viewport>,
    /// The size (rows, cols) each pane was last commanded to, so reconciling
    /// after a layout change only disturbs panes whose cell rect moved.
    pane_sizes: HashMap<PaneId, (u16, u16)>,
    /// This actor's own mailbox, handed to every pane `Core` spawns so its
    /// reports have somewhere to go, and used to answer this same mailbox
    /// asynchronously when a command (a split) needs to await something
    /// before `absorb` can fold it.
    handle: CoreHandle,
    /// The `Persist` actor's mailbox, when the session has one.
    ///
    /// Used for exactly one message in normal operation — the final capture
    /// pushed on the way down (`docs/07-m1-plan.md` §2) — because dirtiness is
    /// *observed* off the event bus, not pushed: `Core` has no "remember to
    /// tell persistence" call sites and never gains any.
    persist: Option<PersistHandle>,
    /// `AgentHub`'s mailbox, when the session has one.
    ///
    /// Written to with `try_send` only, and only about pane lifecycle: `Core`
    /// must never park on the hub, because the hub reads `Core`'s mirror
    /// through this same actor and a `Core` waiting there while the hub waits
    /// here is one half of a deadlock (`docs/08-m2-plan.md` §3).
    agent: Option<AgentHandle>,
    /// What this session keeps about handing itself over (`docs/09-m3-plan.md`
    /// §3): the commit fence the final capture reads, the in-flight flag that
    /// refuses a second handoff, the last attempt `session.report` answers
    /// with, and where a frozen session is sent.
    handoff: self::handoff::HandoffSeat,
    /// What this server's startup restore cost, if it performed one.
    ///
    /// Held for the process's lifetime: `session.report` serves the entries
    /// whole and `session.state` carries their counts, so the client can show
    /// the loss indicator without a second call (04 §6).
    restore: Option<Restored>,
    /// How long a capture waits for the foreground-cwd probes it started.
    capture_budget: Duration,
    /// The user's live configuration, borrowed at the moment a pane is spawned
    /// rather than cached — which is what makes `[terminal] shell` hot for new
    /// panes without ever disturbing a running one (D-M1-8).
    config: watch::Receiver<Config>,
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
            workspace_shorts: ShortNumbers::new(),
            pane_shorts: ShortNumbers::new(),
            panes: HashMap::new(),
            draining: Vec::new(),
            history: HashMap::new(),
            mouse: HashMap::new(),
            agent_status: HashMap::new(),
            attention: Vec::new(),
            agent: None,
            viewport: None,
            pane_sizes: HashMap::new(),
            handle,
            persist: None,
            handoff: self::handoff::HandoffSeat::default(),
            restore: None,
            capture_budget: CAPTURE_PROBE_BUDGET,
            // Defaults, with no sender to change them: a `Core` built without a
            // config runtime runs as if the user had no `config.toml`.
            config: watch::channel(Config::default()).1,
        }
    }

    /// Tell this `Core` where the running configuration is published.
    ///
    /// The serve path calls this before the first pane is spawned. A `Core`
    /// that is never told reads the defaults forever, which is what every test
    /// that does not care about configuration gets.
    pub fn set_config(&mut self, config: watch::Receiver<Config>) {
        self.config = config;
    }

    /// Tell this `Core` where persistence is listening.
    ///
    /// Only the shutdown push uses it (see [`Core::persist`]); a `Core` without
    /// one simply has nothing to push to, which is what every test that does
    /// not care about persistence gets.
    pub fn set_persist(&mut self, persist: PersistHandle) {
        self.persist = Some(persist);
    }

    /// Tell this `Core` where `AgentHub` is listening.
    ///
    /// The serve path calls this before the first pane is spawned, so that
    /// every pane's `PaneStarted` reaches the hub — including the ones the
    /// startup restore brings back. A `Core` that is never told simply tracks
    /// no agents, which is what every test that does not care about them gets.
    pub fn set_agent(&mut self, agent: AgentHandle) {
        self.agent = Some(agent);
    }

    /// Change how long a capture waits for its foreground-cwd probes.
    ///
    /// [`CAPTURE_PROBE_BUDGET`] is the default and the one the server uses.
    /// Zero turns the refresh off: a capture that cannot wait for an answer
    /// does not ask, and every pane contributes its stored cwd — the same
    /// outcome a stalled probe produces, reached without the wait.
    pub const fn set_capture_budget(&mut self, budget: Duration) {
        self.capture_budget = budget;
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
        self.panes.get(&pane).map(PaneHost::handle)
    }

    /// This server instance's identity, as carried in `Welcome`/`ping`.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Continue an existing session's identity instead of minting a new one.
    ///
    /// The inheritance a live upgrade turns on (`docs/09-m3-plan.md` D-M3-5).
    /// A reconnecting client branches on `Welcome.session`: the same id means
    /// "this is the session I was talking to, my caches and my last seq still
    /// mean something", a different one means "a different server, drop
    /// everything". A successor that minted a fresh id would therefore tell
    /// every client to throw away exactly the state the handoff went to the
    /// trouble of carrying across.
    ///
    /// Only the import assembly calls this, and only before [`Core::run`]:
    /// changing a live session's identity mid-flight would be the same lie in
    /// the other direction.
    #[must_use]
    pub const fn with_session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = session_id;
        self
    }

    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: SERVER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// The number a new workspace is displayed as: the lowest one free.
    fn next_workspace_short(&mut self, workspace: WorkspaceId) -> ShortNumber {
        self.release_departed_shorts();
        self.workspace_shorts.assign(workspace)
    }

    /// The number a new pane is displayed as: the lowest one free.
    fn next_pane_short(&mut self, pane: PaneId) -> ShortNumber {
        self.release_departed_shorts();
        self.pane_shorts.assign(pane)
    }

    /// Free the number of every workspace and pane no longer in the tree.
    ///
    /// One rule in one place: a short number is held exactly while its object
    /// is in [`Core::state`]. The alternative — a release beside every close,
    /// kill and prune — is a handful of call sites that each have to remember,
    /// and the one that forgets leaves a number pointing at nothing forever.
    ///
    /// Swept immediately before an assignment rather than after a close, so
    /// that what the next number is depends on the tree and never on which
    /// batch the close landed in: 04 §6's numbers are what a user types, and a
    /// split that answers `4` or `2` depending on scheduling is not a mapping
    /// anybody can learn. It costs one walk of the map per create, on a map
    /// with one entry per live pane.
    fn release_departed_shorts(&mut self) {
        let state = &self.state;
        self.pane_shorts.retain(|pane| state.pane(*pane).is_some());
        self.workspace_shorts
            .retain(|ws| state.workspace(*ws).is_some());
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

    /// Move this batch's folded effects into `out` and reset for the next one.
    ///
    /// `out` keeps its buffer capacity across calls (`EffectSet::drain_into`'s
    /// contract), so a steady state of drain-a-mailbox / schedule / repeat
    /// performs no allocation once warmed up.
    ///
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
                // A batch that moved the layout moved pane rects with it: the
                // active client's declared size projects onto the new tree and
                // every pane whose cell rect changed is resized (04 §3 — the
                // pane grid follows the most-recently-active client).
                if scheduled.level() >= Level::Layout {
                    self.reconcile_pane_sizes();
                }
                sink.schedule(&scheduled);
            }
            if stop {
                break;
            }
        }
        self.push_final_capture();
        // The mailbox closes before the panes are joined: an actor mid-report
        // must see its send fail rather than wait on a receiver nobody will
        // drain again.
        drop(mailbox);
        self.join_panes().await;
        self
    }

    /// Hand persistence one last snapshot on the way down.
    ///
    /// Built from stored state only — the panes are about to be killed and
    /// probing them would mean waiting on actors that are already draining —
    /// and pushed with `try_send`, so a full persistence mailbox can never
    /// wedge `Core`'s own drain (`docs/07-m1-plan.md` §2, R-M1-2). A push that
    /// does not fit costs the last few seconds of changes, which the debounced
    /// save has already bounded.
    ///
    /// Fenced after a handoff commits (R-M3-10, D-M3-6 point 5). Past the
    /// commit the snapshot on disk is the successor's, and this dying view of
    /// the session would overwrite it with panes the successor has already
    /// resumed and a layout it may already have changed — a split brain amx
    /// closes rather than outraces. Any future change to this path must keep
    /// the fence; `no_final_snapshot_is_written_after_commit` is the pin.
    fn push_final_capture(&self) {
        if self.handoff_fenced() {
            tracing::info!("the successor owns the snapshot; not writing a final capture");
            return;
        }
        let Some(persist) = &self.persist else { return };
        let _ = persist.try_send(PersistCommand::Snapshot(Box::new(self.capture_cheap())));
    }
}

/// `Core`'s own program name, reported in [`session::PingReply`].
const SERVER_NAME: &str = "amx-server";
