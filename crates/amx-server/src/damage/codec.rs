//! The grid stream's byte layout: message tags, the cursor, and a reader.
//!
//! Hand-rolled little-endian, like every other amx wire encoding, so the bytes
//! stay legible in a dump and in a golden. The shape of a message is:
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
//! row      := u16 cell count, cells
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
//! This lives in `amx-server` and not in `amx-proto` because
//! [`GridMessage::encode`](amx_proto::stream::GridMessage::encode) is still
//! `todo!()` and its `decode` counterpart cannot be written as declared — see
//! the module docs of [`crate::damage`].

use amx_proto::stream::{Cursor, CursorShape, DamageRect};
use amx_vt::CursorStyle;
use thiserror::Error;

/// A keyframe: the whole visible grid.
pub const TAG_RESET: u8 = 0;
/// An incremental update over a rect list.
pub const TAG_DELTA: u8 = 1;
/// Rows that left the live grid.
pub const TAG_SCROLLED: u8 = 2;
/// The cursor moved and nothing else did.
pub const TAG_CURSOR: u8 = 3;

/// Bytes one rect occupies.
pub const RECT_BYTES: usize = 8;
/// Bytes one cursor occupies.
pub const CURSOR_BYTES: usize = 6;

/// The cursor is drawn.
const CURSOR_VISIBLE: u8 = 1 << 0;
/// The cursor blinks.
const CURSOR_BLINK: u8 = 1 << 1;

/// A grid stream payload could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum CodecError {
    /// The payload ended before the message did.
    #[error("grid message truncated: wanted {wanted} more bytes, {left} left")]
    Truncated {
        /// Bytes the next field needs.
        wanted: usize,
        /// Bytes still in the payload.
        left: usize,
    },
    /// The tag byte names a message this build does not know.
    #[error("unknown grid message tag {tag}")]
    UnknownTag {
        /// The tag that arrived.
        tag: u8,
    },
    /// A cell set a flag bit no version of the layout has defined.
    #[error("cell flags {flags:#010b} set a reserved bit")]
    Reserved {
        /// The flags byte.
        flags: u8,
    },
    /// A cell's text was not UTF-8.
    #[error("cell text is not utf-8")]
    BadText,
    /// The message declared more cell bytes than it carried, or fewer than its
    /// rects need.
    #[error("cell payload does not match the rects it belongs to")]
    CellMismatch,
}

/// A cursor position on the wire.
///
/// `visible` folds in `visible_in_viewport`: a cursor scrolled into history is
/// not drawn, and the client has no way to tell the difference from the two
/// flags separately without also knowing the scroll position.
#[must_use]
pub fn cursor_of(cursor: amx_vt::Cursor) -> Cursor {
    Cursor {
        row: cursor.y,
        col: cursor.x,
        visible: cursor.visible && cursor.visible_in_viewport,
        shape: match cursor.style {
            // A hollow block is a focus hint the client draws for itself; on the
            // wire it is the block the application asked for.
            CursorStyle::Block | CursorStyle::BlockHollow => CursorShape::Block,
            CursorStyle::Underline => CursorShape::Underline,
            CursorStyle::Bar => CursorShape::Bar,
        },
        blink: cursor.blinking,
    }
}

/// Append a cursor.
pub fn put_cursor(cursor: Cursor, out: &mut Vec<u8>) {
    out.extend_from_slice(&cursor.row.to_le_bytes());
    out.extend_from_slice(&cursor.col.to_le_bytes());
    out.push(u8::from(cursor.visible) * CURSOR_VISIBLE + u8::from(cursor.blink) * CURSOR_BLINK);
    out.push(match cursor.shape {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
    });
}

/// Append a damage rect.
pub fn put_rect(rect: DamageRect, out: &mut Vec<u8>) {
    out.extend_from_slice(&rect.row.to_le_bytes());
    out.extend_from_slice(&rect.col.to_le_bytes());
    out.extend_from_slice(&rect.rows.to_le_bytes());
    out.extend_from_slice(&rect.cols.to_le_bytes());
}

/// A cursor over a payload that never reads past its end.
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// Read `bytes` from the start.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Bytes not yet read.
    #[must_use]
    pub const fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    /// Whether the payload is exhausted.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.left() == 0
    }

    /// Take `n` bytes.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] if fewer than `n` remain.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let end = self.at.checked_add(n).ok_or(CodecError::Truncated {
            wanted: n,
            left: self.left(),
        })?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated {
            wanted: n,
            left: self.left(),
        })?;
        self.at = end;
        Ok(slice)
    }

    /// Read one byte.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] at the end of the payload.
    pub fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    /// Read a little-endian `u16`.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] at the end of the payload.
    pub fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_le_bytes(fixed(self.take(2)?)))
    }

    /// Read a little-endian `u32`.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] at the end of the payload.
    pub fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(fixed(self.take(4)?)))
    }

    /// Read a little-endian `u64`.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] at the end of the payload.
    pub fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(fixed(self.take(8)?)))
    }

    /// Read a cursor.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] if the payload ends inside it.
    pub fn cursor(&mut self) -> Result<Cursor, CodecError> {
        let row = self.u16()?;
        let col = self.u16()?;
        let flags = self.u8()?;
        let shape = self.u8()?;
        Ok(Cursor {
            row,
            col,
            visible: flags & CURSOR_VISIBLE != 0,
            blink: flags & CURSOR_BLINK != 0,
            shape: match shape {
                1 => CursorShape::Underline,
                2 => CursorShape::Bar,
                // Anything else is a shape this build does not draw; a block is
                // the DECSCUSR reset value and the safe fallback.
                _ => CursorShape::Block,
            },
        })
    }

    /// Read a damage rect.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] if the payload ends inside it.
    pub fn rect(&mut self) -> Result<DamageRect, CodecError> {
        Ok(DamageRect {
            row: self.u16()?,
            col: self.u16()?,
            rows: self.u16()?,
            cols: self.u16()?,
        })
    }
}

/// Widen a slice the caller just proved is exactly `N` bytes long.
fn fixed<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [0_u8; N];
    out.copy_from_slice(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_payload_is_truncated_not_a_panic() {
        let mut reader = Reader::new(&[1, 2, 3]);
        assert_eq!(reader.u16(), Ok(0x0201));
        assert!(matches!(reader.u32(), Err(CodecError::Truncated { .. })));
    }

    #[test]
    fn a_cursor_round_trips() {
        let cursor = Cursor {
            row: 7,
            col: 42,
            visible: true,
            shape: CursorShape::Bar,
            blink: true,
        };
        let mut bytes = Vec::new();
        put_cursor(cursor, &mut bytes);
        assert_eq!(bytes.len(), CURSOR_BYTES);
        assert_eq!(Reader::new(&bytes).cursor(), Ok(cursor));
    }

    #[test]
    fn a_rect_round_trips() {
        let rect = DamageRect {
            row: 3,
            col: 0,
            rows: 9,
            cols: 80,
        };
        let mut bytes = Vec::new();
        put_rect(rect, &mut bytes);
        assert_eq!(bytes.len(), RECT_BYTES);
        assert_eq!(Reader::new(&bytes).rect(), Ok(rect));
    }
}
