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

use crate::model::{Attrs, Cell, Color, Underline};

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
    ///
    /// A zero char writes nothing: it marks a wide cell's spacer tail, whose
    /// column the wide glyph before it already advanced over — emitting even a
    /// space there would shove the rest of the row one column right.
    pub fn write_cell(&mut self, cell: &Cell) {
        if cell.ch == '\0' {
            return;
        }
        self.set_attrs(cell.attrs);
        let mut encoded = [0_u8; 4];
        self.buf
            .extend_from_slice(cell.ch.encode_utf8(&mut encoded).as_bytes());
    }

    /// Put the terminal into `attrs`: a full reset, then every attribute the
    /// cell wears.
    ///
    /// Always the whole paint rather than a difference from the last one, for
    /// the reason the handoff's own pen has
    /// (`amx-server/src/handoff/grid/mod.rs:383-386`): a run that says only
    /// what changed goes wrong the first time an attribute has no reset code.
    /// The differ is the equality check above it — an unchanged run of cells
    /// emits nothing at all.
    fn set_attrs(&mut self, attrs: Attrs) {
        if self.sgr_valid && self.sgr == attrs {
            return;
        }
        self.buf.extend_from_slice(b"\x1b[0m");
        for (on, sequence) in [
            (attrs.bold, &b"\x1b[1m"[..]),
            (attrs.faint, b"\x1b[2m"),
            (attrs.italic, b"\x1b[3m"),
            (attrs.blink, b"\x1b[5m"),
            (attrs.reverse, b"\x1b[7m"),
            (attrs.invisible, b"\x1b[8m"),
            (attrs.strikethrough, b"\x1b[9m"),
            (attrs.overline, b"\x1b[53m"),
        ] {
            if on {
                self.buf.extend_from_slice(sequence);
            }
        }
        // `4` is the one underline every terminal has always understood and is
        // what this writer has always emitted for it; the four shapes that have
        // no single-parameter spelling take the sub-parameter form, which is
        // the grammar the vendored parser reads for every shape
        // (`vendor/libghostty-vt/src/terminal/sgr.zig:269-285`). Double is `4:2`
        // here where the handoff's pen writes `21`: the pen paints into a
        // libghostty-vt terminal, whose reading of `21` is known
        // (`sgr.zig:301`), and this writer paints into whatever the user
        // attached from, where one grammar for the four is the narrower claim.
        self.buf.extend_from_slice(match attrs.underline {
            Underline::None => &b""[..],
            Underline::Single => b"\x1b[4m",
            Underline::Double => b"\x1b[4:2m",
            Underline::Curly => b"\x1b[4:3m",
            Underline::Dotted => b"\x1b[4:4m",
            Underline::Dashed => b"\x1b[4:5m",
        });
        write_color(&mut self.buf, attrs.fg, FOREGROUND);
        write_color(&mut self.buf, attrs.bg, BACKGROUND);
        write_color(&mut self.buf, attrs.underline_color, UNDERLINE);
        self.sgr = attrs;
        self.sgr_valid = true;
    }
}

/// The SGR parameter that introduces a foreground colour.
const FOREGROUND: u16 = 38;
/// The SGR parameter that introduces a background colour.
const BACKGROUND: u16 = 48;
/// The SGR parameter that introduces an underline colour — the same
/// direct-colour grammar as the other two, which is how the vendored parser
/// reads it (`vendor/libghostty-vt/src/terminal/sgr.zig:388-413`).
const UNDERLINE: u16 = 58;

fn write_color(buf: &mut Vec<u8>, color: Color, base: u16) {
    match color {
        // Absent: the reset already put this colour back to the terminal's
        // default, which for the underline means "follow the foreground".
        Color::Default => {}
        Color::Indexed(n) => {
            let _ = write!(buf, "\x1b[{base};5;{n}m");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(buf, "\x1b[{base};2;{r};{g};{b}m");
        }
    }
}
