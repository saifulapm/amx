//! The broadcast bus and its bounded replay buffer.

use crate::event::cursor::Subscription;
use crate::event::{Event, Seq};

/// Replay capacity used when nothing more specific is configured.
pub const DEFAULT_REPLAY_CAPACITY: usize = 1024;

/// A broadcast bus with per-subscriber cursors over a bounded replay buffer.
///
/// The bus is the single ordering authority in the server: every state
/// transition is published here exactly once and receives the next sequence
/// number. Subscribers read at their own pace; the replay buffer bounds how far
/// behind a subscriber may fall before it is told, explicitly, what it missed.
///
/// `publish` is on the hot path and must not allocate after warm-up — the
/// replay buffer is a fixed-capacity ring that is written in place, never a
/// growing queue.
#[derive(Debug)]
pub struct Bus {
    replay_capacity: usize,
}

impl Bus {
    /// A bus whose replay buffer holds `replay_capacity` events.
    ///
    /// Capacity is the width of the recovery window: a subscriber that falls
    /// more than this many events behind gets a [`Gap`](crate::event::Delivery::Gap)
    /// instead of the events it missed.
    #[must_use]
    pub fn new(replay_capacity: usize) -> Self {
        Self { replay_capacity }
    }

    /// How many events the replay buffer holds.
    #[must_use]
    pub fn replay_capacity(&self) -> usize {
        self.replay_capacity
    }

    /// Publish one transition and return the sequence number it was given.
    ///
    /// Sequence numbers are strictly increasing and gapless at the bus; a gap
    /// only ever exists in what a *subscriber* saw, never in what was
    /// published. Takes `&self`: publishers are actors holding a shared handle,
    /// not an exclusive one.
    pub fn publish(&self, event: Event) -> Seq {
        let _ = event;
        todo!("assign the next seq, write the replay slot, wake subscribers")
    }

    /// The sequence number of the most recently published event.
    ///
    /// Every state-query response carries this value, so a consumer can say
    /// "this state is as of seq N" and resume the stream at exactly N+1.
    #[must_use]
    pub fn head(&self) -> Seq {
        todo!("read the published head")
    }

    /// Subscribe from now on.
    ///
    /// The returned subscription carries the head at subscribe time
    /// ([`Subscription::subscribed_at`]), which is the sequence a caller pairs
    /// with a state snapshot taken at the same moment.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        todo!("register a cursor at head")
    }

    /// Subscribe to events strictly after `seq`, replaying what is still buffered.
    ///
    /// This is the resume path: a consumer that saw a `Gap{from, to}` re-reads
    /// state and resubscribes after `to`. If `seq` has already fallen out of
    /// the replay buffer, the first delivery is another gap — never a silently
    /// truncated history.
    #[must_use]
    pub fn subscribe_after(&self, seq: Seq) -> Subscription {
        let _ = seq;
        todo!("register a cursor after seq, replaying what remains buffered")
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new(DEFAULT_REPLAY_CAPACITY)
    }
}
