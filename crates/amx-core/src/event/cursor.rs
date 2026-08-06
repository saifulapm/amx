//! Per-subscriber cursors, and what a subscriber can be handed.

use serde::{Deserialize, Serialize};

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
}

impl Subscription {
    /// Build a subscription positioned at `subscribed_at`.
    #[must_use]
    pub fn new(subscribed_at: Seq) -> Self {
        Self { subscribed_at }
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
        todo!("advance the cursor, or report the gap it fell into")
    }
}
