//! The pane grid stream: keyframes, deltas, scroll notices and cursor moves.
//!
//! The byte layout, shared by the server's encoder and every decoder:
//!
//! ```text
//! message  := u8 tag, body
//!
//! 0 reset  := u64 generation, u16 rows, u16 cols, cursor,
//!             u32 cell byte count, rows            (every row, top to bottom)
//! 1 delta  := u64 generation, u16 rect count, rects, cursor,
//!             u32 cell byte count, rows      (the rects' rows, in rect order)
//! 2 scroll := u64 first row id, u64 last row id, u32 count, u64 hash × count
//! 3 cursor := cursor
//!
//! row      := u16 cell count, cells                  (cells per [`cell`])
//! rect     := u16 row, u16 col, u16 rows, u16 cols
//! cursor   := u16 row, u16 col, u8 flags (bit 0 visible, bit 1 blink),
//!             u8 shape (0 block, 1 underline, 2 bar)
//! ```
//!
//! The cell byte count is carried explicitly even though the rects imply how
//! many rows follow, because a cell is variable width: without it a decoder
//! that disagreed about a cell's length would silently mis-parse the rest of
//! the frame instead of failing at a known boundary. Rows carry their own cell
//! count for the same reason — a row need not be exactly `cols` cells wide.
//!
//! Encoding borrows ([`GridMessage`]); decoding owns ([`Decoded`]). The
//! borrowed decode the original contract declared cannot exist: a
//! `&[DamageRect]` cannot be conjured out of an arbitrarily-aligned
//! little-endian byte slice, so the decoder allocates — once per frame, on the
//! receiving side, where correctness is worth an allocation.

use amx_core::{GridGeneration, RowHash, RowId, RowRange};

use super::cell::{self, PackedCell};
use super::codec::{CodecError, Reader, put_cursor, put_rect};

/// A keyframe: the whole visible grid.
pub const TAG_RESET: u8 = 0;
/// An incremental update over a rect list.
pub const TAG_DELTA: u8 = 1;
/// Rows that left the live grid.
pub const TAG_SCROLLED: u8 = 2;
/// The cursor moved and nothing else did.
pub const TAG_CURSOR: u8 = 3;

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
/// The packed layout is [`cell`]'s; rows inside the run are self-delimiting
/// (`u16` cell count first).
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
        match *self {
            Self::Reset {
                generation,
                rows,
                cols,
                cells,
                cursor,
            } => {
                out.push(TAG_RESET);
                out.extend_from_slice(&generation.get().to_le_bytes());
                out.extend_from_slice(&rows.to_le_bytes());
                out.extend_from_slice(&cols.to_le_bytes());
                put_cursor(cursor, out);
                put_cells(cells, out);
            }
            Self::Delta {
                generation,
                rects,
                cells,
                cursor,
            } => {
                out.push(TAG_DELTA);
                out.extend_from_slice(&generation.get().to_le_bytes());
                // The rect list is bounded by the grid height upstream.
                let count = u16::try_from(rects.len()).unwrap_or(u16::MAX);
                out.extend_from_slice(&count.to_le_bytes());
                for rect in rects.iter().take(usize::from(count)) {
                    put_rect(*rect, out);
                }
                put_cursor(cursor, out);
                put_cells(cells, out);
            }
            Self::Scrolled { range, hashes } => {
                out.push(TAG_SCROLLED);
                out.extend_from_slice(&range.first.get().to_le_bytes());
                out.extend_from_slice(&range.last.get().to_le_bytes());
                // One entry per row in the range, itself `u64`-bounded.
                let count = u32::try_from(hashes.len()).unwrap_or(u32::MAX);
                out.extend_from_slice(&count.to_le_bytes());
                for hash in hashes.iter().take(count as usize) {
                    out.extend_from_slice(&hash.get().to_le_bytes());
                }
            }
            Self::Cursor(cursor) => {
                out.push(TAG_CURSOR);
                put_cursor(cursor, out);
            }
        }
    }
}

/// Append the packed cells with their length prefix.
fn put_cells(cells: Cells<'_>, out: &mut Vec<u8>) {
    // Cell bytes are bounded by the frame cap, itself a `u32` on the wire.
    let len = u32::try_from(cells.as_bytes().len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&cells.as_bytes()[..len as usize]);
}

/// One row of decoded cells.
pub type DecodedRow = Vec<PackedCell>;

/// One message off a pane's grid stream, owned.
///
/// The decoding counterpart of [`GridMessage`]: what the client applies to its
/// cached grid, and what a replay compares cell for cell against the grid the
/// server was reading from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// A keyframe: the whole visible grid.
    Reset {
        /// The generation the grid was read at.
        generation: GridGeneration,
        /// Rows in the visible grid.
        rows: u16,
        /// Columns in the visible grid.
        cols: u16,
        /// Every row, top to bottom.
        grid: Vec<DecodedRow>,
        /// The cursor at capture time.
        cursor: Cursor,
    },
    /// An incremental update over a rect list.
    Delta {
        /// The generation the cells were read at.
        generation: GridGeneration,
        /// The damaged rectangles, in the order their rows are packed.
        rects: Vec<DamageRect>,
        /// The rows the rects name, in rect order.
        grid: Vec<DecodedRow>,
        /// The cursor at send time.
        cursor: Cursor,
    },
    /// Rows left the live grid and entered history.
    Scrolled {
        /// The rows that were committed.
        range: RowRange,
        /// One content hash per row in `range`; may be empty when the sender
        /// did not hash.
        hashes: Vec<RowHash>,
    },
    /// Only the cursor moved.
    Cursor(Cursor),
}

/// Decode one grid stream payload.
///
/// Every buffer sized from a wire-declared count is bounded by what the
/// remaining payload can actually hold before it is allocated.
///
/// # Errors
///
/// [`CodecError`] for a truncated payload, an unknown message tag, a count the
/// payload cannot hold, a cell with reserved flags set, or a cell payload that
/// does not line up with the rects it belongs to.
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError> {
    let mut reader = Reader::new(bytes);
    match reader.u8()? {
        TAG_RESET => {
            let generation = GridGeneration::from_raw(reader.u64()?);
            let rows = reader.u16()?;
            let cols = reader.u16()?;
            let cursor = reader.cursor()?;
            let grid = read_rows(&mut reader, usize::from(rows))?;
            Ok(Decoded::Reset {
                generation,
                rows,
                cols,
                grid,
                cursor,
            })
        }
        TAG_DELTA => {
            let generation = GridGeneration::from_raw(reader.u64()?);
            let count = usize::from(reader.u16()?);
            reader.admits(count, super::codec::RECT_BYTES)?;
            let mut rects = Vec::with_capacity(count);
            for _ in 0..count {
                rects.push(reader.rect()?);
            }
            let cursor = reader.cursor()?;
            let wanted = rects
                .iter()
                .map(|rect| usize::from(rect.rows))
                .sum::<usize>();
            let grid = read_rows(&mut reader, wanted)?;
            Ok(Decoded::Delta {
                generation,
                rects,
                grid,
                cursor,
            })
        }
        TAG_SCROLLED => {
            let first = RowId::from_raw(reader.u64()?);
            let last = RowId::from_raw(reader.u64()?);
            let count = reader.u32()? as usize;
            reader.admits(count, size_of::<u64>())?;
            let mut hashes = Vec::with_capacity(count);
            for _ in 0..count {
                hashes.push(RowHash::from_raw(reader.u64()?));
            }
            Ok(Decoded::Scrolled {
                range: RowRange::new(first, last),
                hashes,
            })
        }
        TAG_CURSOR => Ok(Decoded::Cursor(reader.cursor()?)),
        tag => Err(CodecError::UnknownTag { tag }),
    }
}

/// Read the length-prefixed cell blob and split it into exactly `wanted` rows.
fn read_rows(reader: &mut Reader<'_>, wanted: usize) -> Result<Vec<DecodedRow>, CodecError> {
    let len = reader.u32()? as usize;
    let mut cells = Reader::new(reader.take(len)?);
    // Every row costs at least its two count bytes, so `wanted` rows cannot
    // outnumber half the cell bytes — checked before the outer Vec is sized.
    cells.admits(wanted, 2)?;
    let mut grid = Vec::with_capacity(wanted);
    for _ in 0..wanted {
        let count = usize::from(cells.u16()?);
        // A cell is at least its flag byte and its length byte.
        cells.admits(count, 2)?;
        let mut row = Vec::with_capacity(count);
        for _ in 0..count {
            row.push(cell::decode(&mut cells)?);
        }
        grid.push(row);
    }
    if !cells.is_empty() {
        return Err(CodecError::CellMismatch);
    }
    Ok(grid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scroll_notice_round_trips() {
        let range = RowRange::new(RowId::from_raw(4), RowId::from_raw(6));
        let hashes = [
            RowHash::from_raw(11),
            RowHash::from_raw(22),
            RowHash::from_raw(33),
        ];
        let mut bytes = Vec::new();
        GridMessage::Scrolled {
            range,
            hashes: &hashes,
        }
        .encode(&mut bytes);
        assert_eq!(
            decode(&bytes),
            Ok(Decoded::Scrolled {
                range,
                hashes: hashes.to_vec(),
            })
        );
    }

    #[test]
    fn a_cursor_message_round_trips() {
        let cursor = Cursor {
            row: 2,
            col: 5,
            visible: true,
            shape: CursorShape::Bar,
            blink: false,
        };
        let mut bytes = Vec::new();
        GridMessage::Cursor(cursor).encode(&mut bytes);
        assert_eq!(decode(&bytes), Ok(Decoded::Cursor(cursor)));
    }

    #[test]
    fn an_unknown_tag_is_refused() {
        assert_eq!(decode(&[9]), Err(CodecError::UnknownTag { tag: 9 }));
    }

    #[test]
    fn an_empty_payload_is_truncated() {
        assert!(matches!(decode(&[]), Err(CodecError::Truncated { .. })));
    }

    #[test]
    fn a_scroll_count_the_payload_cannot_hold_is_refused_before_allocation() {
        let mut bytes = Vec::new();
        bytes.push(TAG_SCROLLED);
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(CodecError::CountTooLarge { .. })
        ));
    }
}
