//! The server→client event path, and the long polls that ride beside it.
//!
//! Before V11 there was no such path at all: the JSON-RPC notification type
//! existed (`amx-proto/src/rpc.rs`), the server never sent one, and the client
//! dropped any it received. Everything M2 promises — `⚑N` without polling,
//! `amx events --json`, external notifiers — needs it, so this module builds
//! it (D-M2-5).
//!
//! Three pieces of connection-scoped state live here, and they are together
//! because they share one lifetime and one shutdown:
//!
//! - the **subscription pump**: one task per subscribed connection, turning
//!   each [`Delivery`] into a JSON-RPC notification on the control channel;
//! - the **wait tasks**: `wait` and `pane.wait_output` are long polls, and a
//!   long poll served inline on the reader would stop the connection reading —
//!   including reading the peer's disconnect, which is the thing a wait with no
//!   timeout is documented to end on;
//! - the **[`StatusView`]** and the **bus**, which are what a wait predicate
//!   reads. They are not connection state, but this is the handle a connection
//!   task reaches them through.
//!
//! Everything spawned here is joined by [`ConnEvents::shutdown`] on the
//! connection task's way out — nothing detached, everything joined (04 §2).
//!
//! # Backpressure is a gap, not a drop
//!
//! The pump writes with [`Outbound::send_when_ready`], so a consumer that stops
//! reading stalls the pump rather than losing deliveries in the writer. A
//! stalled pump stops draining its cursor, the bus laps it, and the next
//! delivery it does read is a [`Delivery::Gap`] — 04 §2's "subscribers that
//! fall behind the replay buffer get an explicit `gap{from,to}` — never a
//! silent drop", reached over the wire by the same mechanism that reaches it in
//! process. Dropping frames on a full control queue instead would have made
//! loss invisible exactly where the contract says it must not be.
//!
//! # Task ownership
//!
//! **V11** owns this file, together with the reader and writer edits it needs,
//! `dispatch/{wait,events}.rs` and `crates/amx/src/cmd/events.rs`.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use amx_core::event::{StatePredicate, WaitError, Waiter};
use amx_core::{Bus, Delivery, Seq, Subscription};
use amx_proto::control::wait::{EVENT_METHOD, SubscribeReply};
use amx_proto::rpc::{Notification, RpcError};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::actor::StatusView;
use crate::conn::resume::ConnResume;
use crate::conn::writer::{OutFrame, Outbound};

/// Whether a control method is a long poll and must not be served on the
/// reader.
///
/// The two rows of §4's table that block: `wait` and `pane.wait_output`. Both
/// are documented to run until their condition holds, their timeout expires or
/// the connection dies — and the last of those is only observable by a reader
/// that is still reading, which is why they are spawned rather than awaited in
/// place.
#[must_use]
pub fn is_long_poll(method: &str) -> bool {
    method == amx_proto::control::Method::Wait.wire_name()
        || method == amx_proto::control::Method::PaneWaitOutput.wire_name()
}

#[derive(Debug, Default)]
struct Tasks {
    /// The subscription pump, if this connection has subscribed.
    pump: Option<JoinHandle<()>>,
    /// Long polls in flight, joined at connection end.
    ///
    /// Finished handles are swept whenever a new one is added, so a connection
    /// that makes ten thousand waits over its life does not accumulate ten
    /// thousand `JoinHandle`s.
    waits: Vec<JoinHandle<()>>,
}

/// One connection's event subscription, its long polls and the state they read.
#[derive(Clone, Debug)]
pub struct ConnEvents {
    bus: Arc<Bus>,
    status: StatusView,
    outbound: Outbound,
    cancel: CancellationToken,
    resume: ConnResume,
    tasks: Arc<Mutex<Tasks>>,
}

impl ConnEvents {
    /// Event state for one connection: deliveries go out through `outbound`,
    /// and everything spawned here stops with `cancel`.
    ///
    /// `resume` is what the hello presented; it is where a subscription that
    /// names no sequence of its own starts.
    #[must_use]
    pub fn new(
        bus: Arc<Bus>,
        status: StatusView,
        outbound: Outbound,
        cancel: CancellationToken,
        resume: ConnResume,
    ) -> Self {
        Self {
            bus,
            status,
            outbound,
            cancel,
            resume,
            tasks: Arc::new(Mutex::new(Tasks::default())),
        }
    }

    /// The session's event bus — what a wait subscribes to.
    #[must_use]
    pub fn bus(&self) -> &Bus {
        &self.bus
    }

    /// Every tracked pane's agent status, as `AgentHub` last wrote it.
    #[must_use]
    pub fn status(&self) -> &StatusView {
        &self.status
    }

    /// The connection's shutdown signal.
    #[must_use]
    pub fn cancel(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Register this connection for bus deliveries, replacing any earlier
    /// subscription.
    ///
    /// The reply carries the sequence the subscription was taken at, which is
    /// 04 §2's contract verbatim: an external consumer that later sees a gap
    /// "re-queries state and resumes from the returned seq without racing new
    /// transitions". The first delivery is the one after it.
    ///
    /// Subscribing twice on one connection replaces the cursor rather than
    /// running two pumps into one writer: a second cursor would interleave two
    /// orderings on a channel whose whole value is that it has one.
    ///
    /// A caller that names no `after_seq` falls back to the `last_seq` its
    /// hello resumed from, so a reattaching client is replayed the events it
    /// missed without having to repeat itself (D-M3-7). The fallback is spent
    /// once: a second bare subscription on the same connection means "from
    /// here", not "rewind to where I reconnected and replay it all again". A
    /// connection that presented no resume has nothing to fall back to and
    /// subscribes from the head, exactly as it always did.
    pub fn subscribe(&self, after_seq: Option<Seq>) -> SubscribeReply {
        let subscription = match after_seq.or_else(|| self.resume.take_after_seq()) {
            Some(seq) => self.bus.subscribe_after(seq),
            None => self.bus.subscribe(),
        };
        let seq = subscription.subscribed_at();
        let out = self.outbound.clone();
        let cancel = self.cancel.clone();
        let task = tokio::spawn(pump(subscription, out, cancel));
        if let Some(previous) = self.lock().pump.replace(task) {
            previous.abort();
        }
        SubscribeReply { seq }
    }

    /// Run `poll` on its own task, so the reader keeps reading while it waits.
    pub fn spawn_wait<F>(&self, poll: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task = tokio::spawn(poll);
        let mut tasks = self.lock();
        tasks.waits.retain(|handle| !handle.is_finished());
        tasks.waits.push(task);
    }

    /// Stop the pump and every long poll, and wait for each one.
    ///
    /// Called by the connection task after its reader and writer have finished
    /// and before the detach is published, for the same reason `ConnStreams`
    /// is: no task this connection spawned may outlive it.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let (pump, waits) = {
            let mut tasks = self.lock();
            (tasks.pump.take(), std::mem::take(&mut tasks.waits))
        };
        if let Some(pump) = pump {
            let _ = pump.await;
        }
        for wait in waits {
            let _ = wait.await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, Tasks> {
        // Every critical section is a push, a take or a swap on a small
        // struct; a panic cannot happen inside one, so poisoning carries no
        // torn state.
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Turn one connection's cursor into control-channel notifications.
///
/// A `gap` is encoded and sent exactly like an event, because it is exactly as
/// ordinary: [`Delivery`] is the wire shape, so a consumer that only handles
/// what it recognises cannot skip a gap by forgetting to look for one.
async fn pump(mut subscription: Subscription, out: Outbound, cancel: CancellationToken) {
    loop {
        let delivery = tokio::select! {
            () = cancel.cancelled() => return,
            delivery = subscription.recv() => match delivery {
                Some(delivery) => delivery,
                // The bus is gone, which happens only as the session ends.
                None => return,
            },
        };
        let Some(frame) = encode(&delivery) else {
            tracing::warn!("could not encode an event delivery; the subscription stops here");
            return;
        };
        // Waiting for room rather than failing on a full queue is what turns a
        // slow consumer into a bus gap instead of a silent loss; see the module
        // documentation.
        let sent = tokio::select! {
            () = cancel.cancelled() => return,
            sent = out.send_when_ready(frame) => sent,
        };
        if sent.is_err() {
            return;
        }
    }
}

/// One delivery as a control frame, or `None` if it could not be encoded.
fn encode(delivery: &Delivery) -> Option<OutFrame> {
    let params = serde_json::to_value(delivery).ok()?;
    let notification = Notification::new(EVENT_METHOD, Some(params));
    let payload = serde_json::to_vec(&notification).ok()?;
    OutFrame::control(payload).ok()
}

/// How a long poll ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Polled<T> {
    /// The predicate was satisfied.
    Satisfied(T),
    /// The caller's timeout expired first.
    TimedOut,
    /// The connection or the session went away.
    Cancelled,
}

/// Run `waiter` to completion, bounded by `timeout_ms` and by `cancel`.
///
/// The timeout is a parameter, never a poll interval: nothing here wakes on a
/// clock except the deadline itself, and the deadline's expiry is an answer
/// (`satisfied: false`), not a retry.
pub async fn poll<P>(
    waiter: Waiter<P>,
    timeout_ms: Option<u64>,
    cancel: &CancellationToken,
) -> Polled<P::Output>
where
    P: StatePredicate,
{
    match bounded(waiter.wait(), timeout_ms, cancel).await {
        Polled::Satisfied(Ok(value)) => Polled::Satisfied(value),
        // A closed bus means the session is going down, which is the same
        // answer to the caller as a cancelled connection: no further
        // transition can ever arrive, so there is nothing left to wait for.
        Polled::Satisfied(Err(WaitError::BusClosed | WaitError::Cancelled)) => Polled::Cancelled,
        Polled::TimedOut => Polled::TimedOut,
        Polled::Cancelled => Polled::Cancelled,
    }
}

/// The bound every long poll shares, over any future that answers like a wait.
///
/// Cancellation and the deadline are the only two things that can end a poll
/// early, and neither of them talks to anybody: after the session is cancelled
/// a long poll returns without sending a single request to a sibling actor,
/// which is R-M1-2's standing discipline applied to a task that could easily
/// have been the exception.
pub async fn bounded<T>(
    work: impl Future<Output = T>,
    timeout_ms: Option<u64>,
    cancel: &CancellationToken,
) -> Polled<T> {
    let outcome = async {
        tokio::select! {
            () = cancel.cancelled() => None,
            value = work => Some(value),
        }
    };
    let outcome = match timeout_ms {
        Some(ms) => match tokio::time::timeout(std::time::Duration::from_millis(ms), outcome).await
        {
            Ok(outcome) => outcome,
            Err(_elapsed) => return Polled::TimedOut,
        },
        None => outcome.await,
    };
    outcome.map_or(Polled::Cancelled, Polled::Satisfied)
}

/// The error a long poll answers with when it was cut short.
///
/// [`RpcError::WAIT_ABANDONED`] rather than an internal error, and the
/// difference is the whole of D-M3-7's "waits retry transparently across a
/// handoff". A handoff cancels every connection, so a standing wait is cut
/// short *by the swap itself* — and this reply frequently reaches the client
/// before the socket does close, which is a well-formed answer rather than a
/// transport failure. Read as a failure it kills a wait that was doing exactly
/// what it was asked to; read by its code it is what it is, a question nobody
/// answered, and the client re-asks it on the successor.
#[must_use]
pub fn cancelled() -> RpcError {
    RpcError::new(
        RpcError::WAIT_ABANDONED,
        "the session is shutting down; the wait was abandoned",
    )
}
