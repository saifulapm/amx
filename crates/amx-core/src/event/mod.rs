//! The event bus: the spine of the runtime (04 §2).
//!
//! "A broadcast bus with per-subscriber cursors over a bounded replay buffer.
//! Every state transition (`pane.created`, `pane.exited`, …) is one typed event
//! with a monotonic sequence number."
//!
//! Two contracts hang off that sentence and are enforced by the types here:
//! loss is *visible* ([`Delivery::Gap`], never a silent drop) and loss is
//! *recoverable* ([`wait::Waiter`], which re-evaluates state across a gap).

pub mod bus;
pub mod cursor;
pub mod wait;

use serde::{Deserialize, Serialize};

use crate::agent::{AgentKind, AgentState, StatusCause};
use crate::id::{ClientId, GridGeneration, PaneId, WorkspaceId};
use crate::scrollback::{InvalidationCause, RowId, RowRange};

pub use bus::Bus;
pub use cursor::{Delivery, Subscription};
pub use wait::{StatePredicate, WaitError, Waiter};

/// Monotonic bus sequence number.
///
/// Sequence numbers are assigned by [`Bus::publish`] and never reused. 04 §2:
/// "Every state-query response (and the subscribe reply) carries the bus
/// sequence number at which it was captured", which is what lets a consumer
/// that saw a gap re-query state and resume without racing new transitions.
pub type Seq = u64;

/// One state transition.
///
/// `#[non_exhaustive]`: M0 built three of the five actors in 04 §2, so
/// persistence and agent transitions add variants later — M1 added the first
/// two of them without a protocol bump and M2's four agent transitions are the
/// next, which is what the attribute is for. Every consumer must therefore have
/// a catch-all arm, and the skew harness asserts that an unhandled variant is a
/// soft failure on the wire rather than a hard one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// A pane was created in a workspace.
    PaneCreated {
        /// The new pane.
        pane: PaneId,
        /// The workspace it was created in.
        workspace: WorkspaceId,
    },
    /// The process in a pane exited.
    ///
    /// This is the event behind `--until exited`; `status` is `None` when the
    /// child was signalled rather than exited normally.
    PaneExited {
        /// The pane whose process ended.
        pane: PaneId,
        /// Exit status, if the child exited normally.
        status: Option<i32>,
    },
    /// A pane's grid was resized, and its generation bumped.
    PaneResized {
        /// The resized pane.
        pane: PaneId,
        /// New row count.
        rows: u16,
        /// New column count.
        cols: u16,
        /// The generation the resize produced.
        generation: GridGeneration,
    },
    /// A pane's grid has damage pending for the generation named.
    ///
    /// This is notification only: the damage itself is per-client bookkeeping
    /// and the cells are read from the authoritative grid at send time (04 §4).
    PaneDamage {
        /// The damaged pane.
        pane: PaneId,
        /// The generation the damage belongs to.
        generation: GridGeneration,
    },
    /// A pane was given a user-visible label.
    PaneRenamed {
        /// The renamed pane.
        pane: PaneId,
        /// The new label.
        label: String,
    },
    /// The application in a pane set the terminal title (OSC 0/2).
    PaneTitle {
        /// The pane.
        pane: PaneId,
        /// The title the application asked for.
        title: String,
    },
    /// Rows were committed to a pane's history and now have stable ids.
    HistoryCommitted {
        /// The pane.
        pane: PaneId,
        /// The rows that were committed.
        range: RowRange,
    },
    /// Cached history at or beyond `from_row` is no longer valid.
    ///
    /// 04 §3: clients "drop cached ranges and cancel any copy-mode selection
    /// anchored at or beyond `from_row`".
    HistoryInvalidated {
        /// The pane.
        pane: PaneId,
        /// The first row that is no longer what the client cached.
        from_row: RowId,
        /// Why it became invalid.
        cause: InvalidationCause,
    },
    /// A pane's eviction floor advanced; rows below `oldest_row` are gone.
    HistoryEvicted {
        /// The pane.
        pane: PaneId,
        /// The oldest row still fetchable.
        oldest_row: RowId,
    },
    /// A workspace was created.
    WorkspaceCreated {
        /// The new workspace.
        workspace: WorkspaceId,
    },
    /// A workspace was given a user-visible label.
    WorkspaceRenamed {
        /// The renamed workspace.
        workspace: WorkspaceId,
        /// The new label.
        label: String,
    },
    /// A workspace was closed, taking its panes with it.
    WorkspaceClosed {
        /// The closed workspace.
        workspace: WorkspaceId,
    },
    /// Focus moved within a workspace.
    FocusChanged {
        /// The workspace whose focus changed.
        workspace: WorkspaceId,
        /// The focused pane, or `None` if the workspace has no panes left.
        pane: Option<PaneId>,
    },
    /// A workspace's layout tree changed shape.
    LayoutChanged {
        /// The workspace whose layout changed.
        workspace: WorkspaceId,
    },
    /// A client attached to the session.
    ClientAttached {
        /// The client.
        client: ClientId,
    },
    /// A client detached from the session.
    ClientDetached {
        /// The client.
        client: ClientId,
    },
    /// A snapshot was applied at startup, with what it cost.
    ///
    /// Published exactly once, by the restore path, before the gateway accepts
    /// its first connection (D-M1-9) — so a client that attaches and then
    /// subscribes sees it in the replay buffer rather than missing it. The
    /// counts summarize a report whose entries `session.report` serves whole:
    /// 04 §6 requires restore loss to be queryable, never log-only.
    SessionRestored {
        /// Workspaces that came back.
        workspaces: u32,
        /// Panes that came back.
        panes: u32,
        /// Entities that could not be restored and were pruned.
        lost: u32,
        /// Entities restored with something missing (a cwd, a sidecar).
        degraded: u32,
    },
    /// The config file was re-read and applied.
    ///
    /// Published on every reload, including one that changed nothing, so tests
    /// and `amx events` consumers can wait on the reload rather than poll for
    /// its effects. `rejected_sections` counts the sections that kept their
    /// running values because the file's version of them did not parse
    /// (D-M1-8); the sections are *named* in the reload's diagnostics, which
    /// stay on the server side of the bus.
    ConfigReloaded {
        /// How many sections were rejected and kept their running values.
        rejected_sections: u32,
    },
    /// A pane's agent status changed.
    ///
    /// Published by `AgentHub` and by nothing else: 04 §2 gives every
    /// transition one publisher, read per event kind — the hub owns the agent
    /// kinds, the pane actor owns the pane-thread kinds, `Core` owns the rest
    /// (`docs/09-m3-plan.md` D-M3-2, which closed R-M2-3).
    ///
    /// The publish is ordered *after* the `StatusView` write it describes
    /// (`docs/08-m2-plan.md` §3). A waiter woken by this event therefore always
    /// reads a view at least as new as the event; the reverse order can hang a
    /// wait forever — wake, read a stale view, decide `false`, and no further
    /// event ever comes.
    AgentStatus {
        /// The pane whose agent moved.
        pane: PaneId,
        /// What it was, or `None` for the first status a pane ever takes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<AgentState>,
        /// What it is now.
        to: AgentState,
        /// What moved it. For an `edges` agent every exit from `working` or
        /// `blocked` is `screen` or `staleness` and never `hook` — see
        /// [`ExitAuthority`](crate::agent::ExitAuthority).
        cause: StatusCause,
    },
    /// A pane's foreground program was identified as a known agent.
    ///
    /// Separate from [`AgentStatus`](Self::AgentStatus) because identity and
    /// state move independently: an identified agent changes state many times,
    /// and a pane can change state (`busy`/`quiet`) without ever being
    /// identified. V01 §4 measured a case where the gap between them is long —
    /// Codex fires no hook at all until the first prompt, so a pane holding an
    /// idle Codex is identified by tier 3 alone, for as long as the user takes
    /// to type.
    AgentIdentified {
        /// The pane.
        pane: PaneId,
        /// Which agent it is running.
        kind: AgentKind,
    },
    /// A pane entered `blocked` and joined the attention queue.
    ///
    /// The queue is ordered by block time and a re-block re-enters at the tail
    /// (D-M2-8). This is the event the reference notifier filters on and the
    /// one the client turns into an OSC 9/99 to its host terminal — the one
    /// built-in notify path (03 §4).
    AttentionEnqueued {
        /// The pane now wanting attention.
        pane: PaneId,
    },
    /// A pane left the attention queue.
    ///
    /// On leaving `blocked` *or* on pane exit: a queue that kept a dead pane's
    /// entry would send `next-attention` to a pane that is not there.
    AttentionDequeued {
        /// The pane that no longer wants attention.
        pane: PaneId,
    },
}

impl Event {
    /// This event's wire tag — the `event` field serde writes.
    ///
    /// The match is exhaustive and lives *here*, in the crate that defines the
    /// enum, because `#[non_exhaustive]` puts an exhaustive match out of reach
    /// of every other crate including the test crates. That makes this the one
    /// place a new variant stops the build until it says what it is called,
    /// which is what the envelope goldens
    /// (`crates/amx-core/tests/event_goldens.rs`) hang their coverage on: a
    /// variant that reaches the bus without a frozen shape would otherwise
    /// reach M2's `amx events --json` the same way.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match *self {
            Self::PaneCreated { .. } => "pane_created",
            Self::PaneExited { .. } => "pane_exited",
            Self::PaneResized { .. } => "pane_resized",
            Self::PaneDamage { .. } => "pane_damage",
            Self::PaneRenamed { .. } => "pane_renamed",
            Self::PaneTitle { .. } => "pane_title",
            Self::HistoryCommitted { .. } => "history_committed",
            Self::HistoryInvalidated { .. } => "history_invalidated",
            Self::HistoryEvicted { .. } => "history_evicted",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::WorkspaceRenamed { .. } => "workspace_renamed",
            Self::WorkspaceClosed { .. } => "workspace_closed",
            Self::FocusChanged { .. } => "focus_changed",
            Self::LayoutChanged { .. } => "layout_changed",
            Self::ClientAttached { .. } => "client_attached",
            Self::ClientDetached { .. } => "client_detached",
            Self::SessionRestored { .. } => "session_restored",
            Self::ConfigReloaded { .. } => "config_reloaded",
            Self::AgentStatus { .. } => "agent_status",
            Self::AgentIdentified { .. } => "agent_identified",
            Self::AttentionEnqueued { .. } => "attention_enqueued",
            Self::AttentionDequeued { .. } => "attention_dequeued",
        }
    }
}

/// An [`Event`] with the sequence number it was published at.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// The bus sequence number of this event.
    pub seq: Seq,
    /// The transition itself.
    pub event: Event,
}
