#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! Scrollback identity: row ids, invalidation, the eviction floor and ranges.
//!
//! 04 §3 is the specification. The tracker is driven against a terminal
//! directly rather than through a pty, because every assertion here is about
//! *which* row an id names, and a shell's output is not a fact a test can pin.
//! The wiring through a live pane — the parser observing at frame boundaries
//! and serving `PaneCommand::HistoryRange` — is `pane_host.rs`'s.

use amx_core::{EvictionFloor, InvalidationCause, RowId, RowRange};
use amx_server::actor::HistoryError;
use amx_server::history::{HistoryEvent, HistoryTracker};
use amx_vt::{Effects, Terminal, TerminalOptions};

/// A byte bound, not a row count: 8 KiB holds a little over four thousand
/// twenty-column rows (measured), so a few thousand lines fit and six thousand
/// push the early ones off the top.
const SMALL: usize = 8 * 1024;

/// More rows than [`SMALL`] can hold, so the scrollback has to trim.
const OVERFLOW: usize = 6000;

/// Rows per observation. A frame commits what a frame commits; the anchor that
/// carries row identity across a trim only has to survive one of them.
const PER_FRAME: usize = 100;

struct Pane {
    terminal: Terminal,
    effects: Effects,
    history: HistoryTracker,
    found: Vec<HistoryEvent>,
}

impl Pane {
    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: scrollback,
        })
        .expect("a terminal");
        Self {
            terminal,
            effects: Effects::new(),
            history: HistoryTracker::new(cols, rows),
            found: Vec::new(),
        }
    }

    /// Write `lines` numbered lines, then observe once — one "frame".
    fn write_lines(&mut self, from: usize, lines: usize) -> Vec<HistoryEvent> {
        for line in from..from + lines {
            self.effects.clear();
            self.terminal
                .write(format!("L{line}\r\n").as_bytes(), &mut self.effects);
        }
        self.observe()
    }

    fn write(&mut self, bytes: &[u8]) -> Vec<HistoryEvent> {
        self.effects.clear();
        self.terminal.write(bytes, &mut self.effects);
        self.observe()
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Vec<HistoryEvent> {
        self.terminal.resize(cols, rows).expect("a resize");
        self.observe()
    }

    /// Write `lines` lines a frame's worth at a time, as a running pane does.
    fn fill(&mut self, from: usize, lines: usize) -> Vec<HistoryEvent> {
        let mut events = Vec::new();
        let mut written = 0;
        while written < lines {
            let batch = PER_FRAME.min(lines - written);
            events.extend(self.write_lines(from + written, batch));
            written += batch;
        }
        events
    }

    fn observe(&mut self) -> Vec<HistoryEvent> {
        self.found.clear();
        self.history.observe(&mut self.terminal, &mut self.found);
        self.found.clone()
    }

    fn oldest_row(&self) -> EvictionFloor {
        self.history.oldest_row()
    }

    /// The text of one row of a served range, unpacked.
    fn row_text(&mut self, id: u64) -> String {
        let row = RowId::from_raw(id);
        let served = self
            .history
            .read(&self.terminal, RowRange::single(row))
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
        rows.push(text.trim_end().to_string());
    }
    assert_eq!(at, bytes.len(), "the packing is self-delimiting");
    rows
}

fn committed(events: &[HistoryEvent]) -> Vec<(RowRange, usize)> {
    events
        .iter()
        .filter_map(|event| match event {
            HistoryEvent::Committed(committed) => Some((committed.range, committed.hashes.len())),
            _ => None,
        })
        .collect()
}

fn invalidations(events: &[HistoryEvent]) -> Vec<(RowId, InvalidationCause)> {
    events
        .iter()
        .filter_map(|event| match event {
            HistoryEvent::Invalidated { from_row, cause } => Some((*from_row, *cause)),
            _ => None,
        })
        .collect()
}

fn evictions(events: &[HistoryEvent]) -> Vec<RowId> {
    events
        .iter()
        .filter_map(|event| match event {
            HistoryEvent::Evicted { oldest_row } => Some(*oldest_row),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------- row ids

/// The contract 04 §3 opens with: ids are assigned when a line is committed and
/// never reused, *including* across the trimming that a byte-limited scrollback
/// does constantly. The check is against content: the row an id names must
/// still hold the line that was written when that id was handed out.
#[test]
fn row_ids_are_monotonic_and_never_reused_across_trimming() {
    let mut pane = Pane::new(20, 4, SMALL);
    let mut seen: Vec<RowRange> = Vec::new();
    let mut written = 0;

    for round in 0..30 {
        let lines = 1 + round * 29;
        let events = pane.write_lines(written, lines);
        written += lines;
        for (range, _) in committed(&events) {
            if let Some(previous) = seen.last() {
                assert_eq!(
                    range.first,
                    previous.last.next(),
                    "ids are dense and increasing across frames"
                );
            }
            seen.push(range);
        }
    }

    let head = seen.last().expect("rows were committed").last;
    let floor = pane.oldest_row();
    assert!(
        floor.0 > RowId::FIRST,
        "the scrollback trimmed, so the floor moved: {floor:?}"
    );

    // Every row still fetchable holds the line whose id it was given.
    for id in floor.0.get()..=head.get() {
        assert_eq!(pane.row_text(id), format!("L{id}"));
    }
}

/// A clear discards history; the ids it discarded are never handed out again.
#[test]
fn row_ids_are_never_reused_across_clear() {
    let mut pane = Pane::new(20, 4, SMALL);
    let before = pane.write_lines(0, 40);
    let (last, _) = *committed(&before).last().expect("rows were committed");
    let head = last.last;

    // CSI 3J: erase the scrollback, exactly what `clear` sends.
    let cleared = pane.write(b"\x1b[3J");
    assert_eq!(
        invalidations(&cleared),
        vec![(RowId::FIRST, InvalidationCause::Clear)],
        "everything a client cached is gone"
    );
    assert!(pane.history.committed().is_none(), "history is empty");

    let after = pane.write_lines(100, 40);
    let (first, _) = *committed(&after).first().expect("rows were committed");
    assert!(
        first.first > head,
        "new rows take ids past every id ever issued: {first:?} follows {head}"
    );
    // A clear discards *history*, not the live grid, so the rows that were on
    // screen when it arrived are the next ones committed — under fresh ids.
    assert_eq!(pane.row_text(first.first.get()), "L37");
}

/// The narrow case both halves of "monotonic *and* never reused" have to
/// survive: a taller grid pulls committed rows back, so the head falls below
/// ids that were already announced, and then the application clears. The fresh
/// id space has to start above the highest id ever issued, not above the head.
#[test]
fn row_ids_start_above_every_id_ever_issued_not_above_the_head() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.write_lines(0, 40);
    let issued = pane.history.head();

    pane.resize(20, 12);
    let receded = pane.history.head();
    assert!(receded < issued, "the taller grid reclaimed rows");

    pane.write(b"\x1b[3J");
    let after = pane.write_lines(500, 40);
    let (first, _) = *committed(&after).first().expect("rows were committed");

    assert!(
        first.first >= issued,
        "{first:?} reuses an id from below {issued}"
    );
}

// -------------------------------------------------------------- invalidation

/// Only width changes reflow (04 §3), and a reflow rewrites history from its
/// oldest row — so the invalidation starts at the eviction floor, not at the
/// tail — and the rows that survive are renumbered rather than reused.
#[test]
fn width_change_emits_history_invalidated_with_the_right_from_row() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.fill(0, OVERFLOW);
    let floor = pane.oldest_row().0;
    let head = pane.history.head();
    assert!(floor > RowId::FIRST, "the floor moved before the resize");

    let events = pane.resize(10, 4);

    assert_eq!(
        invalidations(&events),
        vec![(floor, InvalidationCause::WidthReflow)],
        "from the oldest row a client could still be holding"
    );
    assert_eq!(
        pane.oldest_row().0,
        head,
        "surviving rows are renumbered past every id ever issued"
    );
    assert!(pane.history.head() > head);
}

/// A height change moves rows between history and the live grid without
/// reflowing anything, so nothing a client cached became wrong.
#[test]
fn height_change_does_not_emit_history_invalidated() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.write_lines(0, 200);
    let floor = pane.oldest_row();
    let head = pane.history.head();

    let taller = pane.resize(20, 10);
    let shorter = pane.resize(20, 4);

    assert!(invalidations(&taller).is_empty(), "{taller:?}");
    assert!(invalidations(&shorter).is_empty(), "{shorter:?}");
    assert!(evictions(&taller).is_empty(), "nothing was evicted");
    assert_eq!(pane.oldest_row(), floor, "the floor did not move");
    assert_eq!(
        pane.history.head(),
        head,
        "the rows the taller grid reclaimed came back under their own ids"
    );
    // And they are still the rows those ids named.
    assert_eq!(
        pane.row_text(head.get() - 1),
        format!("L{}", head.get() - 1)
    );
}

// ------------------------------------------------------------ eviction floor

/// 04 §3: clients "know which ranges the byte-limited scrollback has evicted
/// and cannot re-fetch". Refused, never answered with blanks.
#[test]
fn eviction_floor_advances_and_ranges_below_it_are_refused_not_faked() {
    let mut pane = Pane::new(20, 4, SMALL);
    let first = pane.write_lines(0, 40);
    assert_eq!(pane.oldest_row().0, RowId::FIRST);
    assert!(evictions(&first).is_empty(), "nothing has been evicted yet");

    let flooded = pane.fill(40, OVERFLOW);
    let floor = *evictions(&flooded).last().expect("the floor advanced");
    assert_eq!(floor, pane.oldest_row().0);
    assert!(floor > RowId::FIRST);

    let evicted = RowRange::new(RowId::FIRST, floor);
    assert_eq!(
        pane.history.read(&pane.terminal, evicted),
        Err(HistoryError::Evicted { oldest: floor }),
        "a range that reaches below the floor is refused whole"
    );
    assert_eq!(
        pane.history
            .read(&pane.terminal, RowRange::single(pane.history.head())),
        Err(HistoryError::NotCommitted {
            head: RowId::from_raw(pane.history.head().get() - 1),
        }),
        "so is a row that has not been committed"
    );
    // The floor itself still serves, and serves the right row.
    assert_eq!(pane.row_text(floor.get()), format!("L{}", floor.get()));
}

/// When one frame commits more rows than the whole scrollback holds, the anchor
/// that carries identity is pruned with them and the count is unrecoverable.
/// The model says so rather than guessing: ids jump past everything ever
/// issued, clients are told the range they hold is invalid, and the floor moves
/// to match. It is the event bus's `gap` doctrine applied to scrollback.
#[test]
fn a_history_turning_over_inside_one_frame_is_announced_not_guessed() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.fill(0, 500);
    let floor = pane.oldest_row().0;
    let head = pane.history.head();
    let cached = pane.history.committed().expect("history");

    // One frame, more rows than the scrollback can hold.
    let events = pane.write_lines(10_000, OVERFLOW * 2);

    assert_eq!(
        invalidations(&events),
        vec![(floor, InvalidationCause::Clear)],
        "the client is told the ids it holds no longer mean anything"
    );
    assert_eq!(pane.oldest_row().0, head, "the floor moved past every id");
    assert_eq!(
        pane.history.read(&pane.terminal, cached),
        Err(HistoryError::Evicted { oldest: head }),
        "and what it cached is refused rather than answered with other rows"
    );
    let fresh = pane.history.committed().expect("history");
    assert_eq!(fresh.first, head, "surviving rows take never-issued ids");
    assert!(pane.row_text(fresh.first.get()).starts_with('L'));
}

// ------------------------------------------------------------ announcements

/// 04 §3: "Rows scrolling out of the live grid into history are announced on
/// the pane's delta stream (id + content hash …)".
#[test]
fn rows_scrolling_out_of_the_live_grid_are_announced_with_id_and_hash() {
    let mut pane = Pane::new(20, 4, SMALL);

    let events = pane.write_lines(0, 10);

    let announced = committed(&events);
    let (range, hashes) = *announced.first().expect("rows left the live grid");
    assert_eq!(range.first, RowId::FIRST);
    assert_eq!(
        u64::try_from(hashes).expect("a count"),
        range.row_count().expect("rows"),
        "every announced row carries a hash at this rate"
    );

    let HistoryEvent::Committed(committed) = &events[0] else {
        panic!("the first event announces the commit: {events:?}");
    };
    assert_eq!(committed.hashed(), Some(range));

    // The hash is the one a client can recompute from the row it fetched.
    let rows = pane
        .history
        .read(&pane.terminal, range)
        .expect("the announced rows");
    let mut at = 0;
    for (index, hash) in committed.hashes.iter().enumerate() {
        let len =
            u32::from_le_bytes(rows.rows[at..at + 4].try_into().expect("four bytes")) as usize;
        let end = at + 4 + len + 1;
        assert_eq!(
            *hash,
            amx_server::history::hash_bytes(&rows.rows[at..end]),
            "row {index} hashes to what it was announced with"
        );
        at = end;
    }
    assert_eq!(at, rows.rows.len());
}

/// The hash is what tells a client that an id it holds now means a different
/// row: a taller grid pulls rows back into the live area, the application
/// overwrites them, and they commit again under the ids they already had.
#[test]
fn a_reissued_id_is_announced_with_a_new_hash() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.write_lines(0, 40);
    let head = pane.history.head().get();
    let was = pane.row_text(head - 1);

    // Reclaim rows into the live area, overwrite them, and let them commit.
    pane.resize(20, 8);
    let reclaimed = pane.history.head().get();
    assert!(reclaimed < head, "the head receded");
    pane.write(b"\x1b[H\x1b[Joverwritten\r\n");
    let events = pane.write_lines(500, 10);

    let (range, _) = *committed(&events).first().expect("rows committed again");
    assert!(
        range.first.get() < head,
        "ids the pane had already issued are back: {range:?} below {head}"
    );
    assert_ne!(pane.row_text(range.first.get()), was);
    assert_eq!(pane.row_text(range.first.get()), "overwritten");
}

// -------------------------------------------------------------------- ranges

/// 04 §4 chunks history so a bulk fetch cannot head-of-line-block a keystroke.
/// Chunking is only free if it is invisible: the chunks must reassemble into
/// exactly the bytes the whole range would have served.
#[test]
fn history_range_is_chunked_and_reassembles_byte_identical() {
    let mut pane = Pane::new(40, 4, 512 * 1024);
    pane.write_lines(0, 1000);
    let range = pane.history.committed().expect("history");

    let whole = pane
        .history
        .read(&pane.terminal, range)
        .expect("the whole range");

    let mut chunks = pane.history.chunked(range, 64).expect("a chunked read");
    let mut reassembled = Vec::new();
    let mut served = Vec::new();
    while let Some(chunk) = chunks.next_chunk(&mut pane.history, &pane.terminal, &mut reassembled) {
        served.push(chunk.expect("a chunk"));
    }

    assert!(served.len() > 10, "the range really was chunked");
    assert_eq!(served[0].first, range.first);
    assert_eq!(served.last().expect("a chunk").last, range.last);
    for pair in served.windows(2) {
        assert_eq!(pair[1].first, pair[0].last.next(), "chunks are contiguous");
    }
    assert_eq!(reassembled, whole.rows, "byte-identical");
    assert_eq!(
        unpack(&reassembled).len(),
        range.row_count().expect("rows") as usize
    );
    assert_eq!(unpack(&reassembled)[0], "L0");
}

/// A transfer the scrollback outruns is refused where it stops, not filled in.
#[test]
fn a_chunked_read_is_rechecked_against_the_floor_between_chunks() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.write_lines(0, 100);
    let range = pane.history.committed().expect("history");
    let mut chunks = pane.history.chunked(range, 8).expect("a chunked read");

    let mut out = Vec::new();
    chunks
        .next_chunk(&mut pane.history, &pane.terminal, &mut out)
        .expect("a first chunk")
        .expect("it served");

    // The pane runs on far enough to evict everything the transfer had left.
    pane.fill(1000, OVERFLOW);

    let refused = chunks
        .next_chunk(&mut pane.history, &pane.terminal, &mut out)
        .expect("a second chunk");
    assert!(
        matches!(refused, Err(HistoryError::Evicted { .. })),
        "{refused:?}"
    );
    assert!(!chunks.more(), "a refused chunk ends the transfer");
}

// ------------------------------------------------------------- other screens

/// Both scrollback counters are per active screen and the alternate screen has
/// none, so tracking freezes there rather than reading a wipe that never was.
#[test]
fn the_alternate_screen_neither_commits_nor_invalidates() {
    let mut pane = Pane::new(20, 4, SMALL);
    pane.write_lines(0, 40);
    let floor = pane.oldest_row();
    let head = pane.history.head();

    let entered = pane.write(b"\x1b[?1049h");
    let inside = pane.write_lines(900, 40);
    let left = pane.write(b"\x1b[?1049l");

    assert!(entered.is_empty(), "{entered:?}");
    assert!(inside.is_empty(), "{inside:?}");
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(pane.oldest_row(), floor);
    assert_eq!(pane.history.head(), head);
    assert_eq!(
        pane.row_text(head.get() - 1),
        format!("L{}", head.get() - 1),
        "the primary screen's history is exactly where it was"
    );
}
