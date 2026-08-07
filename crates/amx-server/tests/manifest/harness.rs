//! What the manifest suite reads its fixtures with, and counts allocations
//! with.
//!
//! Split out of the suite so both stay inside the module budget, and because
//! the two halves are different kinds of thing: the suite is the acceptance
//! list, this is the apparatus.
//!
//! The allocation counter is the same shape `amx-vt`'s snapshot suite uses. It
//! is here rather than there because the property it proves is the same one in
//! a different place — HACKING.md's rule that a hot path allocates nothing —
//! and tier-2 evaluation runs on the damage path of every tracked pane.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use amx_core::agent::AgentState;
use amx_server::agent::manifest::{Manifest, ManifestError, ScreenBuf, Verdict};

/// Counts Rust-side allocations made by the arming thread.
///
/// Per-thread, so the other tests in this binary running in parallel cannot
/// inflate it.
pub struct Counting;

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

/// Count the allocations `body` makes on this thread.
pub fn allocations(body: impl FnOnce()) -> usize {
    ALLOCATIONS.set(0);
    ARMED.set(true);
    body();
    ARMED.set(false);
    ALLOCATIONS.get()
}

/// Compile `text` as an override manifest from a nominal path.
pub fn parse(text: &str) -> Manifest {
    Manifest::parse_override(text, "/tmp/test.toml").expect("the manifest compiles")
}

/// The error `text` fails to compile with.
pub fn reject(text: &str) -> ManifestError {
    Manifest::parse_override(text, "/tmp/test.toml").expect_err("the manifest is rejected")
}

/// A recorded screen, by fixture name.
///
/// The files are real grids, captured from the real UIs; their provenance is
/// `tests/fixtures/manifest/README.md`.
pub fn screen(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/manifest")
        .join(format!("{name}.txt"));
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The state `manifest` reads off `grid` with `title`, or `None`.
pub fn state_of(manifest: &Manifest, grid: &str, title: &str) -> Option<AgentState> {
    let mut buf = ScreenBuf::new();
    buf.fill(grid.lines());
    match manifest.evaluate(buf.screen(title)) {
        Verdict::Assert { state, .. } => Some(state),
        Verdict::Hold { .. } | Verdict::NoMatch => None,
    }
}

/// The rule `manifest` matched on `grid`, or `None`.
pub fn winner_of(manifest: &Manifest, grid: &str, title: &str) -> Option<String> {
    let mut buf = ScreenBuf::new();
    buf.fill(grid.lines());
    match manifest.evaluate(buf.screen(title)) {
        Verdict::Assert { rule, .. } | Verdict::Hold { rule } => Some(rule.name().to_owned()),
        Verdict::NoMatch => None,
    }
}

/// A manifest that uses every field of the grammar at least once.
///
/// The suite's specification fixture: three tests read it, and between them
/// they cover every rule field, every gate operator and all four regions.
pub const GRAMMAR: &str = r#"
id = "grammar"
version = "1"

[[rule]]
name = "overlay"
priority = 1000
region = "bottom_lines(3)"
skip_state_update = true
contains = ["showing detailed transcript"]

[[rule]]
name = "dialog"
state = "blocked"
priority = 900
region = "bottom_non_empty_lines(4)"
contains = ["do you want to proceed?"]
any = [
  { line_regex = ['^\s*1\.\s*Yes'] },
  { contains = ["esc to cancel"] },
]

[[rule]]
name = "spinner"
state = "working"
priority = 800
region = "title"
regex = ['^[\x{2800}-\x{28FF}]']

[[rule]]
name = "prompt"
state = "idle"
priority = 200
visible_idle = true
region = "bottom_non_empty_lines(2)"
line_regex = ['^>\s']
not = [{ contains = ["esc to interrupt"] }]
"#;
