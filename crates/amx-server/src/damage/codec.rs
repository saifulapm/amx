//! Adapters from libghostty-vt's cell model to the wire's.
//!
//! The grid stream's byte layout lives in `amx-proto` (`stream::{codec, cell,
//! grid}`) so server and client decode one implementation. What remains here
//! is the seam `amx-proto` cannot own: mapping `amx-vt`'s snapshot types into
//! the wire vocabulary, one exhaustive match per enum so a re-vendor that
//! grows a variant is a compile error at the mapping site.

use amx_proto::stream::cell::{CellRef, CellStyle, CellWide, PackedCell, Rgb, Underline};
use amx_proto::stream::{Cursor, CursorShape};
use amx_vt::CursorStyle;

pub use amx_proto::stream::codec::{
    CURSOR_BYTES, CodecError, RECT_BYTES, Reader, put_cursor, put_rect,
};
pub use amx_proto::stream::grid::{TAG_CURSOR, TAG_DELTA, TAG_RESET};

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

/// One snapshot cell in the wire's borrowed form, ready to encode.
#[must_use]
pub fn wire_cell<'a>(row: &'a amx_vt::Row, cell: &'a amx_vt::Cell) -> CellRef<'a> {
    CellRef {
        text: row.text(cell),
        wide: wide_of(cell.wide),
        foreground: cell.foreground.map(rgb_of),
        background: cell.background.map(rgb_of),
        style: style_of(&cell.style),
    }
}

/// The cell a snapshot holds, in the owned form the wire decodes to.
///
/// The reference point for cell-for-cell comparison: whatever this returns for
/// the server's grid is exactly what a client replaying the stream must end up
/// holding.
#[must_use]
pub fn expected_cell(row: &amx_vt::Row, cell: &amx_vt::Cell) -> PackedCell {
    let wire = wire_cell(row, cell);
    PackedCell {
        text: String::from_utf8_lossy(wire.text).into_owned(),
        wide: wire.wide,
        foreground: wire.foreground,
        background: wire.background,
        style: wire.style,
    }
}

const fn rgb_of(rgb: amx_vt::Rgb) -> Rgb {
    Rgb {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
    }
}

const fn wide_of(wide: amx_vt::CellWide) -> CellWide {
    match wide {
        amx_vt::CellWide::Narrow => CellWide::Narrow,
        amx_vt::CellWide::Wide => CellWide::Wide,
        amx_vt::CellWide::SpacerTail => CellWide::SpacerTail,
        amx_vt::CellWide::SpacerHead => CellWide::SpacerHead,
    }
}

const fn underline_of(underline: amx_vt::Underline) -> Underline {
    match underline {
        amx_vt::Underline::None => Underline::None,
        amx_vt::Underline::Single => Underline::Single,
        amx_vt::Underline::Double => Underline::Double,
        amx_vt::Underline::Curly => Underline::Curly,
        amx_vt::Underline::Dotted => Underline::Dotted,
        amx_vt::Underline::Dashed => Underline::Dashed,
    }
}

fn style_of(style: &amx_vt::Style) -> CellStyle {
    CellStyle {
        bold: style.bold,
        italic: style.italic,
        faint: style.faint,
        blink: style.blink,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        overline: style.overline,
        underline: underline_of(style.underline),
        underline_color: style.underline_color.map(rgb_of),
    }
}
