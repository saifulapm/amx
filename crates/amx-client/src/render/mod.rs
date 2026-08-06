//! Direct-ANSI rendering into a reused byte buffer (D-M0-4).
//!
//! No `ratatui`: the pane grid path is "take server cells with their exact
//! SGR attributes and emit them", and `ratatui` would cost a per-frame
//! `Buffer`/`Style` conversion of every visible cell to buy widgets this
//! client does not use. [`FrameWriter`] is that instead — a `Vec<u8>` kept
//! across frames plus an SGR-state differ, so an unchanged run of cells costs
//! only their bytes, never a fresh escape sequence.

pub mod chrome;
pub mod grid;

use std::io::Write as _;

use crate::model::{Attrs, Cell, Color};

/// A reused output buffer with an SGR-state differ.
///
/// [`begin_frame`](Self::begin_frame) clears the buffer (keeping its
/// capacity) and forgets the terminal's SGR state, since a full repaint may
/// follow an escape sequence this writer did not itself emit (a resize, an
/// external clear). Every write after that is diffed against what this
/// writer believes the terminal's current attributes are, so a run of cells
/// sharing one style pays for the escape sequence exactly once.
#[derive(Debug, Default)]
pub struct FrameWriter {
    buf: Vec<u8>,
    sgr: Attrs,
    sgr_valid: bool,
}

impl FrameWriter {
    /// An empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a fresh frame: clear the buffer, keep its capacity, and forget
    /// the SGR state so the first styled cell re-establishes it explicitly.
    pub fn begin_frame(&mut self) {
        self.buf.clear();
        self.sgr_valid = false;
    }

    /// The bytes written so far this frame.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Move the cursor to `(row, col)`, both 0-based.
    pub fn move_to(&mut self, row: u16, col: u16) {
        let _ = write!(self.buf, "\x1b[{};{}H", row + 1, col + 1);
    }

    /// Hide or show the cursor.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.buf
            .extend_from_slice(if visible { b"\x1b[?25h" } else { b"\x1b[?25l" });
    }

    /// Write raw bytes verbatim (control sequences, chrome glyphs).
    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Write one cell, changing SGR state only if it differs from the last
    /// cell written.
    pub fn write_cell(&mut self, cell: &Cell) {
        self.set_attrs(cell.attrs);
        let mut encoded = [0_u8; 4];
        self.buf
            .extend_from_slice(cell.ch.encode_utf8(&mut encoded).as_bytes());
    }

    fn set_attrs(&mut self, attrs: Attrs) {
        if self.sgr_valid && self.sgr == attrs {
            return;
        }
        self.buf.extend_from_slice(b"\x1b[0m");
        if attrs.bold {
            self.buf.extend_from_slice(b"\x1b[1m");
        }
        if attrs.italic {
            self.buf.extend_from_slice(b"\x1b[3m");
        }
        if attrs.underline {
            self.buf.extend_from_slice(b"\x1b[4m");
        }
        if attrs.reverse {
            self.buf.extend_from_slice(b"\x1b[7m");
        }
        write_color(&mut self.buf, attrs.fg, false);
        write_color(&mut self.buf, attrs.bg, true);
        self.sgr = attrs;
        self.sgr_valid = true;
    }
}

fn write_color(buf: &mut Vec<u8>, color: Color, background: bool) {
    let base = if background { 48 } else { 38 };
    match color {
        Color::Default => {}
        Color::Indexed(n) => {
            let _ = write!(buf, "\x1b[{base};5;{n}m");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(buf, "\x1b[{base};2;{r};{g};{b}m");
        }
    }
}
