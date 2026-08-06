//! The pane grid stream: keyframes, deltas, scroll notices and cursor moves.

use amx_core::{GridGeneration, RowHash, RowRange};

use crate::error::FrameError;

/// A rectangle of damaged cells, in grid coordinates.
///
/// Rects, not a queue of deltas: 04 §4 keeps "an accumulated dirty-region set
/// (rects + grid generation)" per client per pane, so a stalled client costs
/// bookkeeping proportional to the damaged *area*, not to how long it stalled.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DamageRect {
    /// Topmost damaged row.
    pub row: u16,
    /// Leftmost damaged column.
    pub col: u16,
    /// Number of rows.
    pub rows: u16,
    /// Number of columns.
    pub cols: u16,
}

/// Where the cursor is and what it looks like.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cursor {
    /// Row.
    pub row: u16,
    /// Column.
    pub col: u16,
    /// Whether the application has the cursor shown.
    pub visible: bool,
    /// Shape requested by the application (DECSCUSR).
    pub shape: CursorShape,
    /// Whether the shape blinks.
    pub blink: bool,
}

/// Cursor shapes a terminal application can ask for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CursorShape {
    /// Full cell block.
    #[default]
    Block,
    /// Underline.
    Underline,
    /// Vertical bar.
    Bar,
}

/// A borrowed run of packed cells.
///
/// Borrowed rather than owned because grid messages are built on the hot path:
/// the encoder reads cells out of the authoritative grid into a reused buffer
/// and hands out a slice of it. Owning the cells here would put an allocation
/// on every frame, which the performance rule forbids.
///
/// The packed layout is defined by the codec, not by this type; it stays
/// hand-rolled and little-endian so the goldens remain readable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cells<'a> {
    bytes: &'a [u8],
}

impl<'a> Cells<'a> {
    /// Borrow a packed cell run.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The packed bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Whether the run is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// One message on a pane's grid stream.
///
/// The generation appears on both cell-carrying variants because it is what
/// makes a delta interpretable: a delta whose generation does not match the
/// receiver's is not applied, it triggers a keyframe (04 §4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridMessage<'a> {
    /// A keyframe: the full visible grid.
    ///
    /// Sent when accumulated damage crosses a threshold, on
    /// [`FlowControl::Resync`](crate::stream::FlowControl::Resync), and on a
    /// reconnect whose generation is stale. Keyframes exist in protocol v1 "so
    /// overflow recovery is skew-compatible forever".
    Reset {
        /// The generation this grid is at.
        generation: GridGeneration,
        /// Rows in the visible grid.
        rows: u16,
        /// Columns in the visible grid.
        cols: u16,
        /// Every visible cell.
        cells: Cells<'a>,
        /// Cursor at capture time.
        cursor: Cursor,
    },
    /// An incremental update: the cells inside `rects`.
    ///
    /// Cells are read from the authoritative grid **at send time**, not at
    /// damage time, so coalescing damage while a client is stalled cannot make
    /// the delta describe a grid that no longer exists.
    Delta {
        /// The generation these cells were read at.
        generation: GridGeneration,
        /// The damaged rectangles, in the order the cells are packed.
        rects: &'a [DamageRect],
        /// Packed cells for `rects`.
        cells: Cells<'a>,
        /// Cursor at send time.
        cursor: Cursor,
    },
    /// Rows left the live grid and entered history with these ids.
    ///
    /// 04 §3: rows scrolling out are announced on the pane's delta stream with
    /// their id and content hash, which is what makes scrolling up during heavy
    /// output defined rather than racy.
    Scrolled {
        /// The rows that were committed, in order.
        range: RowRange,
        /// Content hash per row, parallel to `range`.
        hashes: &'a [RowHash],
    },
    /// Only the cursor moved.
    Cursor(Cursor),
}

impl GridMessage<'_> {
    /// Append the little-endian encoding of this message to `out`.
    ///
    /// `out` is caller-owned and reused across frames: encoding a grid message
    /// must not allocate on the hot path.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let _ = out;
        todo!("write the hand-rolled little-endian grid stream encoding")
    }
}

impl<'a> GridMessage<'a> {
    /// Decode a message that borrows from `bytes`.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, FrameError> {
        let _ = bytes;
        todo!("read the hand-rolled little-endian grid stream encoding")
    }
}
