//! What every other thread sees of a pty actor.
//!
//! A handle queues work and rings the wake pipe; it never touches the terminal
//! itself. Commands that answer carry their own reply channel, so there is no
//! correlation table and no reply that can arrive for a request nobody waits
//! on — the same rule the tokio mailboxes follow.

use std::os::fd::OwnedFd;
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
    /// The terminal is still moving, so it cannot be handed over.
    ///
    /// Only [`PtyActorHandle::dup_fd`] answers with this: a descriptor taken
    /// while the actor is still reading and writing would let the process
    /// taking it over see a terminal whose state moves under it, which is the
    /// whole reason [`State::Quiesced`] exists.
    #[error("pty actor is not quiesced")]
    NotQuiesced,
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
    DupFd(sync_mpsc::Sender<Result<OwnedFd, PtyActorError>>),
    Shutdown,
}

/// A handle on one pty actor.
///
/// Cloning gives another way to reach the same actor, not another actor.
#[derive(Clone, Debug)]
pub struct PtyActorHandle {
    input: mpsc::Sender<Bytes>,
    /// Serializes the *placing* of input chunks, so a pair cannot be split.
    ///
    /// A pane has more than one producer for this queue — a drive on the
    /// parser thread, and the connection forwarding a human's keystrokes — and
    /// reserving two slots does not stop a third party's send from landing
    /// between the two. Nothing else gives adjacency, so this does.
    ///
    /// Capacity is reserved outside the lock and the send inside it is a push
    /// that cannot block, so the critical section is a few instructions and
    /// never spans an await.
    input_order: Arc<Mutex<()>>,
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
            input_order: Arc::new(Mutex::new(())),
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
        {
            let _order = lock(&self.input_order);
            permit.send(bytes);
        }
        self.wake()
    }

    /// Queue input without waiting for room.
    pub fn try_write_input(&self, bytes: Bytes) -> Result<(), PtyActorError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(PtyActorError::NotAccepting);
        }
        // Reserve then send, rather than `try_send`, so the send is the part
        // under `input_order` — same semantics, and one placing discipline.
        let slot = self.input.try_reserve().map_err(reserve_error)?;
        {
            let _order = lock(&self.input_order);
            slot.send(bytes);
        }
        self.wake()
    }

    /// Queue two chunks back to back, without waiting for room.
    ///
    /// The queue is drained in order and one chunk is one `write()` to the
    /// terminal, so this is how a caller asks for two *separate* writes that
    /// still cannot be separated from each other. Both are placed under
    /// `input_order`, which is the only thing that makes them adjacent:
    /// reserving two slots bounds the capacity, not the interleaving, and this
    /// queue has more than one producer.
    ///
    /// Both slots are reserved before either send, so a queue with room for
    /// one chunk refuses the pair whole. Sending the first and failing the
    /// second would leave the child holding half of something — for
    /// `pane.run`, text with no submit, which is the exact state this pairing
    /// exists to prevent.
    pub fn try_write_input_pair(&self, first: Bytes, second: Bytes) -> Result<(), PtyActorError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(PtyActorError::NotAccepting);
        }
        // `?` on the second reservation drops the first, handing its slot back.
        let first_slot = self.input.try_reserve().map_err(reserve_error)?;
        let second_slot = self.input.try_reserve().map_err(reserve_error)?;
        {
            let _order = lock(&self.input_order);
            first_slot.send(first);
            second_slot.send(second);
        }
        self.wake()
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

    /// Duplicate the terminal's master descriptor, for a quiesced actor only.
    ///
    /// The missing piece of the quiesce/release machinery (`docs/09-m3-plan.md`
    /// D-M3-3): handoff needs the master fd *out*, and until now nothing could
    /// take it — the actor owns its session until its thread drops it.
    ///
    /// Three properties, each deliberate:
    ///
    /// - **Answered on the actor thread**, like every other command that
    ///   touches the terminal, so the descriptor leaves under the same
    ///   serialization as reads and writes rather than beside it.
    /// - **Quiesced only.** In [`State::Running`] the answer is
    ///   [`PtyActorError::NotQuiesced`] and in [`State::Released`] it is
    ///   [`PtyActorError::Released`]. A descriptor taken from a running actor
    ///   would hand the receiver a terminal whose state moves under it.
    /// - **`fcntl(F_DUPFD_CLOEXEC)`**, so the copy does not travel into
    ///   anything this process later spawns. It is the same open file
    ///   description either way — unix(7): an SCM_RIGHTS transfer is
    ///   "semantically equivalent to `dup(2)` into the file descriptor table of
    ///   another process" — so the pty stays alive exactly as long as some
    ///   reference to it does, and the exporter dropping its own copies after
    ///   the swap costs the child nothing.
    ///
    /// # Errors
    ///
    /// [`PtyActorError::NotQuiesced`] or [`PtyActorError::Released`] for the
    /// wrong state, [`PtyActorError::Io`] if the duplication itself fails, and
    /// the usual [`Gone`](PtyActorError::Gone)/[`TimedOut`](PtyActorError::TimedOut)
    /// if the actor cannot answer.
    pub fn dup_fd(&self) -> Result<OwnedFd, PtyActorError> {
        self.request(Control::DupFd, REPLY_TIMEOUT)?
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

/// Why a slot could not be reserved, in the same terms a send failure uses.
fn reserve_error(err: mpsc::error::TrySendError<()>) -> PtyActorError {
    match err {
        mpsc::error::TrySendError::Full(()) => PtyActorError::QueueFull,
        mpsc::error::TrySendError::Closed(()) => PtyActorError::Gone,
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
