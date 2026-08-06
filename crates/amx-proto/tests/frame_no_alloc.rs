//! Proves the 1 MiB control frame cap is enforced from the 5-byte header
//! alone: no buffer for the (attacker-controlled) payload is ever allocated.
//!
//! 04 §4 states the cap as a protocol property, not an implementation detail.
//! `FrameHeader::decode` takes a fixed `[u8; FRAME_HEADER_LEN]` and never sees
//! payload bytes, so structurally it cannot allocate one for them. This test
//! proves that claim by counting real heap allocations across the call.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use amx_proto::FrameHeader;
use amx_proto::frame::{CONTROL_CHANNEL, MAX_CONTROL_FRAME};

struct CountingAlloc;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwards to the system allocator with the same layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwards to the system allocator with the same layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn control_frame_over_1mib_is_rejected_before_allocation() {
    let over = FrameHeader::new(MAX_CONTROL_FRAME as u32 + 1, CONTROL_CHANNEL);
    let bytes = over.encode();

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let result = FrameHeader::decode(bytes);
    let after = ALLOCATIONS.load(Ordering::Relaxed);

    assert!(
        result.is_err(),
        "an oversized control frame must be rejected"
    );
    assert_eq!(
        after, before,
        "rejecting an oversized length prefix must not allocate — the payload is never read"
    );
}
