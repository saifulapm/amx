//! The broadcast bus and its bounded replay buffer.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::event::cursor::Subscription;
use crate::event::{Envelope, Event, Seq};

/// Replay capacity used when nothing more specific is configured.
pub const DEFAULT_REPLAY_CAPACITY: usize = 1024;

/// State shared between the [`Bus`] and every live [`Subscription`].
///
/// A `Subscription` holds a strong `Arc` to this so it can keep reading
/// buffered events even after the `Bus` handle that created it is gone; the
/// "bus closed" signal (04 §2's `recv` contract) comes separately from
/// [`Bus::changed_tx`] closing, not from this being dropped.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) state: Mutex<State>,
}

/// The replay ring buffer and the monotonic head.
#[derive(Debug)]
pub(crate) struct State {
    /// Sequence of the most recently published event; 0 if none yet.
    pub(crate) head: Seq,
    /// A contiguous suffix of published events, oldest at the front.
    ///
    /// Contiguous because the bus never skips a sequence number itself (04
    /// §2: "a gap only ever exists in what a *subscriber* saw, never in what
    /// was published"), so `buffer.front().seq ..= head` names exactly what
    /// this holds.
    pub(crate) buffer: VecDeque<Envelope>,
}

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
    shared: Arc<Shared>,
    /// Wakes subscribers blocked in `recv`; dropping the `Bus` drops this
    /// sender, which is what makes a dropped bus close every subscription's
    /// stream (`Subscription::recv` returns `None`).
    changed_tx: watch::Sender<()>,
}

impl Bus {
    /// A bus whose replay buffer holds `replay_capacity` events.
    ///
    /// Capacity is the width of the recovery window: a subscriber that falls
    /// more than this many events behind gets a [`Gap`](crate::event::Delivery::Gap)
    /// instead of the events it missed.
    #[must_use]
    pub fn new(replay_capacity: usize) -> Self {
        let (changed_tx, _rx) = watch::channel(());
        Self {
            replay_capacity,
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    head: 0,
                    buffer: VecDeque::with_capacity(replay_capacity),
                }),
            }),
            changed_tx,
        }
    }

    /// How many events the replay buffer holds.
    #[must_use]
    pub fn replay_capacity(&self) -> usize {
        self.replay_capacity
    }

    // Invariant: the lock is only ever held across plain field reads/writes,
    // never across an `.await` or a call that can panic on our behalf, so a
    // poisoned mutex means a real bug worth surfacing rather than a
    // recoverable condition.
    #[allow(
        clippy::expect_used,
        reason = "invariant: lock is never held across a panic"
    )]
    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().expect("bus mutex poisoned")
    }

    /// Publish one transition and return the sequence number it was given.
    ///
    /// Sequence numbers are strictly increasing and gapless at the bus; a gap
    /// only ever exists in what a *subscriber* saw, never in what was
    /// published. Takes `&self`: publishers are actors holding a shared handle,
    /// not an exclusive one.
    pub fn publish(&self, event: Event) -> Seq {
        let seq = {
            let mut state = self.lock_state();
            let seq = state.head + 1;
            state.head = seq;
            if self.replay_capacity > 0 {
                if state.buffer.len() == self.replay_capacity {
                    state.buffer.pop_front();
                }
                state.buffer.push_back(Envelope { seq, event });
            }
            seq
        };
        // No receivers is not an error here: publishing with nobody
        // subscribed yet is normal at startup.
        let _ = self.changed_tx.send(());
        seq
    }

    /// The sequence number of the most recently published event.
    ///
    /// Every state-query response carries this value, so a consumer can say
    /// "this state is as of seq N" and resume the stream at exactly N+1.
    #[must_use]
    pub fn head(&self) -> Seq {
        self.lock_state().head
    }

    /// Subscribe from now on.
    ///
    /// The returned subscription carries the head at subscribe time
    /// ([`Subscription::subscribed_at`]), which is the sequence a caller pairs
    /// with a state snapshot taken at the same moment.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        let head = self.lock_state().head;
        Subscription::from_bus(head, Arc::clone(&self.shared), self.changed_tx.subscribe())
    }

    /// Subscribe to events strictly after `seq`, replaying what is still buffered.
    ///
    /// This is the resume path: a consumer that saw a `Gap{from, to}` re-reads
    /// state and resubscribes after `to`. If `seq` has already fallen out of
    /// the replay buffer, the first delivery is another gap — never a silently
    /// truncated history.
    #[must_use]
    pub fn subscribe_after(&self, seq: Seq) -> Subscription {
        Subscription::from_bus(seq, Arc::clone(&self.shared), self.changed_tx.subscribe())
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new(DEFAULT_REPLAY_CAPACITY)
    }
}
