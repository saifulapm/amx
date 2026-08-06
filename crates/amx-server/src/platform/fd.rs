//! Descriptor plumbing for the pty actor: the wake pipe and the `poll()` it
//! parks in.
//!
//! The actor thread has two reasons to wake — the terminal has output (or has
//! room for input) and another thread queued work — so it parks in one
//! `poll()` over the master descriptor and the read end of a pipe. Handle
//! methods write a byte to that pipe after queueing, which is what makes a
//! queued write visible without waiting for the idle fallback timeout.

use std::io;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
#[cfg(not(target_vendor = "apple"))]
use rustix::pipe::{PipeFlags, pipe_with};

/// How large a batch the wake drain reads at a time.
///
/// The pipe carries no information beyond "something changed", so the drain
/// only has to empty it; the size is a syscall-count trade and nothing else.
const DRAIN_CHUNK: usize = 64;

/// The writing half of a wake pipe, shared by every handle.
///
/// Cloning is refcounting the same descriptor: a handle clone must wake the
/// same actor, not a copy of it.
#[derive(Clone, Debug)]
pub struct WakeWriter {
    fd: Arc<OwnedFd>,
}

impl WakeWriter {
    /// Nudge the actor out of `poll()`.
    ///
    /// A full pipe is success, not failure: the actor has at least one unread
    /// wake byte pending, so it is already going to come back round the loop.
    pub fn wake(&self) -> io::Result<()> {
        loop {
            match rustix::io::write(self.fd.as_ref(), &[1u8]) {
                Ok(_) => return Ok(()),
                Err(Errno::AGAIN) => return Ok(()),
                Err(Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}

/// A freshly created wake pipe, split into the halves that go to each side.
#[derive(Debug)]
pub struct WakePipe {
    /// The end the actor polls.
    pub reader: OwnedFd,
    /// The end handles write to.
    pub writer: WakeWriter,
}

/// Create the wake pipe.
///
/// Both ends are `CLOEXEC` so a pty child never inherits them, and both are
/// `NONBLOCK` so neither side can be parked by the other.
pub fn wake_pipe() -> io::Result<WakePipe> {
    let (reader, writer) = new_pipe()?;
    Ok(WakePipe {
        reader,
        writer: WakeWriter {
            fd: Arc::new(writer),
        },
    })
}

/// Both ends open with the flags already set — `pipe2` takes them atomically.
#[cfg(not(target_vendor = "apple"))]
fn new_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    Ok(pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?)
}

/// macOS has no `pipe2`, so the flags go on with `fcntl` calls after the
/// open. Between those calls a fork on another thread could capture a bare
/// descriptor; the server only ever execs through the pty module's spawn,
/// which runs in this same process, so the window is real but unentered here.
#[cfg(target_vendor = "apple")]
fn new_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let (reader, writer) = rustix::pipe::pipe()?;
    for fd in [&reader, &writer] {
        rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC)?;
        let flags = rustix::fs::fcntl_getfl(fd)?;
        rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    }
    Ok((reader, writer))
}

/// Empty the wake pipe.
///
/// Wakes coalesce: any number of bytes means "look at the queues again", so
/// the drain reads until the pipe is empty and reports nothing about how many
/// wakes it swallowed.
pub fn drain_wake(fd: BorrowedFd<'_>) -> io::Result<()> {
    let mut buf = [0u8; DRAIN_CHUNK];
    loop {
        match rustix::io::read(fd, &mut buf[..]) {
            Ok(0) => return Ok(()),
            Ok(_) => continue,
            Err(Errno::AGAIN) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

/// What the actor wants `poll()` to watch on the terminal.
///
/// Both false means the terminal is not watched at all — a quiesced actor with
/// nothing queued, which must still answer control commands.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Interest {
    /// Watch for output to read.
    pub read: bool,
    /// Watch for room to write.
    pub write: bool,
}

/// What `poll()` reported.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Readiness {
    /// The terminal has output, or has hung up and the read will say so.
    pub read: bool,
    /// The terminal has room for input.
    pub write: bool,
    /// A handle queued work.
    pub wake: bool,
}

/// Park until the terminal is ready, a wake arrives, or the timeout expires.
///
/// A hangup is reported as both read and write readiness rather than as an
/// error, so the caller discovers the end of the terminal through the same
/// `read`/`write` calls it uses for everything else — there is exactly one
/// place that decides what a closed pty means.
pub fn poll_pty_and_wake(
    master: BorrowedFd<'_>,
    wake: BorrowedFd<'_>,
    interest: Interest,
    timeout: Option<Duration>,
) -> io::Result<Readiness> {
    let mut events = PollFlags::empty();
    if interest.read {
        events |= PollFlags::IN;
    }
    if interest.write {
        events |= PollFlags::OUT;
    }

    let deadline = timeout.map(timespec);
    let watched = if events.is_empty() { 1 } else { 2 };
    let mut fds = [
        PollFd::from_borrowed_fd(wake, PollFlags::IN),
        PollFd::from_borrowed_fd(master, events),
    ];

    loop {
        match poll(&mut fds[..watched], deadline.as_ref()) {
            Ok(_) => break,
            // An interrupted poll restarts on the full timeout: the timeout is
            // an idle fallback, and correctness comes from the wake pipe.
            Err(Errno::INTR) => continue,
            Err(err) => return Err(err.into()),
        }
    }

    let wake_events = fds[0].revents();
    let master_events = if watched == 2 {
        fds[1].revents()
    } else {
        PollFlags::empty()
    };
    if (wake_events | master_events).contains(PollFlags::NVAL) {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "pty actor polled a closed descriptor",
        ));
    }

    let ended = PollFlags::HUP | PollFlags::ERR;
    Ok(Readiness {
        read: master_events.intersects(PollFlags::IN | ended),
        write: master_events.intersects(PollFlags::OUT | ended),
        wake: wake_events.intersects(PollFlags::IN | ended),
    })
}

/// Convert a timeout into the `timespec` `poll()` takes.
fn timespec(timeout: Duration) -> Timespec {
    Timespec {
        tv_sec: timeout.as_secs().min(i64::MAX as u64) as _,
        tv_nsec: timeout.subsec_nanos() as _,
    }
}
