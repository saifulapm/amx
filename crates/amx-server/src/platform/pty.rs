//! The Unix side of the [`Pty`] seam.
//!
//! D-M0-3: the four-call `openpt`/`grantpt`/`unlockpt`/`ptsname` sequence is
//! written out here rather than taken as a dependency, and the child is spawned
//! with [`std::process::Command`] plus a `pre_exec` that does setsid, TIOCSCTTY
//! and dup2 — the `login_tty` dance, and one of the crate's two drops to
//! `unsafe` (the other is the darwin libproc reader in [`super::process`]).
//!
//! There are two sessions here, not one. [`UnixPtySession`] owns a terminal
//! this process opened and a child it forked; [`InheritedPtySession`] owns a
//! terminal that crossed a handoff and a child that belongs to a process which
//! has since exited (`docs/09-m3-plan.md` D-M3-12). They answer the same trait,
//! and the difference between them is entirely about what a **parent** may
//! claim: only the first can collect an exit status, and the second says so
//! rather than inventing one.

use std::io;
use std::os::fd::{AsFd, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use amx_core::platform::{PlatformError, ProcessId, Pty, PtyCommand, PtySession, WinSize};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::process::{Pid, Signal};
use rustix::pty::OpenptFlags;
use rustix::termios::Winsize;

/// The lowest descriptor number the slave may occupy before the spawn.
///
/// `dup2(fd, fd)` is a no-op that leaves `FD_CLOEXEC` set, so a slave that
/// already sat on 0, 1 or 2 would be closed by the exec it was supposed to
/// survive. Moving it above the standard descriptors removes the case.
const FIRST_FREE_FD: i32 = 3;

/// Opening pseudo-terminals on Unix.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct UnixPty;

impl Pty for UnixPty {
    type Session = UnixPtySession;

    fn spawn(&self, command: &PtyCommand) -> Result<Self::Session, PlatformError> {
        let master = open_master().map_err(io_error)?;
        rustix::pty::grantpt(&master).map_err(io_error)?;
        rustix::pty::unlockpt(&master).map_err(io_error)?;
        let name = rustix::pty::ptsname(&master, Vec::new()).map_err(io_error)?;

        // The slave is `CLOEXEC` too: the child dups it onto 0/1/2 (which
        // clears the flag on the copies) and the original then closes itself at
        // exec, so no descriptor beyond the standard three survives.
        let slave = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let slave = if slave.as_raw_fd() < FIRST_FREE_FD {
            rustix::io::fcntl_dupfd_cloexec(&slave, FIRST_FREE_FD).map_err(io_error)?
        } else {
            slave
        };

        rustix::termios::tcsetwinsize(&master, winsize(command.size)).map_err(io_error)?;
        let flags = rustix::fs::fcntl_getfl(&master).map_err(io_error)?;
        rustix::fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK).map_err(io_error)?;

        let mut process = Command::new(&command.program);
        process.args(&command.args);
        for (key, value) in &command.env {
            process.env(key, value);
        }
        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        // These are replaced by the `pre_exec` dup2s below; setting them keeps
        // the server's own descriptors out of the child if that never runs.
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // SAFETY: `pre_exec` runs in the forked child between `fork` and
        // `exec`, where only async-signal-safe work is allowed. The closure
        // makes three raw syscalls (`setsid`, `ioctl(TIOCSCTTY)`, `dup2`) and
        // allocates nothing, takes no lock, and touches no state shared with
        // the parent beyond the slave descriptor it owns.
        unsafe {
            process.pre_exec(move || attach_controlling_terminal(slave.as_fd()));
        }

        let child = process.spawn()?;
        // Dropping the command drops the `pre_exec` closure, which owns the
        // parent's copy of the slave: after this the master is the only
        // terminal descriptor left in this process.
        drop(process);

        Ok(UnixPtySession { master, child })
    }
}

/// Open the master descriptor with close-on-exec already set.
///
/// Linux takes `O_CLOEXEC` in the open itself, so the flag is on the
/// descriptor from its first instant.
#[cfg(target_os = "linux")]
fn open_master() -> Result<OwnedFd, Errno> {
    rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
}

/// Open the master descriptor with close-on-exec already set.
///
/// `posix_openpt` elsewhere (macOS among them) does not take `O_CLOEXEC`, so
/// the flag goes on with a second `fcntl` call. Between the two calls a fork
/// on another thread could capture the bare descriptor; the server confines
/// every spawn to this module's `pre_exec` path, which dups only the slave,
/// so the window is real but no code here can fall into it.
#[cfg(not(target_os = "linux"))]
fn open_master() -> Result<OwnedFd, Errno> {
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
    Ok(master)
}

/// Make the freshly forked child the session leader of its own terminal.
///
/// This is `login_tty`: a new session (so the child has no controlling
/// terminal to inherit), the slave claimed as that session's controlling
/// terminal, and the slave installed as the standard three descriptors.
fn attach_controlling_terminal(slave: BorrowedFd<'_>) -> io::Result<()> {
    rustix::process::setsid()?;
    rustix::process::ioctl_tiocsctty(slave)?;
    rustix::stdio::dup2_stdin(slave)?;
    rustix::stdio::dup2_stdout(slave)?;
    rustix::stdio::dup2_stderr(slave)?;
    Ok(())
}

/// One open pty with a child on the far end.
///
/// The session owns the only parent-side descriptor for the terminal; dropping
/// it closes the master, which is what tells the child's terminal it has gone.
#[derive(Debug)]
pub struct UnixPtySession {
    master: OwnedFd,
    child: Child,
}

impl AsFd for UnixPtySession {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }
}

impl PtySession for UnixPtySession {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, PlatformError> {
        match rustix::io::read(&self.master, buf) {
            Ok(count) => Ok(count),
            // Linux reports the last slave closing as `EIO` where a pipe would
            // report end of file; both mean the same thing to the caller.
            Err(Errno::IO) => Ok(0),
            Err(Errno::AGAIN | Errno::INTR) => Err(PlatformError::WouldBlock),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, PlatformError> {
        match rustix::io::write(&self.master, buf) {
            Ok(count) => Ok(count),
            Err(Errno::AGAIN | Errno::INTR) => Err(PlatformError::WouldBlock),
            Err(Errno::IO) => Err(PlatformError::NotFound),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }

    fn resize(&mut self, size: WinSize) -> Result<(), PlatformError> {
        rustix::termios::tcsetwinsize(&self.master, winsize(size)).map_err(|err| match err {
            Errno::IO | Errno::NOTTY => PlatformError::NotFound,
            other => PlatformError::Io(other.into()),
        })
    }

    fn child(&self) -> ProcessId {
        ProcessId(self.child.id())
    }

    fn foreground_group(&self) -> Result<ProcessId, PlatformError> {
        match rustix::termios::tcgetpgrp(&self.master) {
            Ok(pid) => Ok(ProcessId(pid.as_raw_pid().unsigned_abs())),
            Err(Errno::NOTTY | Errno::SRCH | Errno::IO) => Err(PlatformError::NotFound),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }

    fn try_wait(&mut self) -> Result<Option<Option<i32>>, PlatformError> {
        // `Child::try_wait` is `waitpid(WNOHANG)` with the reaping bookkeeping
        // already in it; calling `waitpid` directly here would race the same
        // `Child` for the status.
        Ok(self.child.try_wait()?.map(|status| status.code()))
    }

    fn kill(&mut self) -> Result<(), PlatformError> {
        // A reaped child's pid belongs to the operating system again, and this
        // signal goes to a whole group: check before signalling rather than
        // hang up whatever process inherited the number.
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        // A hangup to the whole group, not a kill to one pid: `setsid` made the
        // child a session leader, and what a closing terminal owes the
        // processes on it is SIGHUP.
        let group = rustix::process::Pid::from_child(&self.child);
        match rustix::process::kill_process_group(group, Signal::HUP) {
            Ok(()) => Ok(()),
            Err(Errno::SRCH) => Err(PlatformError::NotFound),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }
}

/// Whether the process that took a terminal over is allowed to touch it yet.
///
/// One flag per inherited pane, shared with the assembly that received the
/// descriptor. Closed until the handoff commits (§3 step 13), which is what
/// makes "panes stay quiescent until committed" a property of the descriptor
/// rather than a race the assembly wins by being quick: the pane actor is
/// quiesced a moment after it starts, and this covers the moment.
///
/// It matters because of what an abort costs. Output the successor read before
/// the commit is output the exporter can never show again — it stopped reading
/// at its quiesce and the bytes are gone from the kernel's buffer — so a
/// handoff that then aborts would have destroyed exactly what it was carrying
/// across.
#[derive(Clone, Debug, Default)]
pub struct PtyGate(Arc<AtomicBool>);

impl PtyGate {
    /// A gate that refuses everything until it is opened.
    #[must_use]
    pub fn closed() -> Self {
        Self::default()
    }

    /// Let the terminal be read and written: ownership has transferred.
    pub fn open(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether the terminal may be touched.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// A pty this process was handed rather than one it opened (D-M3-12).
///
/// The same open file description the exporter is holding — unix(7) makes an
/// SCM_RIGHTS transfer "semantically equivalent to `dup(2)` into the file
/// descriptor table of another process" — so reads resume at the byte the
/// exporter stopped at, and the terminal outlives the exporter's copy closing.
///
/// What is *not* inherited is parentage. `waitpid` is a parent's call, so
/// [`try_wait`](PtySession::try_wait) here can only ask whether the process
/// still exists and **never invents an exit code**: a dead inherited child is
/// reported as a status that could not be collected, which the pty actor turns
/// into [`ChildExit::Unknown`](crate::pty::ChildExit::Unknown). Exit
/// *detection* is unaffected, because it rides the terminal's end of file and
/// not the wait. Everything else a pane asks of its terminal — resize,
/// foreground group, hangup — is an ioctl or a signal, and neither needs
/// parentage.
#[derive(Debug)]
pub struct InheritedPtySession {
    master: OwnedFd,
    child: ProcessId,
    gate: PtyGate,
}

impl InheritedPtySession {
    /// Take over `master`, the terminal `child` is running on.
    ///
    /// The session starts closed: see [`PtyGate`].
    #[must_use]
    pub fn new(master: OwnedFd, child: ProcessId, gate: PtyGate) -> Self {
        Self {
            master,
            child,
            gate,
        }
    }

    /// Whether the child is still there, as far as a signal test can tell.
    ///
    /// `EPERM` counts as alive: the process exists, this user simply may not
    /// signal it — which cannot happen for a pane this session started, and is
    /// still not evidence of a death.
    fn alive(&self) -> bool {
        let Ok(pid) = i32::try_from(self.child.0) else {
            return false;
        };
        let Some(pid) = Pid::from_raw(pid) else {
            return false;
        };
        !matches!(rustix::process::test_kill_process(pid), Err(Errno::SRCH))
    }
}

impl AsFd for InheritedPtySession {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }
}

impl PtySession for InheritedPtySession {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, PlatformError> {
        if !self.gate.is_open() {
            return Err(PlatformError::WouldBlock);
        }
        match rustix::io::read(&self.master, buf) {
            Ok(count) => Ok(count),
            Err(Errno::IO) => Ok(0),
            Err(Errno::AGAIN | Errno::INTR) => Err(PlatformError::WouldBlock),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, PlatformError> {
        // `WouldBlock` and not a failure: the pty actor leaves an unwritten
        // chunk at the head of its queue and tries again, so a keystroke that
        // arrives during the swap is written after the commit rather than
        // dropped — the same promise a quiesce makes about queued input.
        if !self.gate.is_open() {
            return Err(PlatformError::WouldBlock);
        }
        match rustix::io::write(&self.master, buf) {
            Ok(count) => Ok(count),
            Err(Errno::AGAIN | Errno::INTR) => Err(PlatformError::WouldBlock),
            Err(Errno::IO) => Err(PlatformError::NotFound),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }

    fn resize(&mut self, size: WinSize) -> Result<(), PlatformError> {
        rustix::termios::tcsetwinsize(&self.master, winsize(size)).map_err(|err| match err {
            Errno::IO | Errno::NOTTY => PlatformError::NotFound,
            other => PlatformError::Io(other.into()),
        })
    }

    fn child(&self) -> ProcessId {
        self.child
    }

    fn foreground_group(&self) -> Result<ProcessId, PlatformError> {
        match rustix::termios::tcgetpgrp(&self.master) {
            Ok(pid) => Ok(ProcessId(pid.as_raw_pid().unsigned_abs())),
            Err(Errno::NOTTY | Errno::SRCH | Errno::IO) => Err(PlatformError::NotFound),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }

    fn try_wait(&mut self) -> Result<Option<Option<i32>>, PlatformError> {
        // Never `Some(_)`: that spelling is a *collected* status, and this
        // process has none to collect. A live child is `None` — still running,
        // which is true — and a dead one is a status this session cannot know
        // (D-M3-12). Reporting `Some(None)` for it would say "signalled",
        // which is a guess wearing an answer's clothes.
        if self.alive() {
            Ok(None)
        } else {
            Err(PlatformError::NotFound)
        }
    }

    fn kill(&mut self) -> Result<(), PlatformError> {
        // Signalling needs no parentage — but a reaped pid belongs to the
        // operating system again, and this signal goes to a whole group, so it
        // is tested first exactly as the spawned session tests its `Child`.
        if !self.alive() {
            return Ok(());
        }
        let group = i32::try_from(self.child.0)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or(PlatformError::NotFound)?;
        match rustix::process::kill_process_group(group, Signal::HUP) {
            Ok(()) => Ok(()),
            Err(Errno::SRCH) => Err(PlatformError::NotFound),
            Err(err) => Err(PlatformError::Io(err.into())),
        }
    }
}

/// The terminal size in the shape `TIOCSWINSZ` wants.
///
/// amx has no pixel geometry to report — it is a text multiplexer and the
/// client owns presentation (04 §3) — so the pixel fields stay zero.
fn winsize(size: WinSize) -> Winsize {
    Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Errors from the open sequence are all "the operating system said no".
fn io_error(err: Errno) -> PlatformError {
    PlatformError::Io(err.into())
}
