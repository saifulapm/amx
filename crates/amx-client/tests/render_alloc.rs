//! T13 acceptance: repainting the same frame twice, once warmed up, costs no
//! allocation — measured on the real `App::repaint`, not a re-implementation
//! of it. Fails without either half of the change: drop `FrameWriter`'s
//! buffer reuse (`self.buf.clear()` → `self.buf = Vec::new()`) and every
//! frame reallocates from empty; drop `App`'s status cache and every repaint
//! rebuilds the status line's `String`s.
//!
//! One allocator per test binary, so this lives in its own file rather than
//! sharing a binary with anything else in this crate.
//!
//! The counter is **per thread**, and that is load-bearing. `App::repaint` is
//! driven on this test's own thread, but the server it attaches to is a real
//! one: the first attach seeds a workspace with a live pane, and a pane owns a
//! pty reader and an `amx-vt-…` parser, both plain OS threads outside tokio's
//! runtime. A process-global counter attributes their work — parsing whatever
//! the seeded shell prints, whenever it happens to print it — to the eight
//! repaints between the two reads, which on a loaded machine is often enough
//! to redden a client that allocated nothing. Counting per thread measures the
//! repaint and only the repaint, however busy the session behind it is.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::cell;

struct CountingAlloc;

thread_local! {
    /// Allocations made by *this* thread.
    ///
    /// `const`-initialized and holding a type with no destructor, which is
    /// what makes it safe to touch from inside the global allocator: there is
    /// no lazy initialization to allocate for, no destructor to register, and
    /// no post-teardown state to panic on.
    static ALLOCS: cell::Cell<usize> = const { cell::Cell::new(0) };
}

/// Count one allocation against the calling thread.
fn count() {
    let _ = ALLOCS.try_with(|allocs| allocs.set(allocs.get() + 1));
}

/// How many allocations the calling thread has made.
fn allocs() -> usize {
    ALLOCS.try_with(cell::Cell::get).unwrap_or(0)
}

// SAFETY: every method forwards to the system allocator with the layout it
// was given and returns its result unchanged; the counter is the only
// addition.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: AllocLayout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

use amx_client::app::App;
use amx_client::model::{Attrs, Cell, WorkspaceModel};
use amx_core::{GridGeneration, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::stream::{Cursor, CursorShape};

#[tokio::test]
async fn repaint_does_not_allocate_after_the_first_frame() {
    let server = support::Server::start("render-alloc").await;
    let pty = support::open_pty();

    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        ClientInfo {
            name: "amx-render-alloc-test".to_owned(),
            version: "0.0.0".to_owned(),
            term: None,
        },
    )
    .await
    .expect("attach to the real server over the real socket");

    // A workspace with a label — the very thing the status line formats — and
    // a pane grid filling the inset, so the repaint blits real content.
    let pane = PaneId::new_v4();
    let workspace = WorkspaceId::new_v4();
    app.adopt_workspace(
        workspace,
        WorkspaceModel {
            label: Some("work".to_owned()),
            layout: BspLayout::with_root(pane),
        },
    );
    let rows = 21_u16;
    let cols = 78_u16;
    let source: Vec<Cell> = (0..u32::from(rows) * u32::from(cols))
        .map(|i| Cell {
            ch: char::from(b'a' + (i % 26) as u8),
            attrs: Attrs {
                bold: i % 7 == 0,
                ..Attrs::default()
            },
        })
        .collect();
    app.model().pane_mut(pane, rows, cols).apply_reset(
        GridGeneration::FIRST.next(),
        rows,
        cols,
        &source,
        Cursor {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::default(),
            blink: false,
        },
    );

    // Warm-up: the first repaint computes the layout, grows the frame buffer
    // to its steady-state size and builds the status line once.
    for _ in 0..8 {
        app.repaint();
    }
    assert!(
        !app.frame().is_empty(),
        "the warm-up must have painted something"
    );

    let before = allocs();
    for _ in 0..8 {
        app.repaint();
    }
    let after = allocs();

    assert_eq!(
        after, before,
        "repainting the same frame repeatedly must not allocate once warmed up"
    );

    drop(app);
    server.shutdown().await;
}
