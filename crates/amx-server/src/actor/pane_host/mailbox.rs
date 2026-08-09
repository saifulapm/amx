//! What the parser thread is asked to do, and what it says back.
//!
//! The pane's own vocabulary, kept beside the thread that speaks it rather than
//! inside it, exactly as [`crate::actor`] keeps the vocabulary the tokio actors
//! share. Splitting it out is the module budget doing its job: [`parser`] is the
//! terminal's exclusive owner and the file is long enough without also carrying
//! the two enums every other file in this directory has to name.
//!
//! [`parser`]: super::parser

use amx_core::platform::WinSize;
use amx_core::{GridGeneration, InvalidationCause, RowHash, RowId, RowRange};
use amx_proto::control::session::MouseMode;
use amx_vt::SnapshotRef;
use tokio::sync::oneshot;

use super::drive::{Drive, DriveError, Driven};
use crate::actor::{HistoryError, HistoryRows};
use crate::pty::ChildExit;

/// The buffer that shuttles between the pty thread and the parser thread.
///
/// One per pane, handed over with the bytes and handed back with the replies,
/// so the hot path is two moves and no allocation.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    /// Bytes read off the pty.
    pub(super) input: Vec<u8>,
    /// What the terminal wants written back, concatenated in order.
    pub(super) replies: Vec<u8>,
}

/// What the parser thread is asked to do.
///
/// Serialized like herdr's pty actor commands: one queue, one thread, so a
/// history read and a parse can never interleave inside the terminal.
pub(super) enum ParserCommand {
    /// Feed these bytes to the terminal and hand the buffer back with the
    /// replies. The sender blocks until it comes back.
    Parse(Box<Scratch>),
    /// Feed these bytes to the terminal and answer nobody.
    ///
    /// Restored scrollback (D-M1-6). Unlike [`Parse`](Self::Parse) there is no
    /// buffer to hand back and no reply to order: the bytes came off disk, not
    /// off the pty, so nothing is blocked waiting for them.
    Seed(Vec<u8>),
    /// Resize the grid, then the pty.
    Resize {
        /// New size.
        size: WinSize,
    },
    /// Publish a frame now and answer with it.
    Snapshot(oneshot::Sender<SnapshotRef>),
    /// Read a committed range of history.
    History {
        /// The rows wanted.
        range: RowRange,
        /// Where they go.
        reply: oneshot::Sender<Result<HistoryRows, HistoryError>>,
    },
    /// Put driven input in front of the child (04 §8's `send-text`,
    /// `send-keys`, `run`).
    ///
    /// A command on *this* queue rather than a write straight to the pty for
    /// two reasons. Encoding a key combo depends on the pane's DECCKM, keypad
    /// and Kitty-keyboard state, and the terminal holding that state is owned
    /// by this thread and no other. And queueing the bytes from here is what
    /// keeps driven input ordered against out-of-band query replies: a parse
    /// already under way finishes, and its replies reach the pty's write queue,
    /// before this command is even taken off the queue.
    Drive {
        /// What to put in front of the child.
        what: Drive,
        /// Where the outcome goes.
        reply: oneshot::Sender<Result<Driven, DriveError>>,
    },
    /// Freeze the pane for a live upgrade (D-M3-4).
    ///
    /// On this queue because everything it reads — the published grid, the
    /// modes, the title, the scrollback — is state only this thread may touch,
    /// and because being *behind* every parse already queued is what makes the
    /// capture describe a terminal that has stopped moving.
    Export(oneshot::Sender<Result<super::PaneExport, super::ExportError>>),
    /// Stop the thread.
    Stop,
}

/// What the parser thread tells the pane actor.
///
/// Sent on an unbounded channel on purpose: the pane actor can be awaiting a
/// pty round trip when one of these is produced, and a parser that blocked on a
/// full mailbox would deadlock against it. The rate is bounded by the frame
/// interval, not by output volume.
#[derive(Debug)]
pub(super) enum HostEvent {
    /// The application set the title.
    Title(String),
    /// The application rang the bell.
    Bell,
    /// The grid was resized and its generation bumped.
    ///
    /// Produced here rather than in the pane actor so the generation moves on
    /// the same thread that publishes frames: a bump made anywhere else could
    /// land between a frame and the generation a reader pairs it with.
    Resized {
        /// New row count.
        rows: u16,
        /// New column count.
        cols: u16,
        /// The generation the resize minted.
        generation: GridGeneration,
    },
    /// Rows were committed to history.
    Committed {
        /// The rows, in order.
        range: RowRange,
        /// Content hashes for the tail of `range` (04 §3).
        hashes: Vec<RowHash>,
    },
    /// Cached history at or beyond `from_row` is no longer valid.
    Invalidated {
        /// First invalid row.
        from_row: RowId,
        /// Why.
        cause: InvalidationCause,
    },
    /// The eviction floor advanced.
    Evicted {
        /// Oldest row still fetchable.
        oldest_row: RowId,
    },
    /// The application changed what it asks its terminal to report about the
    /// mouse — or asked for nothing, which is what `None` means.
    ///
    /// Sent only when the answer *moved*, so a pane producing output at full
    /// rate does not put a message on this channel per parsed chunk. See
    /// [`super::mouse`] for the read behind it.
    Mouse(Option<MouseMode>),
    /// The child process ended.
    Exited(ChildExit),
}
