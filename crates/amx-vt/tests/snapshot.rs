#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The POD snapshot: damage, staleness across the double buffer, allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use amx_vt::{Effects, RenderState, Snapshot, Snapshots, Terminal, TerminalOptions};

/// Counts Rust-side allocations made by the arming thread.
///
/// Two deliberate limits. The count is per-thread, so the other tests in this
/// binary running in parallel cannot inflate it. And only Rust's global
/// allocator is seen: the vendored library allocates through its own Zig
/// allocator, so what this measures is exactly what the wrapper does, which is
/// what the acceptance property is about.
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
}

fn row_text(snapshot: &Snapshot, index: u16) -> String {
    let row = snapshot.row(index).expect("a row");
    let text: Vec<u8> = row
        .cells()
        .iter()
        .flat_map(|cell| row.text(cell).to_vec())
        .collect();
    String::from_utf8(text)
        .expect("utf8")
        .trim_end()
        .to_string()
}

#[test]
fn the_first_frame_publishes_every_row_and_its_contents() {
    let mut pane = Pane::new(10, 3);
    pane.write(b"one\r\ntwo");
    pane.frame();

    let snapshot = pane.snapshots.latest();
    assert_eq!(snapshot.rows(), 3);
    assert_eq!(snapshot.cols(), 10);
    assert_eq!(snapshot.damage(), [0, 1, 2]);
    assert_eq!(row_text(&snapshot, 0), "one");
    assert_eq!(row_text(&snapshot, 1), "two");
    assert_eq!(snapshot.cursor().y, 1);
}

/// The point of the per-row dirty layer: a steady pane republishes one row, not
/// the whole grid.
#[test]
fn snapshot_publishes_only_damaged_rows() {
    let mut pane = Pane::new(10, 4);
    pane.write(b"\x1b[4;1H");
    pane.frame();

    pane.write(b"tail");
    pane.frame();

    let snapshot = pane.snapshots.latest();
    assert_eq!(snapshot.damage(), [3]);
    assert_eq!(row_text(&snapshot, 3), "tail");

    // A frame with no output at all reports no damage.
    pane.frame();
    assert!(pane.snapshots.latest().damage().is_empty());
}

/// The double-buffer trap: the buffer being filled is the one published two
/// frames ago, so a row changed *last* frame is stale in it even though the
/// library no longer calls that row dirty. Publication has to rewrite those
/// rows anyway — reporting them as damage only once — and this is the test that
/// fails the moment it stops.
///
/// The second frame writes nothing at all, which is what makes the assertion
/// sharp: with no dirty rows to piggyback on, an implementation without the
/// carry list publishes an empty grid.
#[test]
fn a_row_changed_in_the_previous_frame_is_not_stale_in_the_next_buffer() {
    let mut pane = Pane::new(10, 4);
    pane.write(b"alpha");
    pane.frame();
    assert_eq!(row_text(&pane.snapshots.latest(), 0), "alpha");

    pane.frame();
    let quiet = pane.snapshots.latest();
    // Carried rows are rewritten but not re-reported: they were damage last
    // frame, and a client that already applied them must not be told twice.
    assert!(quiet.damage().is_empty(), "nothing changed");
    assert_eq!(
        row_text(&quiet, 0),
        "alpha",
        "the other buffer still has to hold the whole grid"
    );

    // A different row changes; row 0 must survive into the buffer coming round
    // again. Row 0 is legitimately damaged here too — the cursor left it and
    // its old cell has to be repainted — so only the contents are asserted.
    pane.write(b"\x1b[3;1Hgamma");
    pane.frame();

    let snapshot = pane.snapshots.latest();
    assert_eq!(row_text(&snapshot, 0), "alpha");
    assert_eq!(row_text(&snapshot, 2), "gamma");
}

/// A reflow invalidates everything, so the frame is fully damaged and the
/// reflowed text has to be present in the new geometry.
#[test]
fn resize_reflow_marks_the_frame_fully_dirty() {
    let mut pane = Pane::new(4, 3);
    pane.write(b"abcdefgh");
    pane.frame();
    pane.frame();
    assert!(
        pane.snapshots.latest().damage().is_empty(),
        "the pane is quiet before the resize"
    );

    pane.terminal.resize(8, 3).expect("a resize");
    pane.frame();

    let snapshot = pane.snapshots.latest();
    assert_eq!(snapshot.cols(), 8);
    assert_eq!(snapshot.damage(), [0, 1, 2], "every row is damaged");
    assert_eq!(row_text(&snapshot, 0), "abcdefgh", "the text reflowed");
}

#[test]
fn shrinking_the_grid_reshapes_the_snapshot() {
    let mut pane = Pane::new(10, 5);
    pane.write(b"one\r\ntwo\r\nthree");
    pane.frame();

    pane.terminal.resize(10, 2).expect("a resize");
    pane.frame();

    let snapshot = pane.snapshots.latest();
    assert_eq!(snapshot.rows(), 2);
    assert_eq!(snapshot.damage(), [0, 1]);
}

/// After warmup a frame is pure copying: the buffers, their row `Vec`s and
/// their text arenas all keep their capacity, and publication swaps two `Arc`s
/// rather than allocating a third. This is the HACKING.md performance rule for
/// the frame path, asserted rather than asserted-to.
#[test]
fn snapshot_does_not_allocate_after_the_first_frame() {
    let mut pane = Pane::new(40, 8);

    // Warm every buffer: two frames fill both halves of the double buffer, and
    // the writes size the row arenas to their steady-state width.
    for _ in 0..4 {
        pane.write(b"\x1b[1;1Hwarmup line for the arena\r\nsecond line of warmup");
        pane.frame();
    }

    let allocated = allocations(|| {
        for _ in 0..16 {
            pane.write(b"\x1b[1;1Hwarmup line for the arena\r\nsecond line of warmup");
            pane.frame();
        }
    });

    assert_eq!(allocated, 0, "a warm frame must not allocate");
}

/// A reader holding the buffer the parser wants back must not block it. The
/// parser takes a fresh buffer for that frame instead — one allocation, not a
/// stall — and the reader's copy stays valid and unchanged.
#[test]
fn a_reader_holding_a_snapshot_never_blocks_publication() {
    let mut pane = Pane::new(10, 3);
    pane.write(b"first");
    pane.frame();

    let held = pane.snapshots.latest();
    for line in ["second", "third", "fourth"] {
        pane.write(format!("\x1b[1;1H{line}").as_bytes());
        pane.frame();
    }

    assert_eq!(row_text(&held, 0), "first", "the held copy never changed");
    assert_eq!(row_text(&pane.snapshots.latest(), 0), "fourth");
}

#[test]
fn the_generation_counter_advances_once_per_publication() {
    let mut pane = Pane::new(10, 3);
    pane.frame();
    let first = pane.snapshots.latest().generation();
    pane.frame();

    assert_eq!(pane.snapshots.latest().generation(), first + 1);
}

/// The snapshot is plain data, so it can be handed to another thread — which is
/// the whole point of publishing one instead of sharing the terminal.
#[test]
fn a_snapshot_crosses_a_thread_boundary() {
    let mut pane = Pane::new(10, 3);
    pane.write(b"across");
    pane.frame();

    let snapshot = pane.snapshots.latest();
    let handle = std::thread::spawn(move || row_text(&snapshot, 0));
    assert_eq!(handle.join().expect("the thread"), "across");
}
