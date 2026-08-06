//! The event bus's wait/gap contract (04 §2): loss is visible (`Delivery::Gap`,
//! never a silent drop) and recoverable (`Waiter` re-evaluates state across a
//! gap, so a transition that falls inside one can never hang a wait).
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use amx_core::PaneId;
use amx_core::event::{Bus, Delivery, Event, StatePredicate, Waiter};

/// A predicate over a piece of state owned by the test, not over events —
/// exactly the shape 04 §2 requires: `evaluate` reads live state, `interested`
/// is only ever a notification filter.
struct FlagIsTrue(Arc<AtomicBool>);

impl StatePredicate for FlagIsTrue {
    type Output = ();

    fn evaluate(&mut self) -> Option<()> {
        self.0.load(Ordering::SeqCst).then_some(())
    }
}

fn pane_exited(status: i32) -> Event {
    Event::PaneExited {
        pane: PaneId::new_v4(),
        status: Some(status),
    }
}

#[tokio::test]
async fn slow_subscriber_receives_gap_never_silent_drop() {
    let bus = Bus::new(4);
    let mut sub = bus.subscribe();

    for i in 0..10 {
        bus.publish(pane_exited(i));
    }

    let delivery = sub.recv().await.expect("bus is open");
    assert!(
        matches!(delivery, Delivery::Gap { .. }),
        "a subscriber that fell behind must see a gap, not a silently truncated stream; got {delivery:?}"
    );
}

#[tokio::test]
async fn gap_reports_exact_from_and_to_bounds() {
    let bus = Bus::new(4);
    let mut sub = bus.subscribe();

    for i in 0..10 {
        bus.publish(pane_exited(i));
    }
    // head = 10, replay buffer holds seqs 7..=10 (the last 4), so everything
    // from 1..=6 was published but is no longer available to this subscriber.
    assert_eq!(bus.head(), 10);

    let gap = sub.recv().await.expect("bus is open");
    assert_eq!(gap, Delivery::Gap { from: 1, to: 6 });

    let next = sub.recv().await.expect("bus is open");
    match next {
        Delivery::Event(envelope) => assert_eq!(envelope.seq, 7),
        other => panic!("expected the first still-buffered event after the gap, got {other:?}"),
    }
}

#[tokio::test]
async fn subscribe_reply_carries_head_seq() {
    let bus = Bus::new(8);
    for i in 0..3 {
        bus.publish(pane_exited(i));
    }

    let sub = bus.subscribe();
    assert_eq!(sub.subscribed_at(), 3);
    assert_eq!(sub.subscribed_at(), bus.head());
}

#[tokio::test]
async fn wait_predicate_true_at_subscribe_returns_without_any_event() {
    let bus = Bus::new(8);
    let flag = Arc::new(AtomicBool::new(true));
    let waiter = Waiter::new(&bus, FlagIsTrue(flag));

    // Nothing is ever published. If `wait` did not evaluate before consuming
    // events, this would hang forever waiting for a delivery that never comes.
    let outcome = tokio::time::timeout(Duration::from_millis(200), waiter.wait())
        .await
        .expect("a wait for an already-true predicate must not block on events at all");
    assert!(outcome.is_ok());
}

#[tokio::test]
async fn transition_inside_a_gap_does_not_hang_the_wait() {
    let bus = Bus::new(4);
    let flag = Arc::new(AtomicBool::new(false));
    let waiter = Waiter::new(&bus, FlagIsTrue(Arc::clone(&flag)));
    assert_eq!(waiter.subscribed_at(), 0);

    // The transition the waiter cares about happens here, but it is never
    // observed as an event: flooding the bus past its replay capacity before
    // the waiter ever calls `recv` guarantees the seq the flip landed on has
    // already been evicted, so the only delivery the waiter can get is a gap
    // that spans it.
    flag.store(true, Ordering::SeqCst);
    for i in 0..20 {
        bus.publish(pane_exited(i));
    }
    assert!(
        bus.head() as usize > bus.replay_capacity(),
        "the flood must outrun the replay buffer for this test to prove anything"
    );

    let outcome = tokio::time::timeout(Duration::from_millis(500), waiter.wait())
        .await
        .expect("a gap must force re-evaluation, not a hang, even though no event carried the transition");
    assert!(
        outcome.is_ok(),
        "predicate should have been satisfied via gap re-evaluation"
    );
}

// A thread-local counting allocator: each `#[test]` (including the function
// `#[tokio::test]` expands to) runs on its own thread, so this isolates the
// count from whatever unrelated tests are doing concurrently in this binary.
struct CountingAlloc;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|count| count.set(count.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.with(|count| count.set(count.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn allocs() -> usize {
    ALLOCS.with(Cell::get)
}

#[test]
fn publish_is_allocation_free_after_warmup() {
    let bus = Bus::new(64);
    // One pane id, reused for every event: id generation is not what this
    // test is about, and keeping it out of the loop keeps the measurement
    // window free of anything but `Bus::publish` itself.
    let pane = PaneId::new_v4();

    // Warm up: cycle the ring buffer several times over so the buffer's
    // one-time allocation (and anything else with a one-time setup cost) has
    // already happened before we start counting.
    for i in 0..256 {
        bus.publish(Event::PaneExited {
            pane,
            status: Some(i),
        });
    }

    let before = allocs();
    for i in 0..1024 {
        bus.publish(Event::PaneExited {
            pane,
            status: Some(i),
        });
    }
    let after = allocs();

    assert_eq!(
        after, before,
        "publish allocated after warm-up: {before} -> {after}"
    );
}
