//! Counting allocations, to hold the encode path to the performance rule.
//!
//! Thread-local rather than global: cargo runs the tests in this binary
//! concurrently, and a global counter would be measuring all of them at once.
//! The counter is a `Cell<usize>` with a const initialiser and no destructor, so
//! touching it from inside the allocator cannot itself allocate.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use amx_core::GridGeneration;
use amx_server::damage::{DirtySet, Encoder};

use super::harness::{MAX_FRAME, Pane};

/// An allocator that counts on the calling thread.
struct CountingAlloc;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        // SAFETY: forwards to the system allocator with the same layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwards to the system allocator with the same layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        // SAFETY: forwards to the system allocator with the same arguments.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// Allocations made on this thread since it started.
pub fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn damage_encode_does_not_allocate_per_frame() {
    let pane = Pane::controlled().await;
    pane.write(b"cells for the encoder to pack\r".as_slice())
        .await;
    let snapshot = pane.snapshot_until("cells for the encoder").await;

    let generation = GridGeneration::FIRST;
    let cap = MAX_FRAME as usize;
    let mut dirty = DirtySet::new(snapshot.rows(), snapshot.cols());
    dirty.mark_all();
    let mut encoder = Encoder::new();

    // Warm the buffers to their steady-state capacity. A keyframe is the
    // largest thing this encoder ever builds, so warming on one sizes every
    // buffer for everything that follows.
    for _ in 0..8 {
        encoder.reset(generation, &snapshot, cap).expect("keyframe");
        encoder
            .delta(generation, &snapshot, &dirty, cap)
            .expect("delta");
        encoder.cursor(&snapshot);
    }

    // No await between these two reads: the measurement stays on one thread.
    let before = allocations();
    for _ in 0..64 {
        encoder.reset(generation, &snapshot, cap).expect("keyframe");
        encoder
            .delta(generation, &snapshot, &dirty, cap)
            .expect("delta");
        encoder.cursor(&snapshot);
    }
    let after = allocations();

    assert_eq!(
        after,
        before,
        "encoding 192 grid messages allocated {} times; the encode path must reuse \
         its rect, cell and payload buffers",
        after - before
    );
    assert!(
        !encoder.payload().is_empty(),
        "the measurement must have encoded something"
    );

    pane.stop().await;
}
