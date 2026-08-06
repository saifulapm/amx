//! Reading the grid stream back.
//!
//! The server does not need this to serve a client — but "the client's grid is
//! never corrupted" is only a claim until something replays the emitted stream
//! and compares it, cell for cell, against the grid the server was reading
//! from. That is what `no_client_grid_is_corrupted_after_coalescing` does, and
//! this is the decoder it does it with.
//!
//! Owned, not borrowed. The encoder is the hot path and reuses its buffers; a
//! decoder runs once per frame on the receiving side and correctness there is
//! worth an allocation. It also cannot borrow: a `&[DamageRect]` cannot be
//! conjured out of an arbitrarily-aligned little-endian byte slice, which is
//! the reason [`GridMessage::decode`](amx_proto::stream::GridMessage::decode)
//! cannot be written as its signature was frozen (see [`crate::damage`]).

use amx_core::{GridGeneration, RowHash, RowId, RowRange};
use amx_proto::stream::{Cursor, DamageRect};

use super::cell::{self, PackedCell};
use super::codec::{CodecError, Reader, TAG_CURSOR, TAG_DELTA, TAG_RESET, TAG_SCROLLED};

/// One row of decoded cells.
pub type DecodedRow = Vec<PackedCell>;

/// One message off a pane's grid stream.
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
        /// One content hash per row in `range`.
        hashes: Vec<RowHash>,
    },
    /// Only the cursor moved.
    Cursor(Cursor),
}

/// Decode one grid stream payload.
///
/// # Errors
///
/// [`CodecError`] for a truncated payload, an unknown message tag, a cell with
/// reserved flags set, or a cell payload that does not line up with the rects
/// it belongs to.
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
            let count = reader.u16()?;
            let mut rects = Vec::with_capacity(usize::from(count));
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
            let count = reader.u32()?;
            let mut hashes = Vec::with_capacity(count as usize);
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
    let mut grid = Vec::with_capacity(wanted);
    for _ in 0..wanted {
        let count = cells.u16()?;
        let mut row = Vec::with_capacity(usize::from(count));
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

/// Append a `scrolled` message to `out`.
///
/// Rows leaving the live grid are announced on the pane's delta stream (04 §3);
/// the announcement itself is the history tracker's to make, but the layout is
/// the grid stream's, so it lives with the rest of it.
pub fn encode_scrolled(range: RowRange, hashes: &[RowHash], out: &mut Vec<u8>) {
    out.clear();
    out.push(TAG_SCROLLED);
    out.extend_from_slice(&range.first.get().to_le_bytes());
    out.extend_from_slice(&range.last.get().to_le_bytes());
    // The hash list is one entry per row in the range, itself `u64`-bounded.
    let count = u32::try_from(hashes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for hash in hashes.iter().take(count as usize) {
        out.extend_from_slice(&hash.get().to_le_bytes());
    }
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
        encode_scrolled(range, &hashes, &mut bytes);
        assert_eq!(
            decode(&bytes),
            Ok(Decoded::Scrolled {
                range,
                hashes: hashes.to_vec(),
            })
        );
    }

    #[test]
    fn an_unknown_tag_is_refused() {
        assert_eq!(decode(&[9]), Err(CodecError::UnknownTag { tag: 9 }));
    }

    #[test]
    fn an_empty_payload_is_truncated() {
        assert!(matches!(decode(&[]), Err(CodecError::Truncated { .. })));
    }
}
