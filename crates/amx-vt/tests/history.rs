#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! Scrollback reads and viewport addressing — the minimal surface T12 builds
//! row identity on top of.

use amx_vt::{Effects, Point, Scroll, Terminal, TerminalOptions};

fn pane(cols: u16, rows: u16, scrollback: usize) -> (Terminal, Effects) {
    let terminal = Terminal::new(TerminalOptions {
        cols,
        rows,
        max_scrollback: scrollback,
    })
    .expect("a terminal");
    (terminal, Effects::new())
}

/// Push `lines` numbered lines through the grid so most of them land in
/// scrollback.
fn fill(terminal: &mut Terminal, effects: &mut Effects, lines: usize) {
    for line in 0..lines {
        effects.clear();
        terminal.write(format!("line{line}\r\n").as_bytes(), effects);
    }
}

#[test]
fn scrollback_grows_as_rows_leave_the_active_area() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    assert_eq!(terminal.scrollback_rows().expect("rows"), 0);

    fill(&mut terminal, &mut effects, 10);

    assert!(terminal.scrollback_rows().expect("rows") > 0);
    assert_eq!(
        terminal.total_rows().expect("rows"),
        terminal.scrollback_rows().expect("rows") + 4,
    );
}

/// M0 plan R5 asks whether an arbitrary history range can be read "without
/// moving the live viewport". It can: a point tagged `History` resolves through
/// `ghostty_terminal_grid_ref` and never touches the viewport, so the user's
/// scroll position and everything the application can observe about it are
/// unchanged by the read.
#[test]
fn history_rows_read_without_moving_the_viewport() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    fill(&mut terminal, &mut effects, 10);

    let before = terminal.scrollbar().expect("a scrollbar");
    assert!(terminal.viewport_pinned().expect("pinned"));

    let mut text = String::new();
    let read = terminal
        .read_row(Point::history(0), &mut text)
        .expect("a history row");

    assert_eq!(text.trim_end(), "line0");
    assert_eq!(read.cells, 20, "the whole row width is read");
    assert!(!read.wrapped);
    assert_eq!(terminal.scrollbar().expect("a scrollbar"), before);
    assert!(
        terminal.viewport_pinned().expect("pinned"),
        "reading history left the viewport where it was"
    );
}

#[test]
fn consecutive_history_rows_read_in_order() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    fill(&mut terminal, &mut effects, 10);

    let mut lines = Vec::new();
    for y in 0..3 {
        let mut text = String::new();
        terminal
            .read_row(Point::history(y), &mut text)
            .expect("a history row");
        lines.push(text.trim_end().to_string());
    }

    assert_eq!(lines, ["line0", "line1", "line2"]);
}

#[test]
fn a_row_past_the_end_of_history_is_an_error() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    fill(&mut terminal, &mut effects, 10);

    let mut text = String::new();
    assert!(terminal.read_row(Point::history(9_999), &mut text).is_err());
}

#[test]
fn a_soft_wrapped_history_row_says_so() {
    let (mut terminal, mut effects) = pane(4, 3, 100);
    terminal.write(b"abcdefgh\r\n", &mut effects);
    fill(&mut terminal, &mut effects, 6);

    let mut text = String::new();
    let read = terminal
        .read_row(Point::history(0), &mut text)
        .expect("a history row");

    assert_eq!(text, "abcd");
    assert!(read.wrapped, "the line continues on the next row");
}

/// A wide character occupies two columns and one of them is a spacer, so a
/// naive read would emit an extra blank and shift the row.
#[test]
fn a_wide_character_reads_as_one_character() {
    let (mut terminal, mut effects) = pane(6, 3, 100);
    terminal.write("漢字ab".as_bytes(), &mut effects);

    let mut text = String::new();
    let read = terminal
        .read_row(Point::viewport(0), &mut text)
        .expect("a viewport row");

    assert_eq!(text, "漢字ab");
    assert_eq!(read.cells, 4, "six columns, four of them characters");
}

#[test]
fn scrolling_the_viewport_moves_it_and_bottom_brings_it_back() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    fill(&mut terminal, &mut effects, 10);
    let pinned = terminal.scrollbar().expect("a scrollbar");

    terminal.scroll_viewport(Scroll::Top);
    let top = terminal.scrollbar().expect("a scrollbar");
    assert_eq!(top.offset, 0);
    assert!(!terminal.viewport_pinned().expect("pinned"));

    terminal.scroll_viewport(Scroll::Bottom);
    assert_eq!(terminal.scrollbar().expect("a scrollbar"), pinned);
    assert!(terminal.viewport_pinned().expect("pinned"));
}

/// The scrollbar offset and `Scroll::Row` share a row space, so a position read
/// off one can be handed straight back to the other.
#[test]
fn an_absolute_row_scroll_round_trips_through_the_scrollbar() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    fill(&mut terminal, &mut effects, 10);

    terminal.scroll_viewport(Scroll::Row(2));
    let at_two = terminal.scrollbar().expect("a scrollbar");
    assert_eq!(at_two.offset, 2);

    terminal.scroll_viewport(Scroll::Top);
    terminal.scroll_viewport(Scroll::Row(usize::try_from(at_two.offset).expect("a row")));

    assert_eq!(terminal.scrollbar().expect("a scrollbar"), at_two);
}

#[test]
fn a_delta_scroll_moves_by_rows() {
    let (mut terminal, mut effects) = pane(20, 4, 100);
    fill(&mut terminal, &mut effects, 10);
    let pinned = terminal.scrollbar().expect("a scrollbar").offset;

    terminal.scroll_viewport(Scroll::Delta(-2));

    assert_eq!(
        terminal.scrollbar().expect("a scrollbar").offset,
        pinned - 2
    );
}
