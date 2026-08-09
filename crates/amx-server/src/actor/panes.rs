//! The `Core` ↔ `PaneHost` vocabulary: what is asked of a pane, and what a
//! pane says happened.
//!
//! Split out of [`super`] by W03 (`docs/09-m3-plan.md` R-M3-7) before M3's
//! handoff work pressed the file past the soft budget. The two halves of the
//! old module change for different reasons — this one when a *pane* learns a
//! new trick, [`super::calls`] when a *method* does — and neither of them is
//! the mailbox plumbing that stayed behind in [`super`].

use std::path::PathBuf;

use amx_core::{GridGeneration, InvalidationCause, RowHash, RowId, RowRange};
use amx_proto::control::session::MouseMode;
use amx_vt::SnapshotRef;
use bytes::Bytes;
use thiserror::Error;
use tokio::sync::oneshot;

use super::pane_host;

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
    /// Write bytes into the pane's *terminal*, without going near the pty.
    ///
    /// The replay half of persistence (D-M1-6): a restored pane's saved
    /// scrollback is fed to the fresh VT so the user's history is where they
    /// left it, and the child that has just been spawned knows nothing about
    /// it. `Write` cannot do this — those bytes go to the child — and nothing
    /// but restore should send this: bytes that did not come from the process
    /// are a lie about what the process printed, told exactly once, on purpose.
    ///
    /// Sent immediately after the spawn, so it is queued on the parser thread
    /// ahead of the child's first output, which still has a fork, an exec and
    /// a pty read to travel through.
    Seed(Vec<u8>),
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
    /// Put driven input in front of the child: 04 §8's `send-text`,
    /// `send-keys`, `run`.
    ///
    /// A command rather than a [`Write`](Self::Write) of bytes already made,
    /// because they cannot be made anywhere else: key encoding and paste
    /// bracketing are both questions about terminal state, and the terminal
    /// belongs to the parser thread (04 §3). Forwarded there with the caller's
    /// reply channel like any other terminal-dependent read;
    /// [`pane_host::drive`](crate::actor::pane_host::drive) has the whole
    /// argument, ordering against query replies included.
    Drive {
        /// What to put in front of the child.
        what: pane_host::Drive,
        /// Where the outcome goes.
        reply: oneshot::Sender<Result<pane_host::Driven, pane_host::DriveError>>,
    },
    /// Read the foreground process's working directory.
    ///
    /// `None` when it cannot be read — the caller falls back to the pane's own
    /// cwd rather than failing the split (04 §7).
    ForegroundCwd(oneshot::Sender<Option<PathBuf>>),
    /// Freeze the pane into the bytes and the descriptor a successor needs
    /// (D-M3-4, D-M3-5).
    ///
    /// A parser-thread command like every other read of terminal state, and a
    /// **quiesced-only** one: the answer is a duplicate of the master
    /// descriptor, which the pty actor hands out in no other state, so a pane
    /// that is still moving cannot be captured at all rather than being
    /// captured wrongly.
    ExportHandoff {
        /// Where the frozen pane goes.
        reply: oneshot::Sender<Result<pane_host::PaneExport, pane_host::ExportError>>,
    },
    /// Signal the child process.
    Kill,
    /// Stop the actor, releasing the PTY and the terminal.
    Shutdown,
}

/// What a `PaneHost` tells the `Core`.
///
/// Reports are facts about what already happened, and the pane actor has
/// already published every one of them: what `Core` takes from a report is the
/// fold beside it, never a second announcement (`docs/09-m3-plan.md` D-M3-2).
///
/// So a transition with no fold has no report. The title is the one that was:
/// `Core` answers no question about a pane title — no state row carries one —
/// and `Event::PaneTitle` already tells every client. W02 found the arm folding
/// to nothing once the republish went; W04 removed the message, because a
/// report whose only effect is a blocking send into a bounded `Core` mailbox is
/// D-M3-2's duplicate wearing a different hat. If a title ever lands in session
/// state, the report comes back with the fold that needs it.
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
    /// The application rang the bell.
    Bell,
    /// What the application asks its terminal to report about the mouse
    /// changed; `None` means it now asks for nothing.
    ///
    /// The exception that proves the rule above it: this is reported and *not*
    /// published, where every other variant here is both. A mouse mode is not a
    /// transition — nothing about the session moved, no client has to redraw,
    /// and 04 §2 gives sequence numbers to transitions. What it is is a fact
    /// `session.state` has to be able to answer synchronously, which is exactly
    /// the case the fold beside it exists for (`docs/notes/m4-mouse-path.md`
    /// §3).
    Mouse(Option<MouseMode>),
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
