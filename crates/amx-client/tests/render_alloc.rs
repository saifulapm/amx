//! T13 acceptance: rendering the same frame twice, once warmed up, costs no
//! allocation. Fails without the change: drop `FrameWriter::begin_frame`'s
//! buffer reuse (`self.buf.clear()` → `self.buf = Vec::new()`) and every
//! frame reallocates from empty.
//!
//! One process-global allocator per test binary, so this lives in its own
//! file rather than sharing a binary with anything else in this crate.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the system allocator with the layout it
// was given and returns its result unchanged; the counter is the only
// addition.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: AllocLayout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

use amx_client::model::{ClientModel, WorkspaceModel};
use amx_client::render::{FrameWriter, chrome, grid};
use amx_core::{Layout as BspLayout, PaneId, WorkspaceId};

#[test]
fn render_does_not_allocate_after_the_first_frame() {
    let pane = PaneId::new_v4();
    let workspace = WorkspaceId::new_v4();

    let mut model = ClientModel::new(24, 80);
    model.set_workspace(
        workspace,
        WorkspaceModel {
            label: Some("work".to_owned()),
            layout: BspLayout::with_root(pane),
        },
    );
    model.pane_mut(pane, 22, 78);

    // Computed once, outside the timed loop: `amx_core::Layout::rects`
    // allocates a fresh `Vec` every call by design (it lives outside this
    // crate), so a real render loop caches this rather than recomputing it
    // every frame — see `App::repaint`'s `pane_rects` field. Recomputing it
    // inside the loop below would make this test measure `amx-core`'s
    // allocator behaviour instead of this crate's.
    let content = model.content_area();
    let rects = model.pane_rects(content);

    let mut writer = FrameWriter::new();
    let render_once = |writer: &mut FrameWriter, model: &ClientModel| {
        writer.begin_frame();
        for &(pane, rect) in &rects {
            chrome::draw_border(writer, rect);
            let inner = chrome::inset(rect);
            if let Some(pane_grid) = model.pane(pane) {
                grid::blit(writer, pane_grid, inner);
            }
        }
        chrome::status_line(
            writer,
            model.term.h.saturating_sub(1),
            model.term.w,
            " work ",
        );
    };

    // Warm-up: let `FrameWriter`'s buffer grow to this frame's steady-state
    // size.
    for _ in 0..8 {
        render_once(&mut writer, &model);
    }

    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..8 {
        render_once(&mut writer, &model);
    }
    let after = ALLOCS.load(Ordering::Relaxed);

    assert_eq!(
        after, before,
        "rendering the same frame repeatedly must not allocate once warmed up"
    );
}
