//! The `StatusView` write-before-publish contract (`docs/08-m2-plan.md` §3).
//!
//! One ordering rule, and the reason it is a rule:
//!
//! > the write protocol is fixed: **update `StatusView`, then publish the
//! > event.** A waiter woken by the event therefore always observes a view at
//! > least as new as the event; the reverse order can hang a wait forever
//! > (wake → read stale view → false → no further event ever comes).
//!
//! The hang is the point. A `wait --until blocked` is a state predicate
//! (04 §2): it reads live state and uses events only as notification. If the
//! event that wakes it can arrive before the state it announces, the predicate
//! evaluates false, the waiter goes back to sleep, and the transition it was
//! waiting for has already happened — so nothing will ever wake it again.
//!
//! V08's `status_view_is_current_before_the_status_event_is_receivable` races a
//! real hub. This suite races the type, which is where the ordering is actually
//! enforced: [`StatusView::commit`] is the only mutator and it takes the events
//! it is about, so there is no way for a later task to publish one before the
//! write.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::agent::{AgentSnapshot, AgentState, StatusCause};
use amx_core::{Bus, Delivery, Event, PaneId, Seq};
use amx_server::actor::{StatusUpdate, StatusView};

fn blocked(transition_seq: Seq) -> AgentSnapshot {
    AgentSnapshot {
        kind: None,
        state: AgentState::Blocked,
        cause: StatusCause::Hook,
        transition_seq,
        attention: None,
        session_ref: None,
    }
}

#[tokio::test]
async fn status_view_orders_write_before_event_publish() {
    let bus = Bus::new(16);
    let view = StatusView::new();
    let pane = PaneId::new_v4();

    // A waiter of exactly the shape `wait --until blocked` has: it subscribes
    // first, then reads *live state* the moment an event wakes it. It never
    // looks at the event's contents — that would make it an event predicate,
    // which is the thing 04 §2 forbids.
    let mut subscription = bus.subscribe();
    let woken = {
        let view = view.clone();
        tokio::spawn(async move {
            loop {
                match subscription.recv().await {
                    Some(Delivery::Event(envelope))
                        if matches!(envelope.event, Event::AgentStatus { .. }) =>
                    {
                        return view.get(pane);
                    }
                    Some(_) => continue,
                    None => return None,
                }
            }
        })
    };

    let seq = view.commit(
        &bus,
        StatusUpdate {
            pane,
            status: Some(blocked(bus.head() + 1)),
            attention: vec![pane],
            events: vec![
                Event::AgentStatus {
                    pane,
                    from: Some(AgentState::Working),
                    to: AgentState::Blocked,
                    cause: StatusCause::Hook,
                },
                Event::AttentionEnqueued { pane },
            ],
        },
    );

    // Whatever the scheduler did with the two tasks, the waiter cannot have
    // seen the event before the write it describes — so it cannot have read a
    // view that would send it back to sleep forever.
    let seen = woken.await.expect("the waiter task");
    assert_eq!(
        seen.map(|status| status.state),
        Some(AgentState::Blocked),
        "a waiter woken by agent_status must not read a stale view",
    );

    // `commit` returns the last sequence it published, which is the seq a reply
    // carries back to the caller.
    assert_eq!(seq, Some(bus.head()));
}

#[tokio::test]
async fn a_commit_with_nothing_to_announce_publishes_nothing() {
    let bus = Bus::new(16);
    let view = StatusView::new();
    let pane = PaneId::new_v4();
    let head = bus.head();

    // Bookkeeping with no transition behind it: 04 §2 gives every *transition*
    // a sequence number, not every write. A view update that published anyway
    // would burn sequence numbers a client has to fetch state for.
    let seq = view.commit(
        &bus,
        StatusUpdate {
            pane,
            status: Some(blocked(head)),
            attention: Vec::new(),
            events: Vec::new(),
        },
    );
    assert_eq!(seq, None);
    assert_eq!(bus.head(), head, "no event, no sequence number");
    assert!(view.get(pane).is_some(), "the write still happened");
}

#[tokio::test]
async fn queue_position_is_derived_from_the_queue_not_stored_on_the_status() {
    let bus = Bus::new(16);
    let view = StatusView::new();
    let first = PaneId::new_v4();
    let second = PaneId::new_v4();

    for (pane, queue) in [(first, vec![first]), (second, vec![first, second])] {
        view.commit(
            &bus,
            StatusUpdate {
                pane,
                status: Some(blocked(bus.head())),
                attention: queue,
                events: vec![Event::AttentionEnqueued { pane }],
            },
        );
    }
    assert_eq!(view.get(first).and_then(|s| s.attention), Some(0));
    assert_eq!(view.get(second).and_then(|s| s.attention), Some(1));
    assert_eq!(view.attention(), vec![first, second]);

    // The head unblocks. The pane behind it moves up — and it does so without
    // anyone rewriting its status, because the position was never stored there.
    // A stored index would have gone stale here, and the status line would
    // render a queue that does not exist.
    view.commit(
        &bus,
        StatusUpdate {
            pane: first,
            status: Some(AgentSnapshot {
                state: AgentState::Idle,
                cause: StatusCause::Screen,
                ..blocked(bus.head())
            }),
            attention: vec![second],
            events: vec![Event::AttentionDequeued { pane: first }],
        },
    );
    assert_eq!(view.get(first).and_then(|s| s.attention), None);
    assert_eq!(view.get(second).and_then(|s| s.attention), Some(0));
}

#[tokio::test]
async fn retiring_a_pane_removes_it_from_the_view_and_the_queue() {
    let bus = Bus::new(16);
    let view = StatusView::new();
    let pane = PaneId::new_v4();

    view.commit(
        &bus,
        StatusUpdate {
            pane,
            status: Some(blocked(bus.head())),
            attention: vec![pane],
            events: vec![Event::AttentionEnqueued { pane }],
        },
    );

    // A pane whose process ended. Its tracker retires, and the queue loses it
    // in the same commit — a queue holding a dead pane would send
    // `next-attention` somewhere that is not there.
    view.commit(
        &bus,
        StatusUpdate {
            pane,
            status: None,
            attention: Vec::new(),
            events: vec![Event::AttentionDequeued { pane }],
        },
    );
    assert!(view.get(pane).is_none());
    assert!(view.attention().is_empty());
}
