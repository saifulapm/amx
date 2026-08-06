//! How one cell is packed on the grid stream.
//!
//! [`Cells`](crate::stream::Cells) is deliberately opaque — "the packed layout
//! is defined by the codec, not by this type" — so the layout lives here, in
//! the same hand-rolled little-endian style as the frame header. Readable in a
//! hex dump is the point.
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
//! Colours are presence-flagged rather than sent as a sentinel because `None`
//! means "the frame default", which the client resolves against its own
//! palette; a sentinel RGB would make the default unrepresentable. The text
//! length escapes to 16 bits because a grapheme cluster has no small bound,
//! while the overwhelmingly common cell is one ASCII byte and pays one length
//! byte.
//!
//! The types here are the *wire's* vocabulary, not any terminal library's: the
//! server maps its terminal cells into a [`CellRef`] at encode time (a field
//! copy, no allocation), and the client reads [`PackedCell`]s out. `amx-proto`
//! depends on `amx-core` alone, so this is the one cell model both sides of
//! the socket can share.

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

/// Where the underline style sits in the style word.
const UNDERLINE_SHIFT: u32 = 8;
/// How wide the underline style field is.
const UNDERLINE_MASK: u16 = 0b111;

/// A direct colour, as the wire carries it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// A cell's width class.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CellWide {
    /// One column.
    #[default]
    Narrow,
    /// Two columns; the next cell is its spacer tail.
    Wide,
    /// The second column of a wide cell.
    SpacerTail,
    /// A spacer before a wide cell that would have straddled the margin.
    SpacerHead,
}

impl CellWide {
    /// Every width class, for exhaustive round-trip tests.
    pub const ALL: &'static [Self] =
        &[Self::Narrow, Self::Wide, Self::SpacerTail, Self::SpacerHead];
}

/// An underline style.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Underline {
    /// No underline.
    #[default]
    None,
    /// A single line.
    Single,
    /// A double line.
    Double,
    /// A curly line.
    Curly,
    /// A dotted line.
    Dotted,
    /// A dashed line.
    Dashed,
}

impl Underline {
    /// Every underline style, for exhaustive round-trip tests.
    pub const ALL: &'static [Self] = &[
        Self::None,
        Self::Single,
        Self::Double,
        Self::Curly,
        Self::Dotted,
        Self::Dashed,
    ];
}

/// The SGR attributes one cell carries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct CellStyle {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Faint.
    pub faint: bool,
    /// Blink.
    pub blink: bool,
    /// Inverse video.
    pub inverse: bool,
    /// Invisible.
    pub invisible: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// Overlined.
    pub overline: bool,
    /// Underline style.
    pub underline: Underline,
    /// Underline colour; `None` follows the foreground.
    pub underline_color: Option<Rgb>,
}

/// Bit positions of the boolean attributes inside the style word.
const STYLE_BITS: [fn(&CellStyle) -> bool; 8] = [
    |style| style.bold,
    |style| style.italic,
    |style| style.faint,
    |style| style.blink,
    |style| style.inverse,
    |style| style.invisible,
    |style| style.strikethrough,
    |style| style.overline,
];

/// One cell as the encoder sees it: borrowed text, copied attributes.
///
/// Borrowed because encoding runs on the server's hot path — the text bytes
/// come straight out of the published snapshot and are never copied before
/// they land in the output buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellRef<'a> {
    /// The grapheme cluster's UTF-8 bytes; empty for a blank cell.
    pub text: &'a [u8],
    /// Width class.
    pub wide: CellWide,
    /// Resolved foreground; `None` is the frame default.
    pub foreground: Option<Rgb>,
    /// Resolved background; `None` is the frame default.
    pub background: Option<Rgb>,
    /// SGR attributes.
    pub style: CellStyle,
}

/// One decoded cell, owned.
///
/// What the receiving side holds: the client's grid cache, and any replay that
/// compares an emitted stream against the grid it was read from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackedCell {
    /// The grapheme cluster, empty for a blank cell.
    pub text: String,
    /// Width class.
    pub wide: CellWide,
    /// Resolved foreground; `None` is the frame default.
    pub foreground: Option<Rgb>,
    /// Resolved background; `None` is the frame default.
    pub background: Option<Rgb>,
    /// SGR attributes.
    pub style: CellStyle,
}

/// Append one cell to `out`.
///
/// Appends only: the caller owns `out` and reuses it across frames, so
/// encoding a cell allocates exactly as often as the buffer needs to grow,
/// which after the first few frames is never.
pub fn encode(cell: &CellRef<'_>, out: &mut Vec<u8>) {
    let styled = cell.style != CellStyle::default();
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

    // A grapheme cluster is bounded by the snapshot's own `u16` text length.
    let len = u16::try_from(cell.text.len()).unwrap_or(u16::MAX);
    if len < u16::from(TEXT_ESCAPE) {
        // The `as` narrowing is exact: the branch proves the value fits.
        #[allow(clippy::cast_possible_truncation, reason = "len < 0xff on this arm")]
        out.push(len as u8);
    } else {
        out.push(TEXT_ESCAPE);
        out.extend_from_slice(&len.to_le_bytes());
    }
    out.extend_from_slice(&cell.text[..usize::from(len)]);
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
fn style_word(style: &CellStyle) -> u16 {
    let mut word = 0_u16;
    for (bit, read) in STYLE_BITS.iter().enumerate() {
        if read(style) {
            word |= 1 << bit;
        }
    }
    word | (underline_code(style.underline) << UNDERLINE_SHIFT)
}

fn style_of(word: u16, underline_color: Option<Rgb>) -> CellStyle {
    CellStyle {
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
        let style = CellStyle {
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

    #[test]
    fn a_full_cell_round_trips() {
        let cell = CellRef {
            text: "\u{4f60}".as_bytes(),
            wide: CellWide::Wide,
            foreground: Some(Rgb { r: 250, g: 0, b: 7 }),
            background: None,
            style: CellStyle {
                bold: true,
                ..CellStyle::default()
            },
        };
        let mut bytes = Vec::new();
        encode(&cell, &mut bytes);
        let decoded = decode(&mut Reader::new(&bytes)).expect("the cell decodes");
        assert_eq!(decoded.text, "\u{4f60}");
        assert_eq!(decoded.wide, CellWide::Wide);
        assert_eq!(decoded.foreground, cell.foreground);
        assert_eq!(decoded.background, None);
        assert!(decoded.style.bold);
    }
}
