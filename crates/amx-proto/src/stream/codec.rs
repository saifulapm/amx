//! The binary streams' shared byte plumbing: a bounded reader and the small
//! fixed shapes (cursor, rect) every stream message uses.
//!
//! Hand-rolled little-endian, like the frame header, so the bytes stay legible
//! in a dump and in a golden. This layout began life next to the server's
//! damage encoder; it lives here now so the server and the client decode one
//! implementation instead of two.

use bytes::BufMut;
use thiserror::Error;

use crate::stream::grid::{Cursor, CursorShape, DamageRect};

/// Bytes one rect occupies.
pub const RECT_BYTES: usize = 8;
/// Bytes one cursor occupies.
pub const CURSOR_BYTES: usize = 6;

/// The cursor is drawn.
const CURSOR_VISIBLE: u8 = 1 << 0;
/// The cursor blinks.
const CURSOR_BLINK: u8 = 1 << 1;

/// A stream payload could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum CodecError {
    /// The payload ended before the message did.
    #[error("stream message truncated: wanted {wanted} more bytes, {left} left")]
    Truncated {
        /// Bytes the next field needs.
        wanted: usize,
        /// Bytes still in the payload.
        left: usize,
    },
    /// The tag byte names a message this build does not know.
    #[error("unknown stream message tag {tag}")]
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
    /// A cell's or row's text was not UTF-8.
    #[error("text is not utf-8")]
    BadText,
    /// The message declared more cell bytes than it carried, or fewer than its
    /// rects need.
    #[error("cell payload does not match the rects it belongs to")]
    CellMismatch,
    /// A raw I/O direction byte named neither direction.
    #[error("unknown raw i/o direction {byte}")]
    UnknownDirection {
        /// The byte that arrived.
        byte: u8,
    },
    /// A history chunk's request id carried an unknown kind byte.
    #[error("unknown request id kind {byte}")]
    UnknownRequestKind {
        /// The byte that arrived.
        byte: u8,
    },
    /// A declared count could not possibly fit the remaining payload.
    ///
    /// Checked before any buffer is sized from the count: a peer declaring a
    /// billion rows costs a rejection, not a billion-slot allocation.
    #[error("declared count {count} cannot fit in the {left} bytes remaining")]
    CountTooLarge {
        /// The declared element count.
        count: usize,
        /// Bytes still in the payload.
        left: usize,
    },
}

/// A cursor position on the wire.
///
/// Distinct from any terminal library's cursor: this is the six-byte shape the
/// grid stream carries, nothing more.
pub fn put_cursor(cursor: Cursor, out: &mut impl BufMut) {
    out.put_u16_le(cursor.row);
    out.put_u16_le(cursor.col);
    out.put_u8(u8::from(cursor.visible) * CURSOR_VISIBLE + u8::from(cursor.blink) * CURSOR_BLINK);
    out.put_u8(match cursor.shape {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
    });
}

/// Append a damage rect.
pub fn put_rect(rect: DamageRect, out: &mut impl BufMut) {
    out.put_u16_le(rect.row);
    out.put_u16_le(rect.col);
    out.put_u16_le(rect.rows);
    out.put_u16_le(rect.cols);
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

    /// Whether `count` elements of at least `min_bytes` each could still fit.
    ///
    /// The guard every `Vec::with_capacity` sized from a wire-declared count
    /// must pass first: a count the remaining payload cannot possibly hold is
    /// refused before a single slot is allocated for it.
    ///
    /// # Errors
    ///
    /// [`CodecError::CountTooLarge`] when it cannot.
    pub fn admits(&self, count: usize, min_bytes: usize) -> Result<(), CodecError> {
        let wanted = count.checked_mul(min_bytes.max(1));
        if wanted.is_none_or(|wanted| wanted > self.left()) {
            return Err(CodecError::CountTooLarge {
                count,
                left: self.left(),
            });
        }
        Ok(())
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

    #[test]
    fn an_impossible_count_is_refused_before_allocation() {
        let reader = Reader::new(&[0; 16]);
        assert!(reader.admits(2, 8).is_ok());
        assert_eq!(
            reader.admits(3, 8),
            Err(CodecError::CountTooLarge { count: 3, left: 16 })
        );
        assert_eq!(
            reader.admits(usize::MAX, 8),
            Err(CodecError::CountTooLarge {
                count: usize::MAX,
                left: 16
            })
        );
    }
}
