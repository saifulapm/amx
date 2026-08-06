#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! Scrollback reads, viewport addressing and row anchors — the surface amx's
//! row identity is built on, and the traps the T12 spike found in it
//! (`docs/notes/scrollback-identity.md`).

use amx_vt::{Effects, Point, PointSpace, Scroll, Terminal, TerminalOptions};

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

/// A viewport parked in history stays parked while history is read: the read
/// path resolves points, and resolving a point mutates nothing (R5).
#[test]
fn a_history_read_leaves_a_scrolled_viewport_alone() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    fill(&mut terminal, &mut effects, 50);
    terminal.scroll_viewport(Scroll::Row(3));
    let parked = terminal.scrollbar().expect("a scrollbar");

    let mut text = String::new();
    for y in 0..40 {
        text.clear();
        terminal
            .read_row(Point::history(y), &mut text)
            .expect("row");
    }

    assert_eq!(terminal.scrollbar().expect("a scrollbar"), parked);
    assert!(!terminal.viewport_pinned().expect("pinned"));
}

/// The `history` space does not stop at the active area, so a caller that does
/// not bound reads by [`Terminal::scrollback_rows`] serves *live* rows as
/// history. The library will not catch it; only a row past the whole screen is
/// an error.
#[test]
fn a_history_point_past_the_scrollback_reads_a_live_row() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    fill(&mut terminal, &mut effects, 10);
    let scrollback = u32::try_from(terminal.scrollback_rows().expect("rows")).expect("fits");

    let mut history = String::new();
    terminal
        .read_row(Point::history(scrollback), &mut history)
        .expect("a row past the end of history still reads");
    let mut live = String::new();
    terminal
        .read_row(Point::viewport(0), &mut live)
        .expect("a live row");

    assert_eq!(history, live, "history(scrollback) is the first live row");
}

/// The one primitive amx's row identity rests on: an anchor placed on a row
/// follows it into history and reports how far down it is, so a caller that
/// knows the row's id learns exactly how many rows were discarded below it.
#[test]
fn an_anchor_follows_its_row_and_measures_what_was_discarded() {
    let (mut terminal, mut effects) = pane(20, 4, 8192);
    fill(&mut terminal, &mut effects, 200);
    let scrollback = terminal.scrollback_rows().expect("rows");
    let newest = u32::try_from(scrollback - 1).expect("fits");
    // Nothing has been pruned yet at this depth, so the newest history row's id
    // is its own offset.
    let anchor_id = u64::from(newest);
    let anchor = terminal.track(Point::history(newest)).expect("an anchor");
    assert!(anchor.alive());
    assert_eq!(
        anchor.row(PointSpace::History).expect("history"),
        Some(newest)
    );
    assert_eq!(anchor.row(PointSpace::Active).expect("active"), None);

    fill(&mut terminal, &mut effects, 4000);

    let offset = anchor
        .row(PointSpace::History)
        .expect("history")
        .expect("the anchor is still in history");
    let floor = anchor_id - u64::from(offset);
    let mut oldest = String::new();
    terminal
        .read_row(Point::history(0), &mut oldest)
        .expect("the oldest row");
    assert_eq!(
        oldest.trim_end(),
        format!("line{floor}"),
        "the derived floor names the row the scrollback actually starts at"
    );
}

/// The failure the model has to survive: when the whole scrollback turns over
/// between two observations the anchor is gone, and no count can be recovered.
#[test]
fn an_anchor_dies_when_the_whole_history_turns_over() {
    let (mut terminal, mut effects) = pane(20, 4, 8192);
    fill(&mut terminal, &mut effects, 50);
    let newest = u32::try_from(terminal.scrollback_rows().expect("rows") - 1).expect("fits");
    let anchor = terminal.track(Point::history(newest)).expect("an anchor");

    fill(&mut terminal, &mut effects, 20_000);

    assert!(!anchor.alive());
    assert_eq!(anchor.row(PointSpace::History).expect("history"), None);
}

/// Growing the grid pulls the newest history rows back into the live area. The
/// anchor follows them out of history and its history offset goes stale, so
/// `Active` is what says whether an anchor may be trusted as a floor probe.
#[test]
fn an_anchor_pulled_back_into_the_live_area_says_so() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    fill(&mut terminal, &mut effects, 20);
    let scrollback = terminal.scrollback_rows().expect("rows");
    let newest = u32::try_from(scrollback - 1).expect("fits");
    let anchor = terminal.track(Point::history(newest)).expect("an anchor");

    terminal.resize(20, 10).expect("a taller grid");

    let grown = terminal.scrollback_rows().expect("rows");
    assert!(grown < scrollback, "a taller grid reclaims history rows");
    assert!(anchor.alive());
    assert!(
        anchor.row(PointSpace::Active).expect("active").is_some(),
        "the anchored row is in the live area now"
    );
    assert!(
        anchor.row(PointSpace::History).expect("history")
            >= Some(u32::try_from(grown).expect("fits")),
        "its history offset is stale — it counts past the end of history"
    );
}

/// The trap that makes a content check mandatory: erasing the scrollback leaves
/// the anchor reporting a value, and the row it names is a different row.
#[test]
fn an_erased_scrollback_leaves_an_anchor_alive_and_wrong() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    fill(&mut terminal, &mut effects, 20);
    let newest = u32::try_from(terminal.scrollback_rows().expect("rows") - 1).expect("fits");
    let mut anchored = String::new();
    terminal
        .read_row(Point::history(newest), &mut anchored)
        .expect("the anchored row");
    let anchor = terminal.track(Point::history(newest)).expect("an anchor");

    terminal.write(b"\x1b[3J", &mut effects);

    assert_eq!(terminal.scrollback_rows().expect("rows"), 0);
    assert!(anchor.alive(), "the library keeps a discarded pin alive");
    let at = anchor
        .row(PointSpace::Active)
        .expect("active")
        .expect("it points somewhere in the live area");
    let mut now = String::new();
    terminal
        .read_row(
            Point {
                space: PointSpace::Active,
                y: at,
            },
            &mut now,
        )
        .expect("the row it points at");
    assert_ne!(
        now, anchored,
        "an alive anchor is not proof of identity — the row underneath changed"
    );
}

/// Only width changes reflow, and a reflow rewrites the *oldest* row too, which
/// is why invalidation starts at the eviction floor rather than at the tail.
#[test]
fn a_width_change_rewrites_history_from_its_oldest_row() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    for line in 0..20 {
        effects.clear();
        terminal.write(
            format!("line{line} padded out to twenty\r\n").as_bytes(),
            &mut effects,
        );
    }
    let before = terminal.scrollback_rows().expect("rows");
    let mut oldest = String::new();
    terminal
        .read_row(Point::history(0), &mut oldest)
        .expect("the oldest row");

    terminal.resize(10, 4).expect("a narrower grid");

    let mut reflowed = String::new();
    terminal
        .read_row(Point::history(0), &mut reflowed)
        .expect("the oldest row");
    assert!(terminal.scrollback_rows().expect("rows") > before);
    assert_ne!(reflowed, oldest);
}

/// A height change moves rows between history and the live area without
/// reflowing anything: the content that stays in history is untouched.
#[test]
fn a_height_change_does_not_rewrite_history() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    fill(&mut terminal, &mut effects, 20);
    let mut oldest = String::new();
    terminal
        .read_row(Point::history(0), &mut oldest)
        .expect("the oldest row");
    let before = terminal.scrollback_rows().expect("rows");

    terminal.resize(20, 10).expect("a taller grid");
    let grown = terminal.scrollback_rows().expect("rows");
    terminal.resize(20, 4).expect("a shorter grid");

    let mut after = String::new();
    terminal
        .read_row(Point::history(0), &mut after)
        .expect("the oldest row");
    assert!(grown < before, "growing reclaimed rows from history");
    assert_eq!(terminal.scrollback_rows().expect("rows"), before);
    assert_eq!(after, oldest);
}

/// Both scrollback counters are per *active screen*, so tracking has to freeze
/// while the alternate screen is up rather than read a wipe that never was.
#[test]
fn the_alternate_screen_reports_no_scrollback() {
    let (mut terminal, mut effects) = pane(20, 4, 4096);
    fill(&mut terminal, &mut effects, 20);
    let primary = terminal.scrollback_rows().expect("rows");

    terminal.write(b"\x1b[?1049h", &mut effects);
    assert_eq!(terminal.scrollback_rows().expect("rows"), 0);
    fill(&mut terminal, &mut effects, 20);
    assert_eq!(
        terminal.scrollback_rows().expect("rows"),
        0,
        "output on the alternate screen commits nothing to history"
    );

    terminal.write(b"\x1b[?1049l", &mut effects);
    assert_eq!(terminal.scrollback_rows().expect("rows"), primary);
}

/// `max_scrollback` is a memory bound, not a row count — the same limit holds
/// an order of magnitude fewer rows at ten times the width. Nothing about the
/// eviction floor may be derived from it.
#[test]
fn the_scrollback_limit_is_a_memory_bound_not_a_row_count() {
    let mut settled = Vec::new();
    for cols in [20u16, 200] {
        let (mut terminal, mut effects) = pane(cols, 4, 8192);
        fill(&mut terminal, &mut effects, 20_000);
        settled.push(terminal.scrollback_rows().expect("rows"));
    }

    assert_ne!(settled[0], 8192);
    assert!(
        settled[0] > settled[1] * 4,
        "a narrow pane keeps far more rows under the same limit: {settled:?}"
    );
}
