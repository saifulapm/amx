//! Chrome: borders and the one status line the client draws itself (04 §3).
//!
//! Everything here is presentation the client invents locally — none of it
//! is server state — which is exactly the split 04 §3 draws: "draws its own
//! chrome (borders, status line, picker)".

use std::ops::Range;

use amx_core::Rect;
use unicode_width::UnicodeWidthChar;

use crate::model::{Attrs, Cell};
use crate::render::FrameWriter;

const TOP_LEFT: char = '┌';
const TOP_RIGHT: char = '┐';
const BOTTOM_LEFT: char = '└';
const BOTTOM_RIGHT: char = '┘';
const HORIZONTAL: char = '─';
const VERTICAL: char = '│';

/// Draw a one-cell border around `rect`. `rect` must be at least 2x2; smaller
/// rects (a pane squeezed to nothing by an extreme split) draw nothing.
pub fn draw_border(writer: &mut FrameWriter, rect: Rect) {
    if rect.w < 2 || rect.h < 2 {
        return;
    }
    let line = Cell {
        ch: HORIZONTAL,
        attrs: Attrs::default(),
    };
    let side = Cell {
        ch: VERTICAL,
        attrs: Attrs::default(),
    };

    writer.move_to(rect.y, rect.x);
    writer.write_cell(&corner(TOP_LEFT));
    for _ in 1..rect.w - 1 {
        writer.write_cell(&line);
    }
    writer.write_cell(&corner(TOP_RIGHT));

    for row in rect.y + 1..rect.y + rect.h - 1 {
        writer.move_to(row, rect.x);
        writer.write_cell(&side);
        writer.move_to(row, rect.x + rect.w - 1);
        writer.write_cell(&side);
    }

    writer.move_to(rect.y + rect.h - 1, rect.x);
    writer.write_cell(&corner(BOTTOM_LEFT));
    for _ in 1..rect.w - 1 {
        writer.write_cell(&line);
    }
    writer.write_cell(&corner(BOTTOM_RIGHT));
}

fn corner(ch: char) -> Cell {
    Cell {
        ch,
        attrs: Attrs::default(),
    }
}

/// The interior a border leaves inside `rect`: shrunk by one cell on every
/// side, clamped to empty rather than going negative.
#[must_use]
pub fn inset(rect: Rect) -> Rect {
    if rect.w < 2 || rect.h < 2 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    Rect::new(rect.x + 1, rect.y + 1, rect.w - 2, rect.h - 2)
}

/// Draw `text` left-aligned on one full-width row, truncated or blank-padded
/// to exactly `cols` cells, with the bytes in `emphasis` drawn out of the bar.
///
/// The whole row is reverse video, so a run drawn *without* it is what stands
/// out — which is how the status line marks the workspace this client is
/// showing (D15 asks for the active workspace to be highlighted). Emphasis by
/// attribute rather than by a marker glyph, because the one surface that most
/// needs the distinction is the one with the least width to spend on it: D14's
/// compact form on a phone.
///
/// `emphasis` is a byte range into `text`; an empty or out-of-range one draws
/// the row uniformly, which is what a line with no active workspace wants.
pub fn status_line(
    writer: &mut FrameWriter,
    row: u16,
    cols: u16,
    text: &str,
    emphasis: Range<usize>,
) {
    let bar = Attrs {
        reverse: true,
        ..Attrs::default()
    };
    writer.move_to(row, 0);
    let mut written = 0_u16;
    for (at, ch) in text.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if written + w > cols {
            break;
        }
        let attrs = if emphasis.contains(&at) {
            Attrs::default()
        } else {
            bar
        };
        writer.write_cell(&Cell { ch, attrs });
        written += w;
    }
    let pad = Cell {
        ch: ' ',
        attrs: bar,
    };
    for _ in written..cols {
        writer.write_cell(&pad);
    }
}
