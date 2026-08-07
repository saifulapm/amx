#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The snapshot's text view: the row lines and grid tail detection reads.
//!
//! The properties under test are the ones the detection path depends on — a
//! line that reads as the row was painted, a tail that is the live bottom of
//! the grid, and neither of them allocating or copying, because both run once
//! per row per damage batch.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::mpsc;

use amx_vt::{Effects, RenderState, Snapshot, SnapshotRef, Snapshots, Terminal, TerminalOptions};

/// Counts Rust-side allocations made by the arming thread.
///
/// Per-thread, so the other tests in this binary running in parallel cannot
/// inflate it, and Rust-side only: the vendored library allocates through its
/// own Zig allocator, so what this measures is exactly what the wrapper does.
/// This is the harness `snapshot.rs` uses for the frame path, pointed at the
/// read path instead.
struct Counting;

thread_local! {
    /// `const`-initialised and `Copy`, so touching these from inside the
    /// allocator cannot itself allocate or register a destructor.
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn record() {
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: every method forwards to the system allocator unchanged; the counter
// is the only addition and it does not allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: the caller upholds the `GlobalAlloc` contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds the `GlobalAlloc` contract.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        // SAFETY: the caller upholds the `GlobalAlloc` contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Count the allocations `body` makes on this thread.
fn allocations(body: impl FnOnce()) -> usize {
    ALLOCATIONS.set(0);
    ARMED.set(true);
    body();
    ARMED.set(false);
    ALLOCATIONS.get()
}

struct Pane {
    terminal: Terminal,
    render: RenderState,
    snapshots: Snapshots,
    effects: Effects,
}

impl Pane {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            terminal: Terminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: 100,
            })
            .expect("a terminal"),
            render: RenderState::new().expect("a render state"),
            snapshots: Snapshots::new(cols, rows),
            effects: Effects::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.effects.clear();
        self.terminal.write(bytes, &mut self.effects);
    }

    /// One whole frame: read the terminal, then copy the damage out.
    fn frame(&mut self) {
        self.render.update(&mut self.terminal).expect("an update");
        self.snapshots.publish(&mut self.render).expect("a publish");
    }

    /// Write and publish in one step, and hand back the published grid.
    fn paint(&mut self, bytes: &[u8]) -> SnapshotRef {
        self.write(bytes);
        self.frame();
        self.snapshots.latest()
    }
}

fn line(snapshot: &Snapshot, index: u16) -> &str {
    snapshot.row(index).expect("a row").line()
}

/// One cell contributes one cluster, whatever it costs in columns or
/// codepoints: a wide character is one `char` and its spacer tail adds nothing,
/// and a base plus its combining mark stay one cluster in one cell.
#[test]
fn row_line_concatenates_cells_including_wide_and_combining() {
    let mut pane = Pane::new(12, 2);
    let snapshot = pane.paint("wi\u{4e16}de\u{0301}x".as_bytes());

    assert_eq!(line(&snapshot, 0), "wi\u{4e16}de\u{0301}x     ");
    assert_eq!(
        line(&snapshot, 0)
            .chars()
            .filter(|c| *c != '\u{0301}')
            .count(),
        11,
        "twelve columns, one of them the second half of the wide character"
    );
    assert_eq!(
        snapshot.row(0).expect("a row").cells().len(),
        12,
        "the grid is still twelve cells wide"
    );
}

/// A column an application skipped over is a column the user sees blank, so the
/// line has to hold that blank. Concatenating only the cells that carry text
/// would slide `b` five columns left and let a rule match text that is not on
/// the screen.
#[test]
fn row_line_holds_the_columns_an_application_skipped() {
    let mut pane = Pane::new(12, 2);
    let snapshot = pane.paint(b"a\x1b[1;7Hb");

    assert_eq!(line(&snapshot, 0), "a     b     ");
    assert_eq!(
        line(&snapshot, 0).find('b'),
        Some(6),
        "b is in column seven"
    );
    assert_eq!(
        line(&snapshot, 1),
        "            ",
        "an untouched row is its width in blanks"
    );
}

/// Spaces the application actually printed are indistinguishable from blanks,
/// which is the point: the line is what the screen shows, not a transcript of
/// what was written.
#[test]
fn row_line_keeps_written_spaces_and_trailing_blanks() {
    let mut pane = Pane::new(12, 1);
    let snapshot = pane.paint(b"  two words");

    assert_eq!(line(&snapshot, 0), "  two words ");
    assert_eq!(line(&snapshot, 0).trim_end(), "  two words");
}

/// Erasing a cell blanks the column, and the line follows — a stale cluster
/// left behind here is a screen rule matching text the user cannot see.
#[test]
fn row_line_follows_an_erase_back_to_blanks() {
    let mut pane = Pane::new(12, 1);
    assert_eq!(line(&pane.paint(b"abcdef"), 0), "abcdef      ");

    // Erase from column three to the end of the line.
    assert_eq!(line(&pane.paint(b"\x1b[1;3H\x1b[K"), 0), "ab          ");
}

#[test]
fn tail_returns_the_last_n_rows_top_to_bottom() {
    let mut pane = Pane::new(8, 5);
    let snapshot = pane.paint(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");

    let last_two: Vec<&str> = snapshot.tail(2).map(|row| row.line().trim_end()).collect();
    assert_eq!(last_two, ["four", "five"], "top to bottom, not reversed");

    let everything: Vec<&str> = snapshot.tail(99).map(|row| row.line().trim_end()).collect();
    assert_eq!(
        everything,
        ["one", "two", "three", "four", "five"],
        "a count past the top clamps to the whole grid"
    );

    assert_eq!(snapshot.tail(0).count(), 0, "no rows asked for, none given");
    assert_eq!(snapshot.tail(2).len(), 2, "the walk knows its own length");
    let bottom_up: Vec<&str> = snapshot
        .tail(3)
        .rev()
        .map(|row| row.line().trim_end())
        .collect();
    assert_eq!(
        bottom_up,
        ["five", "four", "three"],
        "a bottom-anchored rule can walk up without collecting"
    );
}

/// The line is a borrow of the row's own arena, so reading the whole tail — the
/// detection path's per-damage-batch work — costs nothing. Two calls handing
/// back the same pointer is the proof that nothing is built on the way out.
#[test]
fn row_line_borrows_and_does_not_allocate() {
    let mut pane = Pane::new(40, 8);

    // Warm both halves of the double buffer and size the arenas.
    for _ in 0..4 {
        pane.write(b"\x1b[1;1Hwarmup line for the arena\r\nsecond line of warmup");
        pane.frame();
    }
    let snapshot = pane.snapshots.latest();

    let mut lengths = 0;
    let allocated = allocations(|| {
        for row in snapshot.tail(8) {
            lengths += row.line().trim_end().len();
        }
    });

    assert_eq!(allocated, 0, "reading the text view must not allocate");
    assert!(lengths > 0, "the rows actually had text in them");

    let row = snapshot.row(0).expect("a row");
    assert!(
        std::ptr::eq(row.line().as_ptr(), row.text(&row.cells()[0]).as_ptr()),
        "the line is the row's own arena, not a copy built per call"
    );
}

/// A reader holds an `Arc`, and the parser keeps publishing. The held view has
/// to stay exactly as it was — this is the whole reason detection reads a
/// published snapshot instead of the terminal — including from another thread,
/// which is where a detector actually runs.
#[test]
fn text_view_of_a_published_snapshot_is_stable_while_parser_writes() {
    let mut pane = Pane::new(40, 4);
    let held = pane.paint(b"held first line\r\nheld second line");
    let expected: Vec<String> = held.grid().iter().map(|row| row.line().into()).collect();

    let (started, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let reader = {
        let held = SnapshotRef::clone(&held);
        std::thread::spawn(move || {
            started.send(()).expect("the test is listening");
            let mut reads = 0_u32;
            while stopped.try_recv() == Err(mpsc::TryRecvError::Empty) {
                for (index, row) in held.grid().iter().enumerate() {
                    assert_eq!(row.line(), expected[index], "the held view changed");
                }
                reads += 1;
            }
            reads
        })
    };
    wait.recv().expect("the reader started");

    for generation in 0..64 {
        pane.write(
            format!("\x1b[1;1Hgeneration {generation} of noise\r\nand a second row").as_bytes(),
        );
        pane.frame();
    }
    stop.send(()).expect("the reader is listening");
    let reads = reader.join().expect("the reader thread");

    assert!(reads > 0, "the reader ran at least one pass");
    assert_eq!(
        line(&held, 0).trim_end(),
        "held first line",
        "the held snapshot is the one that was published to it"
    );
    assert_eq!(
        line(&pane.snapshots.latest(), 0).trim_end(),
        "generation 63 of noise",
        "while the live grid moved on"
    );
}
