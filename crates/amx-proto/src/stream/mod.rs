//! Binary streams: binding, flow control and channel priority (04 §4).

pub mod cell;
pub mod codec;
pub mod grid;
pub mod history;
pub mod raw;

use amx_core::PaneId;
use serde::{Deserialize, Serialize};

pub use cell::{CellRef, CellStyle, CellWide, PackedCell, Rgb, Underline};
pub use codec::{CodecError, Reader};
pub use grid::{Cells, Cursor, CursorShape, DamageRect, Decoded, GridMessage};
pub use history::{HistoryChunk, PackedRow, PackedRows};
pub use raw::{RawDirection, RawPaneIo};

/// A bound binary stream.
///
/// The id is also the frame channel byte, so ids above `u8::MAX` cannot be
/// bound simultaneously; the type is 16-bit because ids are not reused when a
/// stream unbinds — a late frame for a closed stream must be identifiable as
/// stale rather than land on whatever took the channel next.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamId(u16);

impl StreamId {
    /// Wrap a raw stream id.
    #[must_use]
    pub const fn new(id: u16) -> Self {
        Self(id)
    }

    /// The raw id.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// The frame channel byte carrying this stream, if it is bindable.
    ///
    /// `None` once ids have passed 255: the channel byte is the binding
    /// resource, and running out of it is a protocol-visible condition rather
    /// than a silent truncation.
    #[must_use]
    pub const fn channel(self) -> Option<u8> {
        if self.0 <= u8::MAX as u16 && self.0 != crate::frame::CONTROL_CHANNEL as u16 {
            Some(self.0 as u8)
        } else {
            None
        }
    }
}

/// What a stream carries.
///
/// A stream is bound by a control call and then speaks only its own kind, which
/// is what lets each kind be versioned per-stream instead of versioning the
/// whole protocol whenever one payload changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamKind {
    /// Grid deltas and keyframes for one pane.
    PaneGrid {
        /// The pane.
        pane: PaneId,
    },
    /// Chunked history ranges for one pane.
    History {
        /// The pane.
        pane: PaneId,
    },
    /// Raw pane I/O: observe, or control.
    RawPaneIo {
        /// The pane.
        pane: PaneId,
    },
    /// Reserved for pane graphics (kitty images). Not carried in M0.
    Graphics {
        /// The pane.
        pane: PaneId,
    },
}

impl StreamKind {
    /// The pane this stream is about.
    #[must_use]
    pub const fn pane(&self) -> PaneId {
        match *self {
            Self::PaneGrid { pane }
            | Self::History { pane }
            | Self::RawPaneIo { pane }
            | Self::Graphics { pane } => pane,
        }
    }
}

/// Backpressure and recovery signals for one stream.
///
/// Flow control is protocol v1, not a later optimization (04 §4): grid deltas
/// are incremental and cannot be dropped without corrupting the client's grid,
/// so both "stop sending" and "start over" have to exist from the first
/// version. [`Resync`](Self::Resync) is the client's way to ask for a keyframe
/// when it knows its grid is untrustworthy.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "flow", rename_all = "snake_case")]
pub enum FlowControl {
    /// The per-stream frame cap, negotiated at bind time.
    StreamCap {
        /// The stream.
        stream: StreamId,
        /// Largest payload the receiver will accept on it.
        max_frame: u32,
    },
    /// Stop sending on this stream.
    ///
    /// Damage accumulates into the per-client dirty set while paused; it is
    /// never queued as a backlog of deltas.
    Pause {
        /// The stream.
        stream: StreamId,
    },
    /// Resume sending on this stream.
    Resume {
        /// The stream.
        stream: StreamId,
    },
    /// Send a keyframe: the receiver's copy is not trustworthy.
    Resync {
        /// The stream.
        stream: StreamId,
    },
}

/// Writer priority class. Strict: a lower value always drains first.
///
/// 04 §4: "The connection writer has strict channel priority (control > grid
/// deltas > history/bulk), and history transfers are chunked — a big scrollback
/// fetch can never head-of-line-block a keystroke's response." Strict, not
/// weighted: a keystroke's reply must not wait behind bulk transfer even a
/// fraction of the time.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Control channel traffic: method calls and their replies.
    Control = 0,
    /// Grid deltas and keyframes.
    Grid = 1,
    /// History ranges and other bulk transfer.
    Bulk = 2,
}

impl Priority {
    /// The priority class a stream kind is written at.
    #[must_use]
    pub const fn of(kind: &StreamKind) -> Self {
        match kind {
            StreamKind::PaneGrid { .. } | StreamKind::Graphics { .. } => Self::Grid,
            StreamKind::History { .. } => Self::Bulk,
            StreamKind::RawPaneIo { .. } => Self::Grid,
        }
    }
}
