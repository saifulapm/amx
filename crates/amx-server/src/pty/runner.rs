//! The loop the actor thread runs.
//!
//! One pass is: take the control commands, take the queued input, apply the
//! collapsed resize, write what fits, then park in `poll()`. Everything the
//! terminal owes and everything it is owed passes through this one thread, so
//! ordering is a property of the loop rather than of who happened to get a lock
//! first.

use std::collections::VecDeque;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use amx_core::platform::{PlatformError, PtySession};
use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::handle::{Control, PtyActorError, SharedControls, State, lock};
use super::{ChildExit, ExitCallback, ReadCallback};
use crate::platform::fd::{Interest, drain_wake, poll_pty_and_wake};

/// How long the actor waits for a child that has just closed its terminal to
/// become collectable.
///
/// The terminal closes while the process is exiting, so the status is a moment
/// behind the end of output rather than simultaneous with it.
const EXIT_WAIT: Duration = Duration::from_secs(1);

/// How often the exit wait looks again.
const EXIT_POLL: Duration = Duration::from_millis(2);

/// Why the loop stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ending {
    /// The terminal ended: the child closed it, or it failed.
    TerminalGone,
    /// A handle told the actor to stop.
    Commanded,
}

/// The actor's own state: everything here belongs to the actor thread.
pub(super) struct Runner<S> {
    pub(super) session: S,
    pub(super) state: State,
    pub(super) input_rx: mpsc::Receiver<Bytes>,
    pub(super) control_rx: sync_mpsc::Receiver<Control>,
    pub(super) wake: OwnedFd,
    pub(super) pending: VecDeque<Bytes>,
    pub(super) offset: usize,
    pub(super) controls: Arc<Mutex<SharedControls>>,
    pub(super) response_order: Arc<Mutex<()>>,
    pub(super) accepting: Arc<AtomicBool>,
    pub(super) on_read: ReadCallback,
    pub(super) on_exit: Option<ExitCallback>,
    pub(super) idle_timeout: Duration,
    pub(super) quiesce_timeout: Duration,
    pub(super) read_buf: Vec<u8>,
    pub(super) responses: Vec<Bytes>,
}

impl<S: PtySession + AsFd> Runner<S> {
    /// Run until the terminal ends or a handle says to stop, then report.
    pub(super) fn run(&mut self) {
        let ending = self.pump();
        let exit = match ending {
            Ending::TerminalGone => self.collect_exit(),
            Ending::Commanded => ChildExit::Unknown,
        };
        if let Some(on_exit) = self.on_exit.take() {
            on_exit(exit);
        }
    }

    /// The loop proper.
    fn pump(&mut self) -> Ending {
        loop {
            if let Some(ending) = self.drain_control() {
                return ending;
            }
            self.drain_input();
            self.apply_controls();
            if !self.pending.is_empty() && !self.flush_once() {
                return Ending::TerminalGone;
            }

            let interest = Interest {
                read: self.state == State::Running,
                write: !self.pending.is_empty(),
            };
            let readiness = match poll_pty_and_wake(
                self.session.as_fd(),
                self.wake.as_fd(),
                interest,
                Some(self.idle_timeout),
            ) {
                Ok(readiness) => readiness,
                Err(err) => {
                    debug!(error = %err, "pty actor poll failed");
                    return Ending::TerminalGone;
                }
            };

            // A wake means the queues changed under us, so go round again and
            // look at them before touching the terminal.
            if readiness.wake {
                if let Err(err) = drain_wake(self.wake.as_fd()) {
                    debug!(error = %err, "pty actor wake drain failed");
                    return Ending::TerminalGone;
                }
                continue;
            }
            if interest.read && readiness.read && !self.read_once() {
                return Ending::TerminalGone;
            }
            if readiness.write && !self.pending.is_empty() && !self.flush_once() {
                return Ending::TerminalGone;
            }
        }
    }

    /// Handle every control command that has arrived.
    fn drain_control(&mut self) -> Option<Ending> {
        loop {
            match self.control_rx.try_recv() {
                Ok(command) => {
                    if let Some(ending) = self.handle_control(command) {
                        return Some(ending);
                    }
                }
                Err(sync_mpsc::TryRecvError::Empty) => return None,
                // Every handle is gone, so nothing can ask for anything again.
                Err(sync_mpsc::TryRecvError::Disconnected) => return Some(Ending::Commanded),
            }
        }
    }

    /// Handle one control command, answering on the channel it carries.
    fn handle_control(&mut self, command: Control) -> Option<Ending> {
        match command {
            Control::Quiesce(reply) => {
                let result = self.quiesce();
                let _ = reply.send(result);
            }
            Control::Resume(reply) => {
                let result = if self.state == State::Released {
                    Err(PtyActorError::Released)
                } else {
                    self.state = State::Running;
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Control::Release(reply) => {
                self.state = State::Released;
                self.pending.clear();
                self.offset = 0;
                let _ = reply.send(Ok(()));
                return Some(Ending::Commanded);
            }
            Control::ForegroundGroup(reply) => {
                let _ = reply.send(
                    self.session
                        .foreground_group()
                        .map_err(PtyActorError::Platform),
                );
            }
            Control::DupFd(reply) => {
                let _ = reply.send(self.dup_fd());
            }
            Control::Shutdown => return Some(Ending::Commanded),
        }
        None
    }

    /// Write everything already queued, then stop reading and writing.
    ///
    /// Reads keep happening while draining: the child may be blocked writing
    /// output, and a drain that refused to read would wait for a write that
    /// cannot land.
    fn quiesce(&mut self) -> Result<(), PtyActorError> {
        if self.state == State::Released {
            return Err(PtyActorError::Released);
        }
        self.drain_input();
        self.apply_controls();

        let deadline = Instant::now() + self.quiesce_timeout;
        if !self.pending.is_empty() && !self.flush_once() {
            return Err(PlatformError::NotFound.into());
        }
        while !self.pending.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PtyActorError::TimedOut);
            }
            let readiness = poll_pty_and_wake(
                self.session.as_fd(),
                self.wake.as_fd(),
                Interest {
                    read: true,
                    write: true,
                },
                Some(remaining),
            )?;
            if readiness.wake {
                drain_wake(self.wake.as_fd())?;
            }
            if readiness.read && !self.read_once() {
                return Err(PlatformError::NotFound.into());
            }
            if readiness.write && !self.flush_once() {
                return Err(PlatformError::NotFound.into());
            }
        }

        self.state = State::Quiesced;
        Ok(())
    }

    /// Hand out a duplicate of the terminal's master descriptor.
    ///
    /// Quiesced only: see [`PtyActorHandle::dup_fd`](super::PtyActorHandle::dup_fd)
    /// for why, and D-M3-3 for what the handoff does with it. Running is
    /// refused rather than obliged, because the whole value of the descriptor
    /// is that the state behind it is not moving.
    fn dup_fd(&self) -> Result<OwnedFd, PtyActorError> {
        match self.state {
            State::Quiesced => rustix::io::fcntl_dupfd_cloexec(self.session.as_fd(), 0)
                .map_err(|err| PtyActorError::Io(err.into())),
            State::Running => Err(PtyActorError::NotQuiesced),
            State::Released => Err(PtyActorError::Released),
        }
    }

    /// Move queued input into the write queue.
    ///
    /// Only a running actor takes input off the queue: what a quiesced actor
    /// leaves there is written when it resumes rather than dropped.
    fn drain_input(&mut self) {
        if self.state != State::Running {
            return;
        }
        while let Ok(bytes) = self.input_rx.try_recv() {
            self.enqueue(bytes);
        }
    }

    /// Apply the collapsed resize and take the queued replies.
    fn apply_controls(&mut self) {
        let (resize, responses) = {
            let mut controls = lock(&self.controls);
            (
                controls.resize.take(),
                std::mem::take(&mut controls.responses),
            )
        };
        if self.state == State::Released {
            return;
        }
        if let Some(request) = resize {
            if let Err(err) = self.session.resize(request.size) {
                debug!(error = %err, "pty resize failed");
            }
            self.enqueue_all(request.responses);
        }
        self.enqueue_all(responses);
    }

    /// Take one chunk of output off the terminal and let the parser see it.
    ///
    /// Returns false when the terminal has ended. The ordering lock spans the
    /// callback and the queueing of what it produced, which is what stops an
    /// out-of-band reply from landing between them.
    fn read_once(&mut self) -> bool {
        let count = match self.session.read(&mut self.read_buf) {
            Ok(0) => return false,
            Ok(count) => count,
            Err(PlatformError::WouldBlock) => return true,
            Err(err) => {
                debug!(error = %err, "pty actor read failed");
                return false;
            }
        };

        {
            let _order = lock(&self.response_order);
            self.responses.clear();
            (self.on_read)(&self.read_buf[..count], &mut self.responses);
            if !self.responses.is_empty() {
                lock(&self.controls).responses.append(&mut self.responses);
            }
        }

        // Empty by far most of the time — a `Vec::new` that never allocates —
        // because most output answers nothing.
        let queued = std::mem::take(&mut lock(&self.controls).responses);
        self.enqueue_all(queued);
        true
    }

    /// Queue bytes for the terminal.
    fn enqueue(&mut self, bytes: Bytes) {
        if !bytes.is_empty() && self.state != State::Released {
            self.pending.push_back(bytes);
        }
    }

    /// Queue a batch, keeping its order.
    fn enqueue_all(&mut self, batch: Vec<Bytes>) {
        for bytes in batch {
            self.enqueue(bytes);
        }
    }

    /// Write as much of the queue as the terminal will take right now.
    ///
    /// A partial write leaves the offset cursor inside the front chunk, so the
    /// next attempt resumes at the byte the terminal stopped at instead of
    /// repeating what it already took.
    fn flush_once(&mut self) -> bool {
        loop {
            let Some(chunk) = self.pending.front() else {
                return true;
            };
            let len = chunk.len();
            match self.session.write(&chunk[self.offset..]) {
                Ok(0) | Err(PlatformError::WouldBlock) => return true,
                Ok(written) => {
                    self.offset += written;
                    if self.offset >= len {
                        self.pending.pop_front();
                        self.offset = 0;
                    }
                }
                Err(err) => {
                    warn!(error = %err, "pty actor write failed");
                    self.pending.clear();
                    self.offset = 0;
                    return false;
                }
            }
        }
    }

    /// Collect the child's exit status now that its terminal has ended.
    fn collect_exit(&mut self) -> ChildExit {
        self.accepting.store(false, Ordering::Release);
        let deadline = Instant::now() + EXIT_WAIT;
        loop {
            match self.session.try_wait() {
                Ok(Some(Some(code))) => return ChildExit::Code(code),
                Ok(Some(None)) => return ChildExit::Signalled,
                Ok(None) => {}
                Err(err) => {
                    debug!(error = %err, "pty actor could not collect the child");
                    return ChildExit::Unknown;
                }
            }
            if Instant::now() >= deadline {
                return ChildExit::Unknown;
            }
            std::thread::sleep(EXIT_POLL);
        }
    }
}
