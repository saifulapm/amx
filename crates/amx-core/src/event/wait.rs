//! The wait contract: state predicates, not event predicates.

use thiserror::Error;

use crate::event::{Bus, Delivery, Event, Seq, Subscription};

/// A condition on *current state*, notified by events.
///
/// 04 §2, stated normatively: **waits are state predicates, not event
/// predicates.** `amx wait --until blocked` "evaluates the predicate against
/// current state at subscribe time, then uses events as notification; on any
/// `gap` it re-evaluates state before consuming events after `to`. A transition
/// falling inside a gap can therefore never hang a wait."
///
/// The consequence for implementors: [`evaluate`](Self::evaluate) must read
/// live state, not remember what it has seen. A predicate that accumulates
/// event history is an event predicate wearing this trait's clothes, and it
/// will hang the first time its transition lands inside a gap.
///
/// [`interested`](Self::interested) is a notification filter only. Returning
/// `false` for an event must never change the outcome of a wait — at worst it
/// delays the next evaluation until an event the predicate does care about.
pub trait StatePredicate {
    /// What the wait yields once satisfied.
    type Output;

    /// Evaluate against current state; `Some` completes the wait.
    fn evaluate(&mut self) -> Option<Self::Output>;

    /// Whether this event is worth re-evaluating for.
    ///
    /// Defaults to re-evaluating on everything, which is always correct and
    /// never wrong — only wasteful.
    fn interested(&self, event: &Event) -> bool {
        let _ = event;
        true
    }
}

/// A wait in progress.
///
/// The sequencing is the whole point, so it is spelled out here:
///
/// 1. Subscribe first, capturing `subscribed_at`.
/// 2. Evaluate the predicate. If it is already satisfied, return without
///    consuming a single event — a wait for a condition that already holds must
///    not block for a transition that already happened.
/// 3. Otherwise consume deliveries. On [`Delivery::Event`](crate::event::Delivery::Event),
///    re-evaluate if the predicate is interested.
/// 4. On [`Delivery::Gap`](crate::event::Delivery::Gap), **re-evaluate state
///    unconditionally** before consuming anything after `to`. This is the step
///    that makes a transition inside a gap unable to hang the wait.
///
/// Steps 1 and 2 are in that order for the same reason: evaluating before
/// subscribing leaves a window in which the transition happens unobserved.
#[derive(Debug)]
pub struct Waiter<P> {
    predicate: P,
    subscribed_at: Seq,
    subscription: Subscription,
}

impl<P: StatePredicate> Waiter<P> {
    /// Subscribe to `bus` and prepare to wait on `predicate`.
    ///
    /// Subscribing happens here, before the first evaluation, so the caller
    /// cannot accidentally introduce the window described above.
    #[must_use]
    pub fn new(bus: &Bus, predicate: P) -> Self {
        let subscription = bus.subscribe();
        let subscribed_at = subscription.subscribed_at();
        Self {
            predicate,
            subscribed_at,
            subscription,
        }
    }

    /// The bus sequence this wait subscribed at.
    #[must_use]
    pub fn subscribed_at(&self) -> Seq {
        self.subscribed_at
    }

    /// The predicate being waited on.
    pub fn predicate(&self) -> &P {
        &self.predicate
    }

    /// Run the wait to completion.
    pub async fn wait(mut self) -> Result<P::Output, WaitError> {
        // Step 2: a condition that already holds must not wait for a
        // transition that already happened.
        if let Some(output) = self.predicate.evaluate() {
            return Ok(output);
        }
        loop {
            match self.subscription.recv().await {
                None => return Err(WaitError::BusClosed),
                // A transition inside the gap is exactly why this
                // re-evaluates unconditionally rather than trying to infer
                // what happened from the events on either side of it.
                Some(Delivery::Gap { .. }) => {
                    if let Some(output) = self.predicate.evaluate() {
                        return Ok(output);
                    }
                }
                Some(Delivery::Event(envelope)) => {
                    if self.predicate.interested(&envelope.event)
                        && let Some(output) = self.predicate.evaluate()
                    {
                        return Ok(output);
                    }
                }
            }
        }
    }
}

/// Why a wait ended without its predicate being satisfied.
#[derive(Debug, Error)]
pub enum WaitError {
    /// The bus closed, so no further transition can ever arrive.
    #[error("event bus closed before the predicate was satisfied")]
    BusClosed,
    /// The session is shutting down.
    #[error("cancelled")]
    Cancelled,
}
