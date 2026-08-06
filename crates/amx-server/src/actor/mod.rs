//! Mailbox types: what the actors say to each other.
//!
//! Two directions and two actors: the `Core` sends [`PaneCommand`] to a
//! `PaneHost` and receives [`PaneReport`] back; connection tasks send
//! [`CoreCommand`] to the `Core`. Every command that needs an answer carries
//! its own `oneshot` sender, so there is no correlation table to keep in sync
//! and no reply that can arrive for a request nobody is waiting on.

pub mod pane_host;

use std::path::PathBuf;

pub mod core;

use amx_core::{GridGeneration, InvalidationCause, PaneId, RowHash, RowId, RowRange};
use amx_proto::control::{client, pane, session, workspace};
use amx_proto::rpc::RpcError;
use amx_vt::SnapshotRef;
use bytes::Bytes;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

pub use pane_host::{PaneHost, PaneHostConfig, PaneHostError, PaneProbe, SnapshotFeed};

/// A reply channel for a command that answers.
pub type Reply<T> = oneshot::Sender<Result<T, RpcError>>;

/// What the `Core` asks of one `PaneHost`.
///
/// Everything that touches the pane's terminal state is a command, including
/// reads. 04 §3: "Scrollback-dependent reads — history ranges, persistence
/// snapshots — are served as commands executed on the parser actor thread,
/// serialized like herdr's PTY actor commands. Readers therefore never contend
/// with the parser on a pane-state mutex." A variant here that took a lock
/// instead would be reintroducing W10 by hand.
#[derive(Debug)]
pub enum PaneCommand {
    /// Write bytes into the pane's PTY.
    ///
    /// `Bytes` is refcounted: the same input can be handed to a pane and
    /// mirrored to a raw I/O stream without copying it.
    Write(Bytes),
    /// Resize the pane's grid, which bumps its generation and signals the child.
    Resize {
        /// New row count.
        rows: u16,
        /// New column count.
        cols: u16,
    },
    /// Publish and return the current POD cell snapshot.
    TakeSnapshot(oneshot::Sender<SnapshotRef>),
    /// Read a range of committed history.
    HistoryRange {
        /// The rows wanted.
        range: RowRange,
        /// Where to send them.
        reply: oneshot::Sender<Result<HistoryRows, HistoryError>>,
    },
    /// Read the foreground process's working directory.
    ///
    /// `None` when it cannot be read — the caller falls back to the pane's own
    /// cwd rather than failing the split (04 §7).
    ForegroundCwd(oneshot::Sender<Option<PathBuf>>),
    /// Signal the child process.
    Kill,
    /// Stop the actor, releasing the PTY and the terminal.
    Shutdown,
}

/// What a `PaneHost` tells the `Core`.
///
/// Reports are facts about what already happened; the `Core` turns them into
/// bus events and effects. The pane actor never publishes to the bus itself —
/// one publisher per transition is what keeps sequence numbers meaningful.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PaneReport {
    /// The grid has damage at this generation.
    Damage {
        /// The generation the damage belongs to.
        generation: GridGeneration,
    },
    /// Rows were committed to history with stable ids.
    ///
    /// `hashes` covers the *last* `hashes.len()` rows of `range` — 04 §3 has
    /// rows leaving the live grid announced "id + content hash" on the pane's
    /// delta stream, and hashing is capped per frame so a flood cannot put an
    /// unbounded cost on the frame path.
    Committed {
        /// The rows, in order.
        range: RowRange,
        /// Content hashes for the tail of `range`.
        hashes: Vec<RowHash>,
    },
    /// History at or beyond `from_row` no longer means what clients cached.
    Invalidated {
        /// First invalid row.
        from_row: RowId,
        /// Why.
        cause: InvalidationCause,
    },
    /// The eviction floor advanced.
    Evicted {
        /// The oldest row still fetchable.
        oldest_row: RowId,
    },
    /// The application set the terminal title.
    Title(String),
    /// The application rang the bell.
    Bell,
    /// The child process ended.
    Exited {
        /// Exit status, or `None` if it was signalled.
        status: Option<i32>,
    },
}

/// Rows of history, packed for transfer.
///
/// Packed rather than structured: history is served straight into the wire
/// encoding a [`HistoryChunk`](amx_proto::stream::HistoryChunk) carries, so a
/// bulk fetch never rebuilds a cell model it is about to flatten again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistoryRows {
    /// The rows this buffer holds.
    pub range: RowRange,
    /// Packed rows.
    pub rows: Vec<u8>,
}

/// Why a history range could not be served.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum HistoryError {
    /// The range starts below the pane's eviction floor.
    ///
    /// Refused, never faked: 04 §3 has clients treat evicted ranges as
    /// unavailable rather than rendering blanks, which only works if the server
    /// says so plainly.
    #[error("rows below the eviction floor of {oldest}")]
    Evicted {
        /// The oldest row still fetchable.
        oldest: RowId,
    },
    /// The range extends past what has been committed.
    #[error("rows past the committed head of {head}")]
    NotCommitted {
        /// The newest committed row.
        head: RowId,
    },
    /// The terminal could not serve the range.
    #[error("terminal error: {0}")]
    Terminal(String),
}

/// What a connection task asks of the `Core`.
///
/// One variant per dispatch group, mirroring the method table's domains: the
/// connection decodes a call, hands the `Core` the typed parameters and a reply
/// channel, and never touches session state itself.
#[derive(Debug)]
pub enum CoreCommand {
    /// A `session.*` call.
    Session(SessionCall),
    /// A `workspace.*` call.
    Workspace(WorkspaceCall),
    /// A `pane.*` call.
    Pane(PaneCall),
    /// A `client.*` call.
    Client(ClientCall),
    /// A pane actor reported a transition.
    PaneReport {
        /// Which pane.
        pane: PaneId,
        /// What happened.
        report: PaneReport,
    },
    /// Shut the session down.
    Shutdown,
}

/// `session.*` calls.
#[derive(Debug)]
pub enum SessionCall {
    /// `ping`.
    Ping {
        /// Parameters.
        params: session::PingParams,
        /// Where the reply goes.
        reply: Reply<session::PingReply>,
    },
}

/// `workspace.*` calls.
#[derive(Debug)]
pub enum WorkspaceCall {
    /// `workspace.create`.
    Create {
        /// Parameters.
        params: workspace::CreateParams,
        /// Where the reply goes.
        reply: Reply<workspace::CreateReply>,
    },
}

/// `pane.*` calls.
#[derive(Debug)]
pub enum PaneCall {
    /// `pane.split`.
    Split {
        /// Parameters.
        params: pane::SplitParams,
        /// Where the reply goes.
        reply: Reply<pane::SplitReply>,
    },
}

/// `client.*` calls.
#[derive(Debug)]
pub enum ClientCall {
    /// The client declared the panes its projection makes visible.
    Viewport {
        /// Parameters.
        params: client::Viewport,
        /// Where the acknowledgement goes.
        reply: Reply<()>,
    },
}

/// A handle on one pane actor's mailbox.
#[derive(Clone, Debug)]
pub struct PaneHandle {
    tx: mpsc::Sender<PaneCommand>,
}

impl PaneHandle {
    /// Wrap a mailbox sender.
    #[must_use]
    pub fn new(tx: mpsc::Sender<PaneCommand>) -> Self {
        Self { tx }
    }

    /// Send a command, waiting for mailbox capacity.
    ///
    /// Bounded mailboxes are the backpressure: a pane that cannot keep up slows
    /// its sender rather than growing an unbounded queue behind it.
    pub async fn send(&self, command: PaneCommand) -> Result<(), MailboxError> {
        self.tx.send(command).await.map_err(|_| MailboxError::Gone)
    }

    /// Whether the actor is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A handle on the `Core` actor's mailbox.
#[derive(Clone, Debug)]
pub struct CoreHandle {
    tx: mpsc::Sender<CoreCommand>,
}

impl CoreHandle {
    /// Wrap a mailbox sender.
    #[must_use]
    pub fn new(tx: mpsc::Sender<CoreCommand>) -> Self {
        Self { tx }
    }

    /// Send a command, waiting for mailbox capacity.
    pub async fn send(&self, command: CoreCommand) -> Result<(), MailboxError> {
        self.tx.send(command).await.map_err(|_| MailboxError::Gone)
    }

    /// Whether the actor is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A mailbox could not be used.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum MailboxError {
    /// The actor has stopped and its mailbox is closed.
    #[error("actor is gone")]
    Gone,
    /// The actor stopped before answering.
    #[error("actor dropped the reply channel")]
    NoReply,
}
