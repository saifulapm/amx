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
use amx_proto::stream::FlowControl;
use amx_server::conn::writer::{self, Outbound};
use amx_server::damage::{DirtySet, Encoder, KeyframePolicy};
use tokio_util::sync::CancellationToken;

use super::harness::{MAX_FRAME, Pane, grid_stream};

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

/// Let the writer task drain the queue and drop the frame it popped.
///
/// Single-threaded runtime on purpose: yielding runs the writer to its next
/// await point, so when this returns the frame's payload has been written and
/// dropped — which is exactly the condition under which the encoder's buffer
/// is reclaimable on the next build.
async fn drained(out: &Outbound) {
    while !out.is_empty() {
        tokio::task::yield_now().await;
    }
    // The writer drops the popped frame at the end of its loop iteration; one
    // more turn guarantees that iteration has finished.
    tokio::task::yield_now().await;
}

/// The production path the pump drives — [`GridStream::flush`] building the
/// payload, freezing it into an outbound frame, and queueing it for the writer
/// — must not allocate per message either. The encoder-only test above holds
/// the scratch buffers to the rule; this one holds the hand-off to the wire,
/// which is where a per-frame `Bytes::copy_from_slice` used to hide.
///
/// [`GridStream::flush`]: amx_server::damage::GridStream::flush
#[tokio::test]
async fn grid_stream_flush_and_outbound_do_not_allocate_per_frame() {
    let pane = Pane::controlled().await;
    pane.write(b"cells for the stream to flush\r".as_slice()).await;
    let snapshot = pane.snapshot_until("cells for the stream").await;
    let generation = pane.feed().generation();

    let mut stream = grid_stream(&pane, KeyframePolicy::default());
    let (out, queue) = writer::channel();
    let cancel = CancellationToken::new();
    let writer = tokio::spawn(writer::run(tokio::io::sink(), queue, cancel.clone()));

    stream.absorb(&snapshot, generation);

    // Warm to steady state: the first flushes size the scratch, the queue and
    // the payload buffer, and the writer's wake channel cycles past its first
    // internal block boundary so the measured loop sees only reused blocks.
    // A resync forces a keyframe — the largest message this stream ever
    // builds — on every iteration.
    for _ in 0..40 {
        stream.control(FlowControl::Resync {
            stream: stream.stream(),
        });
        stream
            .flush(&snapshot, &out)
            .expect("the keyframe fits the cap")
            .expect("a resync always owes a message");
        drained(&out).await;
    }

    // The drains between iterations await, so the measurement brackets each
    // synchronous flush rather than the whole loop: `allocations()` is
    // thread-local and the window contains no await points.
    let mut allocated = 0;
    for _ in 0..64 {
        stream.control(FlowControl::Resync {
            stream: stream.stream(),
        });
        let before = allocations();
        let sent = stream.flush(&snapshot, &out);
        allocated += allocations() - before;
        sent.expect("the keyframe fits the cap")
            .expect("a resync always owes a message");
        drained(&out).await;
    }

    assert_eq!(
        allocated, 0,
        "flushing 64 keyframes through the outbound queue allocated {allocated} times; \
         the flush path must hand the encoder's buffer out without copying and \
         reclaim it once the writer drops the frame"
    );

    cancel.cancel();
    let report = writer
        .await
        .expect("the writer task ended")
        .expect("the sink never fails");
    assert_eq!(
        report.frames, 104,
        "every flushed keyframe reached the writer"
    );

    pane.stop().await;
}
