//! The connection writer: strict channel priority (04 §4).
//!
//! "The connection writer has strict channel priority (control > grid deltas >
//! history/bulk), and history transfers are chunked — a big scrollback fetch can
//! never head-of-line-block a keystroke's response."
//!
//! Strict is the operative word. There is no weighting and no ageing: whenever
//! the writer picks its next frame it takes the lowest
//! [`Priority`](amx_proto::stream::Priority) class that has anything queued, so
//! a control reply enqueued behind a megabyte of history chunks still goes out
//! ahead of every one of them. That only holds if enqueueing is *not* what
//! orders the wire, which is why producers push into per-class queues here
//! rather than into a single channel the writer drains in arrival order.
//!
//! Enqueueing is synchronous ([`Outbound::send`]) so an actor loop never awaits
//! to hand a frame off. Each class is depth-capped instead: a producer that
//! outruns the socket is told so ([`OutboundError::Full`]) rather than growing
//! an unbounded backlog, and bulk producers can wait for room with
//! [`Outbound::send_when_ready`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use amx_proto::frame::{CONTROL_CHANNEL, MAX_CONTROL_FRAME};
use amx_proto::stream::Priority;
use amx_proto::{FrameError, FrameHeader};
use bytes::Bytes;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

/// How many control frames may be queued before a producer is refused.
///
/// Control traffic is bounded by the number of calls in flight, so a full
/// control queue means the peer has stopped reading entirely; the connection is
/// dropped rather than buffered for.
pub const CONTROL_DEPTH: usize = 256;

/// How many grid frames may be queued before a producer is refused.
///
/// Deliberately shallow: 04 §4 has damage coalesce into a per-client dirty set
/// rather than into a queue of deltas, so a deep grid queue would be a second,
/// worse copy of that mechanism.
pub const GRID_DEPTH: usize = 32;

/// How many bulk frames may be queued before a producer is refused.
pub const BULK_DEPTH: usize = 256;

/// One frame on its way out, with the class it is written at.
///
/// The length is validated at construction and kept as the [`FrameHeader`] that
/// will be written, so the writer itself has no failure mode between popping a
/// frame and putting it on the wire.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OutFrame {
    header: FrameHeader,
    priority: Priority,
    payload: Bytes,
}

impl OutFrame {
    /// A control-channel frame.
    ///
    /// Enforces [`MAX_CONTROL_FRAME`] on the way out as well as on the way in:
    /// a reply that would exceed the cap must become an error reply, never a
    /// frame the peer is obliged to reject.
    pub fn control(payload: impl Into<Bytes>) -> Result<Self, OutboundError> {
        let payload = payload.into();
        let len = checked_len(payload.len(), MAX_CONTROL_FRAME)?;
        Ok(Self {
            header: FrameHeader::new(len, CONTROL_CHANNEL),
            priority: Priority::Control,
            payload,
        })
    }

    /// A binary stream frame on `channel`, written at `priority`.
    ///
    /// `cap` is the frame cap negotiated for that stream
    /// ([`FlowControl::StreamCap`](amx_proto::stream::FlowControl::StreamCap)).
    pub fn stream(
        channel: u8,
        priority: Priority,
        cap: u32,
        payload: impl Into<Bytes>,
    ) -> Result<Self, OutboundError> {
        if channel == CONTROL_CHANNEL {
            return Err(OutboundError::ControlChannelReserved);
        }
        let payload = payload.into();
        let len = checked_len(payload.len(), cap as usize)?;
        Ok(Self {
            header: FrameHeader::new(len, channel),
            priority,
            payload,
        })
    }

    /// The header that will precede this frame's payload.
    #[must_use]
    pub const fn header(&self) -> FrameHeader {
        self.header
    }

    /// The class this frame is written at.
    #[must_use]
    pub const fn priority(&self) -> Priority {
        self.priority
    }

    /// The payload bytes.
    #[must_use]
    pub const fn payload(&self) -> &Bytes {
        &self.payload
    }
}

fn checked_len(len: usize, cap: usize) -> Result<u32, OutboundError> {
    if len > cap {
        return Err(OutboundError::TooLarge { len, cap });
    }
    u32::try_from(len).map_err(|_| OutboundError::TooLarge { len, cap })
}

/// A frame could not be queued.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum OutboundError {
    /// The payload is larger than the cap that applies to its channel.
    #[error("frame payload of {len} bytes exceeds the {cap} byte cap for its channel")]
    TooLarge {
        /// Payload length.
        len: usize,
        /// The cap that applies.
        cap: usize,
    },
    /// A binary stream frame named channel 0, which is always control.
    #[error("channel 0 is reserved for control")]
    ControlChannelReserved,
    /// That priority class is at its depth cap.
    #[error("the {priority:?} queue is full")]
    Full {
        /// The class that was full.
        priority: Priority,
    },
    /// The writer has stopped.
    #[error("the connection writer is gone")]
    Closed,
}

/// The three queues, strictly ordered.
#[derive(Debug, Default)]
struct Queues {
    control: VecDeque<OutFrame>,
    grid: VecDeque<OutFrame>,
    bulk: VecDeque<OutFrame>,
}

impl Queues {
    fn queue_mut(&mut self, priority: Priority) -> (&mut VecDeque<OutFrame>, usize) {
        match priority {
            Priority::Control => (&mut self.control, CONTROL_DEPTH),
            Priority::Grid => (&mut self.grid, GRID_DEPTH),
            Priority::Bulk => (&mut self.bulk, BULK_DEPTH),
        }
    }

    /// Take the next frame: control, then grid, then bulk. Never anything else.
    fn pop(&mut self) -> Option<OutFrame> {
        self.control
            .pop_front()
            .or_else(|| self.grid.pop_front())
            .or_else(|| self.bulk.pop_front())
    }

    fn len(&self, priority: Priority) -> usize {
        match priority {
            Priority::Control => self.control.len(),
            Priority::Grid => self.grid.len(),
            Priority::Bulk => self.bulk.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.control.is_empty() && self.grid.is_empty() && self.bulk.is_empty()
    }
}

#[derive(Debug)]
struct Shared {
    queues: Mutex<Queues>,
    /// Signalled after every pop, so a producer waiting for room wakes up.
    drained: Notify,
    closed: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queues> {
        // A panic can never happen while this lock is held — every critical
        // section is a push, a pop or a length read on a `VecDeque` — so
        // poisoning carries no torn state and the inner value is safe to take.
        self.queues.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The producer handle: cloneable, and enqueueing never awaits.
#[derive(Clone, Debug)]
pub struct Outbound {
    shared: Arc<Shared>,
    wake: mpsc::Sender<()>,
}

/// The consumer side, handed to [`run`].
#[derive(Debug)]
pub struct WriterQueue {
    shared: Arc<Shared>,
    wake: mpsc::Receiver<()>,
}

/// Build a writer queue and its producer handle.
#[must_use]
pub fn channel() -> (Outbound, WriterQueue) {
    let shared = Arc::new(Shared {
        queues: Mutex::new(Queues::default()),
        drained: Notify::new(),
        closed: AtomicBool::new(false),
    });
    // Capacity 1: the permit means "there may be work", not "there is one
    // frame", so a lost `try_send` against a full channel loses nothing — the
    // permit already queued will wake the writer, which then drains everything.
    let (wake, rx) = mpsc::channel(1);
    (
        Outbound {
            shared: Arc::clone(&shared),
            wake,
        },
        WriterQueue { shared, wake: rx },
    )
}

impl Outbound {
    /// Queue `frame` for its priority class. Never blocks.
    pub fn send(&self, frame: OutFrame) -> Result<(), OutboundError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(OutboundError::Closed);
        }
        {
            let mut queues = self.shared.lock();
            let priority = frame.priority();
            let (queue, depth) = queues.queue_mut(priority);
            if queue.len() >= depth {
                return Err(OutboundError::Full { priority });
            }
            queue.push_back(frame);
        }
        let _ = self.wake.try_send(());
        Ok(())
    }

    /// Queue `frame`, waiting for room in its class if it is full.
    ///
    /// This is what a chunked bulk transfer uses: history is produced faster
    /// than a socket drains it, and the transfer must slow down rather than
    /// either buffering without bound or dropping rows.
    pub async fn send_when_ready(&self, frame: OutFrame) -> Result<(), OutboundError> {
        loop {
            // Register for the wakeup *before* attempting the send, so a pop
            // that happens between the failed send and the await is not missed.
            let waiting = self.shared.drained.notified();
            tokio::pin!(waiting);
            waiting.as_mut().enable();

            match self.send(frame.clone()) {
                Err(OutboundError::Full { .. }) => waiting.await,
                other => return other,
            }
        }
    }

    /// How many frames are queued in `priority`.
    #[must_use]
    pub fn depth(&self, priority: Priority) -> usize {
        self.shared.lock().len(priority)
    }

    /// Whether every queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shared.lock().is_empty()
    }

    /// Whether the writer has stopped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }
}

impl WriterQueue {
    fn pop(&self) -> Option<OutFrame> {
        let frame = self.shared.lock().pop();
        if frame.is_some() {
            self.shared.drained.notify_waiters();
        }
        frame
    }

    fn is_empty(&self) -> bool {
        self.shared.lock().is_empty()
    }

    fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.drained.notify_waiters();
    }
}

/// What the writer put on the wire before it stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WriterReport {
    /// Frames written.
    pub frames: u64,
    /// Payload bytes written, excluding headers.
    pub bytes: u64,
}

/// A frame could not be written.
#[derive(Debug, Error)]
pub enum WriteError {
    /// The socket failed.
    #[error("write failed: {0}")]
    Io(#[from] std::io::Error),
    /// A frame was rejected at the framing layer.
    #[error(transparent)]
    Frame(#[from] FrameError),
}

/// Drain `queue` into `sink`, strictly highest-priority-first, until `cancel`
/// fires, every [`Outbound`] is dropped, or the socket fails.
///
/// Header and payload are written from the frame's own buffers: nothing is
/// concatenated into a scratch buffer first, so a steady stream of frames costs
/// no allocation at all on this path.
pub async fn run<W>(
    mut sink: W,
    mut queue: WriterQueue,
    cancel: CancellationToken,
) -> Result<WriterReport, WriteError>
where
    W: AsyncWrite + Unpin,
{
    let mut report = WriterReport::default();
    let outcome = loop {
        let Some(frame) = queue.pop() else {
            let woken = tokio::select! {
                () = cancel.cancelled() => break Ok(()),
                woken = queue.wake.recv() => woken,
            };
            // `None` means every producer is gone. Loop once more so anything
            // queued before the last one dropped is still written, then stop.
            if woken.is_none() && queue.is_empty() {
                break Ok(());
            }
            continue;
        };

        if let Err(err) = sink.write_all(&frame.header().encode()).await {
            break Err(WriteError::Io(err));
        }
        if !frame.payload().is_empty()
            && let Err(err) = sink.write_all(frame.payload()).await
        {
            break Err(WriteError::Io(err));
        }
        report.frames += 1;
        report.bytes += u64::from(frame.header().len);
    };
    queue.close();
    outcome?;
    sink.flush().await?;
    Ok(report)
}
