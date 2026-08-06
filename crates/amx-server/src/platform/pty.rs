//! The Unix side of the [`Pty`] seam.
//!
//! D-M0-3: the four-call `openpt`/`grantpt`/`unlockpt`/`ptsname` sequence is
//! written out here rather than taken as a dependency, and the child is spawned
//! with [`std::process::Command`] plus a `pre_exec` that does setsid, TIOCSCTTY
//! and dup2 — the `login_tty` dance, and the only `unsafe` in the crate.

use std::io;
use std::os::fd::{AsFd, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Stdio};

use amx_core::platform::{PlatformError, ProcessId, Pty, PtyCommand, PtySession, WinSize};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::process::Signal;
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
