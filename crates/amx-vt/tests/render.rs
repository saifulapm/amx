#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The render state: two-phase updates and the two layers of dirty tracking.

use amx_vt::{CellWide, Dirty, Effects, RenderState, Terminal, TerminalOptions, Underline};

struct Frame {
    terminal: Terminal,
    render: RenderState,
    effects: Effects,
}

impl Frame {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            terminal: Terminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: 100,
            })
            .expect("a terminal"),
            render: RenderState::new().expect("a render state"),
            effects: Effects::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.effects.clear();
        self.terminal.write(bytes, &mut self.effects);
    }

    fn update(&mut self) {
        self.render.update(&mut self.terminal).expect("an update");
    }

    /// Clear both layers of dirty state, which `render.h:65` puts on the
    /// caller: "setting one dirty state doesn't unset the other".
    fn clear_dirty(&mut self) {
        let mut rows = self.render.rows_iter().expect("rows");
        while let Some(mut row) = rows.next_row() {
            row.set_dirty(false).expect("a clear");
        }
        self.render.set_dirty(Dirty::Clean).expect("a clear");
    }

    fn dirty_rows(&mut self) -> Vec<u16> {
        let mut dirty = Vec::new();
        let mut index = 0;
        let mut rows = self.render.rows_iter().expect("rows");
        while let Some(row) = rows.next_row() {
            if row.dirty().expect("a dirty flag") {
                dirty.push(index);
            }
            index += 1;
        }
        dirty
    }

    fn row_text(&mut self, wanted: u16) -> String {
        let mut text = Vec::new();
        let mut index = 0;
        let mut rows = self.render.rows_iter().expect("rows");
        while let Some(mut row) = rows.next_row() {
            if index == wanted {
                let mut cells = row.cells().expect("cells");
                while let Some(cell) = cells.next_cell() {
                    cell.text(&mut text).expect("cell text");
                }
                break;
            }
            index += 1;
        }
        String::from_utf8(text).expect("utf8")
    }
}

#[test]
fn a_first_update_reports_a_fully_dirty_frame() {
    let mut frame = Frame::new(10, 4);
    frame.update();

    assert_eq!(frame.render.dirty().expect("dirty"), Dirty::Full);
    assert_eq!(frame.render.cols().expect("cols"), 10);
    assert_eq!(frame.render.rows().expect("rows"), 4);
}

/// The per-row layer is the whole reason damage is cheap: one changed row must
/// report as `Partial` plus exactly one dirty row, not as a full frame.
///
/// The cursor is parked on the row under test *before* the dirty state is
/// cleared, because moving the cursor dirties the row it leaves as well as the
/// one it arrives on — the old cell has to be repainted without it. That is
/// real damage, so the test isolates content damage rather than pretending it
/// does not happen.
#[test]
fn render_state_reports_partial_dirty_for_a_single_changed_row() {
    let mut frame = Frame::new(10, 4);
    frame.write(b"\x1b[3;1H");
    frame.update();
    frame.clear_dirty();
    assert_eq!(frame.render.dirty().expect("dirty"), Dirty::Clean);
    assert!(frame.dirty_rows().is_empty());

    frame.write(b"xy");
    frame.update();

    assert_eq!(frame.render.dirty().expect("dirty"), Dirty::Partial);
    assert_eq!(frame.dirty_rows(), [2]);
    assert!(frame.row_text(2).starts_with("xy"));
}

/// The two layers are independent, exactly as the header warns. Clearing the
/// global flag must leave the row flags alone — a wrapper that conflated them
/// would silently drop damage.
#[test]
fn clearing_the_global_dirty_state_leaves_the_row_flags_set() {
    let mut frame = Frame::new(10, 4);
    frame.write(b"\x1b[2;1H");
    frame.update();
    frame.clear_dirty();

    frame.write(b"z");
    frame.update();
    frame.render.set_dirty(Dirty::Clean).expect("a clear");

    assert_eq!(frame.render.dirty().expect("dirty"), Dirty::Clean);
    assert_eq!(frame.dirty_rows(), [1], "the row flag survives");
}

#[test]
fn the_two_phase_update_is_equivalent_to_the_convenience_call() {
    let mut frame = Frame::new(10, 4);
    frame.write(b"split");
    frame
        .render
        .begin_update(&mut frame.terminal)
        .expect("a begin");
    frame.render.end_update().expect("an end");

    assert_eq!(frame.render.dirty().expect("dirty"), Dirty::Full);
    assert!(frame.row_text(0).starts_with("split"));
}

#[test]
fn end_update_without_a_begin_is_a_no_op() {
    let mut frame = Frame::new(10, 4);
    frame.render.end_update().expect("a safe no-op");
}

#[test]
fn the_cursor_is_reported_in_viewport_coordinates() {
    let mut frame = Frame::new(10, 4);
    frame.write(b"\x1b[2;5H");
    frame.update();

    let cursor = frame.render.cursor().expect("a cursor");
    assert!(cursor.visible_in_viewport);
    assert_eq!((cursor.x, cursor.y), (4, 1));
    assert!(cursor.visible);
}

#[test]
fn a_hidden_cursor_reports_invisible() {
    let mut frame = Frame::new(10, 4);
    frame.write(b"\x1b[?25l");
    frame.update();

    assert!(!frame.render.cursor().expect("a cursor").visible);
}

#[test]
fn sgr_attributes_and_colors_reach_the_cell() {
    let mut frame = Frame::new(10, 2);
    // Bold, curly-underlined, red on blue.
    frame.write(b"\x1b[1;4:3;38;2;255;0;0;48;2;0;0;255mA");
    frame.update();

    let mut rows = frame.render.rows_iter().expect("rows");
    let mut row = rows.next_row().expect("a row");
    let mut cells = row.cells().expect("cells");
    let cell = cells.next_cell().expect("a cell");

    assert!(cell.has_styling().expect("styling"));
    let style = cell.style().expect("a style");
    assert!(style.bold);
    assert_eq!(style.underline, Underline::Curly);
    assert_eq!(
        cell.foreground().expect("fg").map(|c| (c.r, c.g, c.b)),
        Some((255, 0, 0))
    );
    assert_eq!(
        cell.background().expect("bg").map(|c| (c.r, c.g, c.b)),
        Some((0, 0, 255))
    );
    assert_eq!(cell.wide().expect("width"), CellWide::Narrow);
}

/// A wide character occupies two cells and the second must not be drawn; the
/// wrapper has to surface that, or every wide glyph doubles.
#[test]
fn a_wide_character_marks_its_second_cell_as_a_spacer() {
    let mut frame = Frame::new(10, 2);
    frame.write("漢".as_bytes());
    frame.update();

    let mut rows = frame.render.rows_iter().expect("rows");
    let mut row = rows.next_row().expect("a row");
    let mut cells = row.cells().expect("cells");

    let first = cells.next_cell().expect("a cell");
    assert_eq!(first.wide().expect("width"), CellWide::Wide);
    let second = cells.next_cell().expect("a cell");
    assert_eq!(second.wide().expect("width"), CellWide::SpacerTail);
}

#[test]
fn a_soft_wrapped_row_reports_itself_as_wrapped() {
    let mut frame = Frame::new(4, 3);
    frame.write(b"abcdef");
    frame.update();

    let mut rows = frame.render.rows_iter().expect("rows");
    let first = {
        let row = rows.next_row().expect("a row");
        row.wrapped().expect("a wrap flag")
    };
    let second = rows.next_row().expect("a row");

    assert!(first, "the first row continues onto the second");
    assert!(!second.wrapped().expect("a wrap flag"));
}

#[test]
fn the_frame_default_colors_are_readable() {
    let mut frame = Frame::new(10, 2);
    frame.update();

    // No assertion on the values themselves: they are libghostty's defaults,
    // not amx's. What matters is that the sized-struct call succeeds.
    let colors = frame.render.colors().expect("colors");
    assert_eq!(colors.cursor, None);
}
