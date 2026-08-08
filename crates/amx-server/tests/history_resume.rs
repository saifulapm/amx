#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! Row-id continuity across a live upgrade (D-M3-4).
//!
//! 04 §6: "scrollback row ids are continuous across handoff". The successor
//! rebuilds every pane around a *fresh* terminal, so a tracker that started
//! from zero there would hand out ids 0, 1, 2 for rows the exporter already
//! numbered differently — and a client's cached ranges, which the whole resync
//! exists to preserve, would silently address the wrong rows.
//!
//! Driven against a terminal directly, like `history.rs` next door and for the
//! same reason: every assertion here is about *which* row an id names. The two
//! trackers in each test stand for the two processes — one that filled its
//! terminal itself, and one handed the same rows and told where the id space
//! left off.

use amx_core::{RowId, RowRange};
use amx_server::history::{HistoryEvent, HistoryTracker};
use amx_vt::{Effects, Terminal, TerminalOptions};

/// A byte bound, not a row count, and the one `history.rs` measured: 8 KiB
/// holds a little over four thousand twenty-column rows. [`LINES`] is set past
/// it, because the resumed floor under test has to be a real eviction floor and
/// not zero.
const TRIMMED: usize = 8 * 1024;

/// Grid geometry, fixed: a width change would reflow and a height change would
/// reclaim rows, and neither is what these tests are about.
const COLS: u16 = 20;
const ROWS: u16 = 5;

/// How many numbered lines each terminal is given.
const LINES: usize = 6000;

/// Rows per observation on the exporter's side.
///
/// A pane observes at frame boundaries, and the anchor that carries row
/// identity across a trim is placed by an observation — so a fixture that wrote
/// everything and looked once would never move its floor off zero, and the
/// eviction this test needs would not have happened.
const PER_FRAME: usize = 100;

struct Pane {
    terminal: Terminal,
    effects: Effects,
    found: Vec<HistoryEvent>,
}

impl Pane {
    fn new() -> Self {
        let terminal = Terminal::new(TerminalOptions {
            cols: COLS,
            rows: ROWS,
            max_scrollback: TRIMMED,
        })
        .expect("a terminal");
        Self {
            terminal,
            effects: Effects::new(),
            found: Vec::new(),
        }
    }

    fn write_lines(&mut self, from: usize, lines: usize) {
        for line in from..from + lines {
            self.effects.clear();
            self.terminal
                .write(format!("L{line}\r\n").as_bytes(), &mut self.effects);
        }
    }

    /// Write `lines` lines a frame's worth at a time, as a running pane does.
    fn fill(&mut self, tracker: &mut HistoryTracker, from: usize, lines: usize) {
        let mut written = 0;
        while written < lines {
            let batch = PER_FRAME.min(lines - written);
            self.write_lines(from + written, batch);
            self.observe(tracker);
            written += batch;
        }
    }

    fn observe(&mut self, tracker: &mut HistoryTracker) -> Vec<HistoryEvent> {
        self.found.clear();
        tracker.observe(&mut self.terminal, &mut self.found);
        self.found.clone()
    }

    fn row_text(&mut self, tracker: &mut HistoryTracker, id: RowId) -> String {
        let served = tracker
            .read(&self.terminal, RowRange::single(id))
            .expect("a committed row");
        unpack(&served.rows).remove(0)
    }
}

/// Unpack the server's row packing: `u32` length, text, one flag byte.
fn unpack(bytes: &[u8]) -> Vec<String> {
    let mut rows = Vec::new();
    let mut at = 0;
    while at + 5 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
        at += 4;
        let text = String::from_utf8(bytes[at..at + len].to_vec()).expect("utf-8");
        at += len + 1;
        rows.push(text.trim_end().to_owned());
    }
    assert_eq!(at, bytes.len(), "the packing is self-delimiting");
    rows
}

fn committed(events: &[HistoryEvent]) -> Vec<RowRange> {
    events
        .iter()
        .filter_map(|event| match event {
            HistoryEvent::Committed(c) => Some(c.range),
            _ => None,
        })
        .collect()
}

fn invalidations(events: &[HistoryEvent]) -> Vec<&HistoryEvent> {
    events
        .iter()
        .filter(|event| matches!(event, HistoryEvent::Invalidated { .. }))
        .collect()
}

#[test]
fn a_tracker_resumed_at_head_floor_commits_the_next_row_id_contiguously() {
    // The exporter: an ordinary pane that filled its own scrollback.
    let mut exporter = Pane::new();
    let mut before = HistoryTracker::new(COLS, ROWS);
    exporter.fill(&mut before, 0, LINES);

    let head = before.head();
    let floor = before.oldest_row().0;
    assert!(
        floor.get() > 0,
        "this fixture must actually evict, or the resumed floor proves nothing (head {head} floor {floor})"
    );
    assert!(
        head.get() > floor.get(),
        "and must hold rows: {floor}..{head}"
    );

    // The importer: a fresh terminal handed the same rows in one replay — the
    // shape D-M3-4 describes, seeded before anything observes — and a tracker
    // told where the id space left off rather than starting one of its own.
    let mut importer = Pane::new();
    importer.write_lines(0, LINES);
    let mut after = HistoryTracker::resume(COLS, ROWS, head, floor);
    let adopted = importer.observe(&mut after);

    assert!(
        invalidations(&adopted).is_empty(),
        "the first look at an inherited terminal must adopt it, not read it as \
         a clear — an invalidation here drops exactly the rows the manifest \
         went to the trouble of carrying: {adopted:?}"
    );
    assert_eq!(after.head(), head, "the head continues where it left off");
    assert_eq!(after.oldest_row().0, floor, "and so does the floor");

    // The id a pre-swap fetch returned still names the same row afterwards.
    let sampled = RowId::from_raw(floor.get() + 3);
    assert_eq!(
        importer.row_text(&mut after, sampled),
        exporter.row_text(&mut before, sampled),
        "row {sampled} means something different on the successor",
    );

    // And the next row committed takes the next id, on both sides.
    exporter.write_lines(LINES, 1);
    importer.write_lines(LINES, 1);
    let theirs = committed(&exporter.observe(&mut before));
    let ours = committed(&importer.observe(&mut after));
    assert_eq!(
        ours, theirs,
        "the successor's next commit must be the same row"
    );
    assert_eq!(
        ours.first().map(|range| range.first),
        Some(head),
        "and it must be `head` itself — contiguous, never restarted",
    );
}

/// `HistoryTracker::new` is `resume` at a zero id space, and behaves exactly as
/// it always did.
///
/// Asserted rather than assumed: `new` delegates to `resume` now, and the
/// adopt-on-first-look rule the resumed case needs runs for a fresh tracker
/// too. It is a no-op there — nothing has been issued, so the rebaseline it
/// replaces moved nothing and announced nothing — and this is the proof.
#[test]
fn a_fresh_tracker_still_starts_at_zero_and_announces_nothing_on_its_first_look() {
    let mut pane = Pane::new();
    let mut tracker = HistoryTracker::new(COLS, ROWS);
    assert_eq!(tracker.head(), RowId::from_raw(0));
    assert_eq!(tracker.oldest_row().0, RowId::from_raw(0));

    pane.write_lines(0, 12);
    let first = pane.observe(&mut tracker);
    assert!(
        invalidations(&first).is_empty(),
        "a pane's first frame invalidates nothing: {first:?}"
    );
    assert_eq!(
        committed(&first).first().map(|range| range.first),
        Some(RowId::from_raw(0)),
        "and its first committed row is row zero: {first:?}",
    );
}
