//! How one cell is packed on the grid stream.
//!
//! [`Cells`](amx_proto::stream::Cells) is deliberately opaque — "the packed
//! layout is defined by the codec, not by this type" — so the layout lives
//! here, next to the encoder that produces it, in the same hand-rolled
//! little-endian style as the frame header and
//! [`history::pack`](crate::history::pack). Readable in a hex dump is the point.
//!
//! ```text
//! cell := u8  flags
//!         bits 0-1  width: 0 narrow, 1 wide, 2 spacer tail, 3 spacer head
//!         bit  2    styled          → a u16 style word follows
//!         bit  3    foreground set  → three bytes follow
//!         bit  4    background set  → three bytes follow
//!         bit  5    underline colour set → three bytes follow
//!         bits 6-7  reserved, zero
//!         [u16 style]  bit 0 bold, 1 italic, 2 faint, 3 blink, 4 inverse,
//!                      5 invisible, 6 strikethrough, 7 overline,
//!                      bits 8-10 underline style
//!         [u8;3] foreground, [u8;3] background, [u8;3] underline colour
//!         u8  text length, or 0xff followed by a u16 length
//!         text bytes (UTF-8, one grapheme cluster; empty for a blank cell)
//! ```
//!
//! Colours are presence-flagged rather than sent as a sentinel because
//! `None` means "the frame default", which the client resolves against its own
//! palette; a sentinel RGB would make the default unrepresentable. The text
//! length escapes to 16 bits because a grapheme cluster has no small bound —
//! [`TextRef`](amx_vt::TextRef) already carries a `u16` — while the
//! overwhelmingly common cell is one ASCII byte and pays one length byte.

use amx_vt::{Cell, CellWide, Rgb, Row, Style, Underline};

use super::codec::{CodecError, Reader};

/// Bits 0-1 of the flags byte.
const WIDTH_MASK: u8 = 0b0000_0011;
/// A style word follows.
const FLAG_STYLED: u8 = 1 << 2;
/// A foreground colour follows.
const FLAG_FOREGROUND: u8 = 1 << 3;
/// A background colour follows.
const FLAG_BACKGROUND: u8 = 1 << 4;
/// An underline colour follows.
const FLAG_UNDERLINE_COLOR: u8 = 1 << 5;
/// Bits no version of this layout has defined yet.
const FLAG_RESERVED: u8 = 0b1100_0000;

/// A text length of 0xff means the real length follows as a `u16`.
const TEXT_ESCAPE: u8 = u8::MAX;

/// Bit positions of the boolean attributes inside the style word.
const STYLE_BITS: [fn(&Style) -> bool; 8] = [
    |style| style.bold,
    |style| style.italic,
    |style| style.faint,
    |style| style.blink,
    |style| style.inverse,
    |style| style.invisible,
    |style| style.strikethrough,
    |style| style.overline,
];

/// Where the underline style sits in the style word.
const UNDERLINE_SHIFT: u32 = 8;
/// How wide the underline style field is.
const UNDERLINE_MASK: u16 = 0b111;

/// One decoded cell, owned.
///
/// The encoder never builds one of these — it writes straight out of the
/// published snapshot — so this type exists for the receiving side: the client,
/// and the replay that
/// `no_client_grid_is_corrupted_after_coalescing` compares against the server's
/// own grid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackedCell {
    /// The grapheme cluster, empty for a blank cell.
    pub text: String,
    /// Narrow, wide, or a spacer.
    pub wide: CellWide,
    /// Resolved foreground; `None` is the frame default.
    pub foreground: Option<Rgb>,
    /// Resolved background; `None` is the frame default.
    pub background: Option<Rgb>,
    /// SGR attributes.
    pub style: Style,
}

impl PackedCell {
    /// The cell a snapshot holds, in the form the wire carries it.
    ///
    /// The reference point for cell-for-cell comparison: whatever this returns
    /// for the server's grid is exactly what a client replaying the stream must
    /// end up holding.
    #[must_use]
    pub fn of(row: &Row, cell: &Cell) -> Self {
        Self {
            text: String::from_utf8_lossy(row.text(cell)).into_owned(),
            wide: cell.wide,
            foreground: cell.foreground,
            background: cell.background,
            style: cell.style,
        }
    }
}

/// Append one cell of `row` to `out`.
///
/// Appends only: the caller owns `out` and reuses it across frames, so encoding
/// a cell allocates exactly as often as the buffer needs to grow, which after
/// the first few frames is never.
pub fn encode(row: &Row, cell: &Cell, out: &mut Vec<u8>) {
    let styled = cell.style != Style::default();
    let mut flags = width_code(cell.wide);
    if styled {
        flags |= FLAG_STYLED;
    }
    if cell.foreground.is_some() {
        flags |= FLAG_FOREGROUND;
    }
    if cell.background.is_some() {
        flags |= FLAG_BACKGROUND;
    }
    if cell.style.underline_color.is_some() {
        flags |= FLAG_UNDERLINE_COLOR;
    }
    out.push(flags);

    if styled {
        out.extend_from_slice(&style_word(&cell.style).to_le_bytes());
    }
    for rgb in [cell.foreground, cell.background, cell.style.underline_color]
        .into_iter()
        .flatten()
    {
        out.extend_from_slice(&[rgb.r, rgb.g, rgb.b]);
    }

    let text = row.text(cell);
    // A grapheme cluster is bounded by the snapshot's own `u16` text length.
    let len = u16::try_from(text.len()).unwrap_or(u16::MAX);
    if len < u16::from(TEXT_ESCAPE) {
        // The `as` narrowing is exact: the branch proves the value fits.
        #[allow(clippy::cast_possible_truncation, reason = "len < 0xff on this arm")]
        out.push(len as u8);
    } else {
        out.push(TEXT_ESCAPE);
        out.extend_from_slice(&len.to_le_bytes());
    }
    out.extend_from_slice(&text[..usize::from(len)]);
}

/// Read one cell out of `reader`.
///
/// # Errors
///
/// [`CodecError::Truncated`] if the payload ends inside the cell,
/// [`CodecError::Reserved`] if it sets a flag bit this version does not define,
/// and [`CodecError::BadText`] if the cell's bytes are not UTF-8.
pub fn decode(reader: &mut Reader<'_>) -> Result<PackedCell, CodecError> {
    let flags = reader.u8()?;
    if flags & FLAG_RESERVED != 0 {
        return Err(CodecError::Reserved { flags });
    }
    let wide = width_of(flags & WIDTH_MASK);
    let word = if flags & FLAG_STYLED == 0 {
        0
    } else {
        reader.u16()?
    };
    let foreground = read_color(reader, flags & FLAG_FOREGROUND != 0)?;
    let background = read_color(reader, flags & FLAG_BACKGROUND != 0)?;
    let underline_color = read_color(reader, flags & FLAG_UNDERLINE_COLOR != 0)?;

    let short = reader.u8()?;
    let len = if short == TEXT_ESCAPE {
        reader.u16()?
    } else {
        u16::from(short)
    };
    let text = reader.take(usize::from(len))?;

    Ok(PackedCell {
        text: std::str::from_utf8(text)
            .map_err(|_| CodecError::BadText)?
            .to_owned(),
        wide,
        foreground,
        background,
        style: style_of(word, underline_color),
    })
}

fn read_color(reader: &mut Reader<'_>, present: bool) -> Result<Option<Rgb>, CodecError> {
    if !present {
        return Ok(None);
    }
    let bytes = reader.take(3)?;
    Ok(Some(Rgb {
        r: bytes[0],
        g: bytes[1],
        b: bytes[2],
    }))
}

/// The two-bit width code. Exhaustive, so a new `CellWide` variant is a compile
/// error here rather than a silently mis-encoded cell.
const fn width_code(wide: CellWide) -> u8 {
    match wide {
        CellWide::Narrow => 0,
        CellWide::Wide => 1,
        CellWide::SpacerTail => 2,
        CellWide::SpacerHead => 3,
    }
}

const fn width_of(code: u8) -> CellWide {
    match code {
        1 => CellWide::Wide,
        2 => CellWide::SpacerTail,
        3 => CellWide::SpacerHead,
        // Only 0 remains: `code` is masked to two bits by the caller.
        _ => CellWide::Narrow,
    }
}

/// Pack the attributes into the style word.
fn style_word(style: &Style) -> u16 {
    let mut word = 0_u16;
    for (bit, read) in STYLE_BITS.iter().enumerate() {
        if read(style) {
            word |= 1 << bit;
        }
    }
    word | (underline_code(style.underline) << UNDERLINE_SHIFT)
}

fn style_of(word: u16, underline_color: Option<Rgb>) -> Style {
    Style {
        bold: word & 1 != 0,
        italic: word & (1 << 1) != 0,
        faint: word & (1 << 2) != 0,
        blink: word & (1 << 3) != 0,
        inverse: word & (1 << 4) != 0,
        invisible: word & (1 << 5) != 0,
        strikethrough: word & (1 << 6) != 0,
        overline: word & (1 << 7) != 0,
        underline: underline_of((word >> UNDERLINE_SHIFT) & UNDERLINE_MASK),
        underline_color,
    }
}

const fn underline_code(underline: Underline) -> u16 {
    match underline {
        Underline::None => 0,
        Underline::Single => 1,
        Underline::Double => 2,
        Underline::Curly => 3,
        Underline::Dotted => 4,
        Underline::Dashed => 5,
    }
}

const fn underline_of(code: u16) -> Underline {
    match code {
        1 => Underline::Single,
        2 => Underline::Double,
        3 => Underline::Curly,
        4 => Underline::Dotted,
        5 => Underline::Dashed,
        // 0 and the two unassigned codes: an unknown underline draws as none
        // rather than failing the frame, since it is decoration.
        _ => Underline::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_underline_style_round_trips() {
        for underline in Underline::ALL {
            let code = underline_code(*underline);
            assert_eq!(underline_of(code), *underline);
        }
    }

    #[test]
    fn every_width_round_trips() {
        for wide in CellWide::ALL {
            let code = width_code(*wide);
            assert!(code & !WIDTH_MASK == 0, "the width code is two bits");
            assert_eq!(width_of(code), *wide);
        }
    }

    #[test]
    fn every_attribute_survives_the_style_word() {
        let style = Style {
            bold: true,
            italic: true,
            faint: true,
            blink: true,
            inverse: true,
            invisible: true,
            strikethrough: true,
            overline: true,
            underline: Underline::Curly,
            underline_color: Some(Rgb { r: 1, g: 2, b: 3 }),
        };
        let word = style_word(&style);
        assert_eq!(style_of(word, style.underline_color), style);
    }
}
