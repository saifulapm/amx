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
/// `#[non_exhaustive]`: M0 builds three of the five actors in 04 §2, so
/// persistence and agent transitions add variants later. Every consumer must
/// therefore have a catch-all arm, and the skew harness asserts that an
/// unhandled variant is a soft failure on the wire rather than a hard one.
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
}

/// An [`Event`] with the sequence number it was published at.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// The bus sequence number of this event.
    pub seq: Seq,
    /// The transition itself.
    pub event: Event,
}
