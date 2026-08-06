//! Per-subscriber cursors, and what a subscriber can be handed.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::event::bus::Shared;
use crate::event::{Envelope, Seq};

/// What a subscriber receives from the bus.
///
/// 04 §2, stated normatively: **subscribers that fall behind the replay buffer
/// get an explicit `gap{from,to}` — never a silent drop.**
///
/// The contract on a gap, in full:
///
/// - `Gap { from, to }` means every sequence in `from..=to` inclusive was
///   published and is no longer available to this subscriber. `from` is the
///   first lost sequence and `to` is the last; a gap is never empty.
/// - On receiving a gap the subscriber **re-reads state** — the events are
///   gone, but the state they described is not — and then resumes consuming
///   deliveries strictly after `to`.
/// - Deliveries after a gap continue at `to + 1`. Sequence numbers are never
///   reused, so a subscriber can always tell replay from new traffic.
/// - A subscriber that ignores a gap is wrong by construction, which is why the
///   gap is a variant of what `recv` returns rather than an out-of-band flag:
///   it cannot be missed by forgetting to check something.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "delivery", rename_all = "snake_case")]
pub enum Delivery {
    /// One event, in order, with its sequence number.
    Event(Envelope),
    /// Events `from..=to` were published but are no longer available here.
    Gap {
        /// First lost sequence, inclusive.
        from: Seq,
        /// Last lost sequence, inclusive.
        to: Seq,
    },
}

/// A cursor into the bus held by one subscriber.
///
/// Each subscriber has its own cursor: a slow consumer costs the bus a gap, not
/// backpressure on publishers and not memory growth (that asymmetry is the
/// reason the replay buffer is bounded in the first place).
#[derive(Debug)]
pub struct Subscription {
    subscribed_at: Seq,
    next: Seq,
    /// `None` for a [`Subscription::new`] built with no backing bus: such a
    /// subscription behaves as already closed, since there is nothing for it
    /// to read.
    backing: Option<Backing>,
}

#[derive(Debug)]
struct Backing {
    shared: Arc<Shared>,
    changed: watch::Receiver<()>,
}

impl Subscription {
    /// Build a subscription positioned at `subscribed_at`.
    ///
    /// This constructor has no way to reach a bus's replay buffer or wakeup
    /// channel — [`Bus::subscribe`](crate::event::Bus::subscribe) and
    /// [`Bus::subscribe_after`](crate::event::Bus::subscribe_after) are what
    /// actually produce a working, live subscription. A `Subscription` built
    /// directly behaves as already closed: `recv` returns `None` immediately.
    #[must_use]
    pub fn new(subscribed_at: Seq) -> Self {
        Self {
            subscribed_at,
            next: subscribed_at + 1,
            backing: None,
        }
    }

    /// Build a live subscription backed by a bus's shared state.
    pub(crate) fn from_bus(
        subscribed_at: Seq,
        shared: Arc<Shared>,
        changed: watch::Receiver<()>,
    ) -> Self {
        Self {
            subscribed_at,
            next: subscribed_at + 1,
            backing: Some(Backing { shared, changed }),
        }
    }

    /// The bus head at the moment this subscription was created.
    ///
    /// Pair this with a state snapshot taken at the same moment: the snapshot
    /// is "state as of `subscribed_at`", and deliveries carry it forward from
    /// there with no window in which a transition can be lost between the two.
    #[must_use]
    pub fn subscribed_at(&self) -> Seq {
        self.subscribed_at
    }

    /// The next delivery, awaiting one if the subscriber is caught up.
    ///
    /// Returns `None` only when the bus itself is closed — a dropped bus ends
    /// the stream, but a full replay buffer never does; that case is
    /// [`Delivery::Gap`].
    pub async fn recv(&mut self) -> Option<Delivery> {
        loop {
            if let Some(delivery) = self.try_next() {
                return Some(delivery);
            }
            let backing = self.backing.as_mut()?;
            if backing.changed.changed().await.is_err() {
                // Every sender is gone, i.e. the bus was dropped. One last
                // check in case a final publish raced with that drop.
                return self.try_next();
            }
        }
    }

    /// Deliver the next buffered event or gap without waiting, or `None` if
    /// the subscriber is caught up with the bus head.
    // Invariant: the lock is never held across a panic, so poisoning here
    // means a real bug worth surfacing rather than a recoverable condition.
    #[allow(
        clippy::expect_used,
        reason = "invariant: lock is never held across a panic"
    )]
    fn try_next(&mut self) -> Option<Delivery> {
        let backing = self.backing.as_ref()?;
        let state = backing.shared.state.lock().expect("bus mutex poisoned");
        if self.next > state.head {
            return None;
        }
        match state.buffer.front() {
            Some(oldest) if self.next < oldest.seq => {
                let from = self.next;
                let to = oldest.seq - 1;
                self.next = oldest.seq;
                Some(Delivery::Gap { from, to })
            }
            Some(oldest) => {
                let idx = (self.next - oldest.seq) as usize;
                let envelope = state.buffer[idx].clone();
                self.next += 1;
                Some(Delivery::Event(envelope))
            }
            None => {
                let from = self.next;
                let to = state.head;
                self.next = state.head + 1;
                Some(Delivery::Gap { from, to })
            }
        }
    }
}
