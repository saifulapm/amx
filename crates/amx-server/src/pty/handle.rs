//! What every other thread sees of a pty actor.
//!
//! A handle queues work and rings the wake pipe; it never touches the terminal
//! itself. Commands that answer carry their own reply channel, so there is no
//! correlation table and no reply that can arrive for a request nobody waits
//! on — the same rule the tokio mailboxes follow.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use amx_core::platform::{PlatformError, ProcessId, WinSize};
use bytes::Bytes;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::platform::fd::WakeWriter;

/// How long a handle waits for a control command that is not a quiesce.
const REPLY_TIMEOUT: Duration = Duration::from_secs(1);

/// Extra grace on top of the actor's own quiesce budget.
///
/// The actor is allowed the whole drain timeout, so the caller has to be
/// willing to wait for it plus the round trip, or it would report a timeout
/// for a quiesce that was about to succeed.
const QUIESCE_GRACE: Duration = Duration::from_secs(1);

/// A pty actor call did not do what was asked.
#[derive(Debug, Error)]
pub enum PtyActorError {
    /// The actor is not accepting input: it is quiesced or released.
    #[error("pty actor is not accepting input")]
    NotAccepting,
    /// The input queue is full; the terminal is slower than the writer.
    #[error("pty actor input queue is full")]
    QueueFull,
    /// The actor thread is gone.
    #[error("pty actor is gone")]
    Gone,
    /// The actor did not answer in time.
    #[error("timed out waiting for the pty actor")]
    TimedOut,
    /// The actor released its terminal and will not act again.
    #[error("pty actor was released")]
    Released,
    /// The terminal itself reported the failure.
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// The wake pipe or the actor thread could not be set up.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What the actor is doing with its terminal.
///
/// `Quiesced` is the state M3's descriptor handoff lands in: the terminal is
/// still owned and still open, but nothing is being read from it or written to
/// it, so its state cannot move under the process taking it over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum State {
    /// Reading, writing, and accepting input.
    Running,
    /// Holding the terminal still.
    Quiesced,
    /// Done with the terminal for good.
    Released,
}

/// A resize, plus the replies the terminal owes once it has been applied.
#[derive(Debug)]
pub(crate) struct ResizeRequest {
    pub(crate) size: WinSize,
    pub(crate) responses: Vec<Bytes>,
}

/// State shared between the handles and the actor.
///
/// Resizes collapse — only the latest size matters — while replies queue, so
/// they live together under one lock that is taken briefly and never across a
/// syscall.
#[derive(Debug, Default)]
pub(crate) struct SharedControls {
    pub(crate) resize: Option<ResizeRequest>,
    pub(crate) responses: Vec<Bytes>,
}

/// A control command, with the channel its answer goes back on.
pub(crate) enum Control {
    Quiesce(sync_mpsc::Sender<Result<(), PtyActorError>>),
    Resume(sync_mpsc::Sender<Result<(), PtyActorError>>),
    Release(sync_mpsc::Sender<Result<(), PtyActorError>>),
    ForegroundGroup(sync_mpsc::Sender<Result<ProcessId, PtyActorError>>),
    Shutdown,
}

/// A handle on one pty actor.
///
/// Cloning gives another way to reach the same actor, not another actor.
#[derive(Clone, Debug)]
pub struct PtyActorHandle {
    input: mpsc::Sender<Bytes>,
    control: sync_mpsc::Sender<Control>,
    wake: WakeWriter,
    controls: Arc<Mutex<SharedControls>>,
    response_order: Arc<Mutex<()>>,
    accepting: Arc<AtomicBool>,
    quiesce_timeout: Duration,
}

impl PtyActorHandle {
    /// Assemble a handle from the halves the actor kept the other end of.
    #[expect(
        clippy::too_many_arguments,
        reason = "one call site, in `PtyActor::spawn`"
    )]
    pub(crate) fn new(
        input: mpsc::Sender<Bytes>,
        control: sync_mpsc::Sender<Control>,
        wake: WakeWriter,
        controls: Arc<Mutex<SharedControls>>,
        response_order: Arc<Mutex<()>>,
        accepting: Arc<AtomicBool>,
        quiesce_timeout: Duration,
    ) -> Self {
        Self {
            input,
            control,
            wake,
            controls,
            response_order,
            accepting,
            quiesce_timeout,
        }
    }

    /// Queue input for the terminal, waiting if the queue is full.
    pub async fn write_input(&self, bytes: Bytes) -> Result<(), PtyActorError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(PtyActorError::NotAccepting);
        }
        let permit = self
            .input
            .reserve()
            .await
            .map_err(|_| PtyActorError::Gone)?;
        if !self.accepting.load(Ordering::Acquire) {
            return Err(PtyActorError::NotAccepting);
        }
        permit.send(bytes);
        self.wake()
    }

    /// Queue input without waiting for room.
    pub fn try_write_input(&self, bytes: Bytes) -> Result<(), PtyActorError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(PtyActorError::NotAccepting);
        }
        match self.input.try_send(bytes) {
            Ok(()) => self.wake(),
            Err(mpsc::error::TrySendError::Full(_)) => Err(PtyActorError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(PtyActorError::Gone),
        }
    }

    /// Hand the terminal a reply the parser produced out of band.
    ///
    /// The reply is *produced* under the ordering lock, not merely queued under
    /// it: an answer computed while the read callback is running is an answer
    /// to a question the child asked earlier, and it has to queue behind the
    /// replies that read is producing.
    pub fn write_terminal_response(
        &self,
        response: impl FnOnce() -> Option<Bytes>,
    ) -> Result<(), PtyActorError> {
        let queued = {
            let _order = lock(&self.response_order);
            match response() {
                Some(bytes) if !bytes.is_empty() => {
                    lock(&self.controls).responses.push(bytes);
                    true
                }
                _ => false,
            }
        };
        if queued { self.wake() } else { Ok(()) }
    }

    /// Resize the terminal, then send `responses`.
    ///
    /// The replies travel with the resize because they describe the size the
    /// child is about to be told about; splitting them would let the child read
    /// an answer about a geometry it has not been given yet.
    pub fn resize(&self, size: WinSize, responses: Vec<Bytes>) -> Result<(), PtyActorError> {
        lock(&self.controls).resize = Some(ResizeRequest { size, responses });
        self.wake()
    }

    /// Ask which process group is in the foreground of the terminal.
    pub fn foreground_group(&self) -> Result<ProcessId, PtyActorError> {
        self.request(Control::ForegroundGroup, REPLY_TIMEOUT)?
    }

    /// Stop reading and writing, once everything already queued has been
    /// written.
    ///
    /// Input queued after this stays queued: a quiesce that discarded a
    /// keystroke would be a data-loss bug wearing a state machine's clothes.
    pub fn quiesce(&self) -> Result<(), PtyActorError> {
        self.accepting.store(false, Ordering::Release);
        let result = self.request(Control::Quiesce, self.quiesce_timeout + QUIESCE_GRACE)?;
        if result.is_err() {
            self.accepting.store(true, Ordering::Release);
        }
        result
    }

    /// Undo a quiesce.
    pub fn resume(&self) -> Result<(), PtyActorError> {
        let result = self.request(Control::Resume, REPLY_TIMEOUT)?;
        if result.is_ok() {
            self.accepting.store(true, Ordering::Release);
        }
        result
    }

    /// Give up the terminal for good and stop the actor.
    pub fn release(&self) -> Result<(), PtyActorError> {
        self.accepting.store(false, Ordering::Release);
        self.request(Control::Release, REPLY_TIMEOUT)?
    }

    /// Stop the actor without waiting for an answer.
    pub fn shutdown(&self) {
        self.accepting.store(false, Ordering::Release);
        if self.control.send(Control::Shutdown).is_ok() {
            let _ = self.wake();
        }
    }

    /// Send a command that answers and wait for its reply.
    ///
    /// The outer result is whether the actor answered; the inner one is what it
    /// said.
    fn request<T>(
        &self,
        command: impl FnOnce(sync_mpsc::Sender<Result<T, PtyActorError>>) -> Control,
        timeout: Duration,
    ) -> Result<Result<T, PtyActorError>, PtyActorError> {
        let (reply, answer) = sync_mpsc::channel();
        self.control
            .send(command(reply))
            .map_err(|_| PtyActorError::Gone)?;
        self.wake()?;
        match answer.recv_timeout(timeout) {
            Ok(result) => Ok(result),
            Err(sync_mpsc::RecvTimeoutError::Timeout) => Err(PtyActorError::TimedOut),
            Err(sync_mpsc::RecvTimeoutError::Disconnected) => Err(PtyActorError::Gone),
        }
    }

    /// Nudge the actor out of `poll()`.
    fn wake(&self) -> Result<(), PtyActorError> {
        self.wake.wake().map_err(PtyActorError::Io)
    }
}

/// Take a lock, treating a poisoned one as held.
///
/// A panic on the other side of one of these locks tells us nothing about the
/// bytes they guard: the queue is still a queue. Refusing to look at it would
/// turn one thread's panic into a wedged pane.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
