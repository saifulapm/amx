//! The `AgentHub` mailbox, its handle, and the status view waits read.
//!
//! The fifth actor of 04 §2's table — "agent registry, hook-report ingestion,
//! status fusion, attention queue" — assembled on the `Persist` template:
//! bounded mailbox, handle held by `Core`, bus subscription taken at assembly,
//! spawned under the one `Runtime`.
//!
//! This file is the vocabulary the rest of M2 speaks to the hub in. The loop
//! itself is V08's, in `actor/agent_hub/`, and it lives in its own module for
//! the same reason `Persist`'s does: the mailbox is read by half the tree and
//! the loop by nothing.
//!
//! # The two read models, and the ordering that keeps waits honest
//!
//! `docs/08-m2-plan.md` §3, which is the reason [`StatusView`] exists at all:
//!
//! > Wait predicates must read live state … and dispatch runs on connection
//! > tasks, not inside the hub. So the hub maintains a shared `StatusView` …
//! > and the write protocol is fixed: **update `StatusView`, then publish the
//! > event.** A waiter woken by the event therefore always observes a view at
//! > least as new as the event; the reverse order can hang a wait forever
//! > (wake → read stale view → false → no further event ever comes).
//!
//! [`StatusView::commit`] is that protocol as the *only* way to change the
//! view, so the reverse order is not something a later task can write by
//! accident.
//!
//! The second consumer is slower and deliberately so: the hub `try_send`s a
//! status summary to `Core`, which folds it into pane state, and *that* mirror
//! feeds `session.state`, the status line and the final capture. Its mailbox
//! lag is harmless because nothing awaits on it. **Both consumers are named
//! here so nobody simplifies them into one.**
//!
//! # Shutdown
//!
//! `Persist`'s discipline, under the standing wedge-flake constraint (R-M1-2,
//! still undiagnosed): on cancellation the hub drains its own mailbox to
//! closure, drops the deadline wheel, publishes nothing, writes nothing, and
//! **sends no request to any sibling**. There is nothing to flush — refs and
//! status already live in `Core`'s mirror from the fire-and-forget sends made
//! during normal operation, so `Core`'s final capture carries them without
//! asking the hub anything. Detection state is derived and dies with the
//! process.
//!
//! # Task ownership
//!
//! V02 froze everything here. **V08** builds the actor behind it, assembles it
//! in `session/serve.rs`, and is the only writer [`StatusView::commit`] ever
//! has. **V13** added one mailbox variant, [`AgentCommand::Stanza`], for the
//! reason its own documentation gives: `agent.start` needs the registry, the
//! registry is parsed once and lives in the hub, and dispatch may not grow a
//! second copy of it.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use amx_core::agent::{AgentKind, AgentSnapshot, HookToken};
use amx_core::{Bus, Event, PaneId, Seq, WorkspaceId};
use amx_proto::control::agent as proto;
use tokio::sync::mpsc;

use super::{MailboxError, Reply, SnapshotFeed};
use crate::agent::registry::AgentStanza;

/// Depth of the `AgentHub`'s mailbox.
///
/// 64, from `docs/08-m2-plan.md` §3. Deeper than `Persist`'s 16 because hook
/// reports arrive in bursts — V01 §3 M9 measured parallel hooks for one event
/// starting within a millisecond of each other — and shallow enough that a hub
/// that has stopped draining applies backpressure rather than growing a queue
/// of stale status behind it.
pub const AGENT_MAILBOX: usize = 64;

/// What `AgentHub` is asked to do.
///
/// Note what is *not* here: nothing asks the hub for a [`SnapshotFeed`]. `Core`
/// owns the `PaneHost`s and is the only place a feed can be cloned from, so the
/// hub is *handed* one at [`PaneStarted`](Self::PaneStarted) and never reaches
/// for one. A variant that asked would be a request from the hub to `Core`,
/// which is exactly the sibling call the shutdown discipline above forbids.
#[derive(Debug)]
pub enum AgentCommand {
    /// One hook invocation, from a connection task's `agent.report` dispatch.
    ///
    /// The token is checked against the one this pane was spawned with; a
    /// mismatch is dropped and counted (D-M2-4's misattribution guard) rather
    /// than answered with an error, because a hook must never break a turn.
    HookReport {
        /// The pane the report claims.
        pane: PaneId,
        /// The token it carries.
        token: HookToken,
        /// The report itself.
        report: Box<proto::ReportParams>,
        /// Where the acknowledgement goes.
        reply: Reply<proto::ReportReply>,
    },
    /// A pane gained a live process. Sent by `Core`, `try_send`,
    /// fire-and-forget.
    PaneStarted {
        /// The pane.
        pane: PaneId,
        /// Its published frames, cloned from the `PaneHost` `Core` owns. Read
        /// per `PaneDamage` for tier-2 evaluation; contends with nothing.
        frames: SnapshotFeed,
        /// What `Core` knows about what it spawned.
        spawn: SpawnedIdentity,
    },
    /// A pane's process is gone and its tracker should retire.
    ///
    /// Distinct from the `PaneExited` the hub also sees on the bus: this one
    /// carries `Core`'s certainty that the pane itself is gone, and a retired
    /// tracker must also leave the attention queue — a queue holding a dead
    /// pane would send `next-attention` somewhere that is not there.
    PaneClosed {
        /// The pane.
        pane: PaneId,
    },
    /// `agent.explain`: how this pane's status was detected, rule by rule.
    Explain {
        /// The pane.
        pane: PaneId,
        /// Where the explanation goes.
        reply: Reply<proto::ExplainReply>,
    },
    /// `agent.next`: the head of the attention queue, focused.
    NextAttention {
        /// Cycle only this workspace's blocked agents, or the whole queue when
        /// absent (D15's scoped cycling). The queue itself stays global and
        /// block-time ordered; this narrows only which of its entries the call
        /// will focus.
        workspace: Option<WorkspaceId>,
        /// Where the answer goes.
        reply: Reply<proto::NextReply>,
    },
    /// What the registry says about one agent, by id or alias.
    ///
    /// `agent.start` needs a stanza's `start` argv before it can spawn
    /// anything, and dispatch runs on connection tasks where no registry lives.
    /// It asks *here* rather than parsing its own, because D-M2-2's whole claim
    /// is that `agents.toml` is parsed **once** and merged with the user's
    /// override once: a second parse in dispatch would be a second answer to
    /// "what is `claude`", it would miss the override V17's fake agents arrive
    /// through, and it would be W6 wearing a new hat.
    ///
    /// Question-shaped, like [`Explain`](Self::Explain) and
    /// [`NextAttention`](Self::NextAttention): answered from inside the actor,
    /// touching no sibling, refused outright once the session is cancelled.
    Stanza {
        /// The id or alias to resolve.
        kind: AgentKind,
        /// Where the answer goes.
        reply: Reply<Box<AgentStanza>>,
    },
}

/// What `AgentHub` tells the `Core`.
///
/// The other direction of the same conversation [`AgentCommand`] carries, and
/// the slow half of `docs/08-m2-plan.md` §3's two read models: wait predicates
/// read [`StatusView`] because they must see live state, while *chrome* and
/// *persistence* read `Core`'s mirror, which this fills. Its mailbox lag is
/// harmless precisely because nothing awaits on it.
///
/// Neither variant carries a reply channel, and that is the point: there is no
/// shape here in which the hub waits on `Core`, which is what lets the shutdown
/// discipline above hold. Sent with `try_send` and a dropped error — a lost
/// mirror message costs a stale status line until the next transition, and a
/// hub parked on a busy `Core` costs the session.
///
/// **V08** added it, with the actor that sends it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AgentCall {
    /// One pane's fused status, and the attention queue as it stands after it.
    ///
    /// `status` is `None` for a pane whose tracker retired — the same spelling
    /// [`StatusUpdate::status`] uses, so the two read models agree about what a
    /// dead pane is. The queue position is filled in on the snapshot before it
    /// is sent, because `Core` answers `session.state` from this map directly
    /// and has no queue of its own to derive one from.
    Status {
        /// The pane whose status changed.
        pane: PaneId,
        /// Its status now, or `None` to forget it.
        status: Option<Box<AgentSnapshot>>,
        /// The attention queue, in queue order.
        attention: Vec<PaneId>,
    },
    /// Put the user in front of `pane`: switch to its workspace, focus it.
    ///
    /// `agent.next`'s second half (D-M2-8). The hub answers its caller with the
    /// head of the queue immediately and sends this on its way; the client
    /// learns focus moved from the `FocusChanged` event, exactly as it does for
    /// every other focus change.
    Focus {
        /// The pane to focus.
        pane: PaneId,
    },
}

/// What `Core` knows about a pane's child at the moment it spawned it.
///
/// The spawn-side half of D-M2-4's identity scheme. The probe-side half — the
/// process-tree walk that says what is *actually* in the foreground now — is
/// V07's, and the two disagree all the time: a pane spawned as a shell can have
/// an agent typed into it by hand, and that agent is attributed exactly like
/// one `agent start` launched.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpawnedIdentity {
    /// The argv `Core` spawned, recorded so restore can reason about it and so
    /// identity has a starting guess.
    pub argv: Vec<String>,
    /// The token minted for this spawn and injected as `AMX_HOOK_TOKEN`.
    pub token: HookToken,
    /// The agent `agent start` asked for, when that is how the pane came to be.
    ///
    /// `None` for an ordinary split — which is the common case, and the one
    /// where identity has to earn its answer.
    pub requested: Option<AgentKind>,
}

/// A handle on the `AgentHub`'s mailbox.
///
/// Cloned into `Core` (for pane lifecycle) and into every connection task (for
/// `agent.*` dispatch). Same two-method shape as [`CoreHandle`](super::CoreHandle)
/// and [`PaneHandle`](super::PaneHandle), and the same rule about which to use:
/// [`send`](Self::send) when the caller can wait and the message matters,
/// [`try_send`](Self::try_send) for facts that are safe to drop.
#[derive(Clone, Debug)]
pub struct AgentHandle {
    tx: mpsc::Sender<AgentCommand>,
}

impl AgentHandle {
    /// Wrap a mailbox sender.
    #[must_use]
    pub const fn new(tx: mpsc::Sender<AgentCommand>) -> Self {
        Self { tx }
    }

    /// Send a command, waiting for mailbox capacity.
    ///
    /// What a connection task uses: an `agent.explain` that dropped its request
    /// because the hub was busy would report "no such pane" for a pane that
    /// exists.
    pub async fn send(&self, command: AgentCommand) -> Result<(), MailboxError> {
        self.tx.send(command).await.map_err(|_| MailboxError::Gone)
    }

    /// Send a command without waiting for mailbox capacity.
    ///
    /// What `Core` uses for pane lifecycle. `Core` must never park on the hub:
    /// the hub reads `Core`'s state through its own channels, and a `Core` that
    /// waited here while the hub waited there is one half of a deadlock. A lost
    /// `PaneStarted` costs that pane its tracking until the next spawn, which
    /// is a worse outcome than nothing but a far better one than a wedged
    /// session.
    pub fn try_send(&self, command: AgentCommand) -> Result<(), MailboxError> {
        self.tx.try_send(command).map_err(|_| MailboxError::Gone)
    }

    /// Whether the actor is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// One atomic change to the [`StatusView`], and the events that describe it.
///
/// Built by the hub, consumed by [`StatusView::commit`]. Carrying the events
/// *inside* the update is what fixes their ordering: there is no way to publish
/// an agent event except by handing it to the same call that performs the write
/// it describes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusUpdate {
    /// The pane whose status changed.
    pub pane: PaneId,
    /// Its new status, or `None` to retire it — a pane that exited, or one
    /// whose tracker was dropped.
    pub status: Option<AgentSnapshot>,
    /// The attention queue as it stands *after* this change, in queue order.
    ///
    /// The whole queue rather than a delta: the queue is short (it is one entry
    /// per blocked agent), and a delta would be a second representation of an
    /// order that already exists, which is how two views of one truth start
    /// disagreeing.
    pub attention: Vec<PaneId>,
    /// What to publish once the write has landed, in order.
    ///
    /// Typically one `agent_status`, sometimes with an
    /// `attention_enqueued`/`attention_dequeued` beside it, and an
    /// `agent_identified` on the transition that names the agent.
    pub events: Vec<Event>,
}

/// Every tracked pane's agent status, readable from any connection task.
///
/// The fast read model of `docs/08-m2-plan.md` §3. Wait predicates read it —
/// they must read *live* state, per 04 §2 — and dispatch runs on connection
/// tasks rather than inside the hub, so the truth has to be reachable without a
/// round trip through a mailbox.
///
/// A `std::sync::RwLock` and not a Tokio one, deliberately: nothing awaits
/// while holding a guard, every critical section is a map lookup or a small
/// write, and an async lock would add a scheduling point to the read path a
/// keystroke's wait sits on. Poisoning is recovered rather than propagated —
/// a panic elsewhere must not turn every subsequent wait into an error.
///
/// # The ordering
///
/// ```
/// use amx_core::agent::{AgentSnapshot, AgentState, StatusCause};
/// use amx_core::{Bus, Event, PaneId};
/// use amx_server::actor::{StatusUpdate, StatusView};
///
/// let bus = Bus::new(16);
/// let view = StatusView::new();
/// let pane = PaneId::new_v4();
///
/// // A subscriber taken *before* the transition: it can only ever be woken by
/// // the event `commit` publishes.
/// let _cursor = bus.subscribe();
///
/// let seq = view.commit(
///     &bus,
///     StatusUpdate {
///         pane,
///         status: Some(AgentSnapshot {
///             kind: None,
///             state: AgentState::Working,
///             cause: StatusCause::Hook,
///             transition_seq: bus.head() + 1,
///             attention: None,
///             session_ref: None,
///             reason: None,
///             since: None,
///         }),
///         attention: Vec::new(),
///         events: vec![Event::AgentStatus {
///             pane,
///             from: Some(AgentState::Idle),
///             to: AgentState::Working,
///             cause: StatusCause::Hook,
///         }],
///     },
/// );
///
/// // The event exists, so the write that precedes it inside `commit` does too.
/// // A waiter woken by that event therefore cannot read a stale view — which
/// // is the hang this ordering exists to make unbuildable.
/// assert_eq!(seq, Some(bus.head()));
/// assert_eq!(
///     view.get(pane).map(|status| status.state),
///     Some(AgentState::Working),
/// );
/// ```
///
/// V08's `status_view_is_current_before_the_status_event_is_receivable` races a
/// real subscriber against a real hub; this is the contract it tests.
#[derive(Clone, Debug, Default)]
pub struct StatusView {
    inner: Arc<RwLock<Statuses>>,
}

/// The map and the queue, under one lock.
///
/// One lock and not two: a reader that saw a pane `Blocked` but not yet on the
/// queue, or on the queue with a stale position, would be reading a state the
/// hub was never in.
#[derive(Debug, Default)]
struct Statuses {
    panes: HashMap<PaneId, AgentSnapshot>,
    attention: Vec<PaneId>,
}

impl StatusView {
    /// An empty view.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One pane's status, with its queue position filled in.
    ///
    /// The position is derived from the queue on read rather than stored on the
    /// snapshot, so a dequeue that shifts everyone behind it cannot leave a
    /// stale number in someone else's status.
    #[must_use]
    pub fn get(&self, pane: PaneId) -> Option<AgentSnapshot> {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let mut status = inner.panes.get(&pane).cloned()?;
        status.attention = position_of(&inner.attention, pane);
        Some(status)
    }

    /// The attention queue, in queue order.
    ///
    /// What `session.state` reports and `agent.next` takes the head of.
    #[must_use]
    pub fn attention(&self) -> Vec<PaneId> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .attention
            .clone()
    }

    /// Apply `update` and *then* publish its events, returning the sequence of
    /// the last one.
    ///
    /// `None` when the update carried no events — a bookkeeping change with no
    /// transition to announce, which is not a thing to publish an event about
    /// (04 §2 gives every *transition* a sequence number, not every write).
    ///
    /// The order of the two halves is the contract; see the type's
    /// documentation. This is the only mutator, which is what makes the order
    /// impossible to get wrong from outside.
    pub fn commit(&self, bus: &Bus, update: StatusUpdate) -> Option<Seq> {
        {
            let mut inner = self.inner.write().unwrap_or_else(PoisonError::into_inner);
            match update.status {
                Some(status) => {
                    inner.panes.insert(update.pane, status);
                }
                None => {
                    inner.panes.remove(&update.pane);
                }
            }
            inner.attention = update.attention;
        }
        // The guard is released before a single event is published: a
        // subscriber woken here may read this view immediately, and it must not
        // find the lock still held by the writer that woke it.
        let mut last = None;
        for event in update.events {
            last = Some(bus.publish(event));
        }
        last
    }
}

/// Where `pane` sits in `queue`, if it is on it.
fn position_of(queue: &[PaneId], pane: PaneId) -> Option<u32> {
    queue
        .iter()
        .position(|queued| *queued == pane)
        .and_then(|index| u32::try_from(index).ok())
}
