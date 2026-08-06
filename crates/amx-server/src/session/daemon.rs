//! Starting a server nobody is watching: `setsid`, null stdio, then wait for
//! the socket to answer.
//!
//! 04 §1: "`amx` → probe socket; daemonize `amx server` if absent; attach."
//! Two properties make the daemonized process a daemon rather than a background
//! child:
//!
//! - it is a **session leader** (`setsid`), so the terminal that started it can
//!   close, hang up or send `SIGINT` to its own foreground group without any of
//!   it reaching the server. A daemon that outlives terminals cannot be in one.
//! - its **stdio is `/dev/null`**, so it can never write over the client that
//!   started it, and a stray `println!` cannot corrupt an attached client's
//!   alt-screen output. Diagnostics go to the session's own log, never to an
//!   inherited descriptor.
//!
//! Readiness is then the same connect probe as everywhere else — the caller
//! polls until the socket answers rather than assuming a successful spawn means
//! a bound socket, because the two are seconds apart on a cold start and there
//! is nothing else to synchronize on.

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use thiserror::Error;

use crate::session::probe::{Probe, ProbeError, probe};

/// How often [`await_ready`] re-probes the socket.
pub const READY_POLL: Duration = Duration::from_millis(5);

/// How long [`await_ready`] waits before giving up.
///
/// Generous on purpose: this covers a cold start of a process that has to open
/// a pty and bind a socket on a machine that may be swapping. The failure it
/// exists to bound is "the server is never coming", not "the server is slow".
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// A server could not be started.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// The path of the running executable could not be read.
    #[error("could not find this executable: {0}")]
    Exe(#[source] io::Error),
    /// The daemon process could not be spawned.
    #[error("could not start {}: {source}", program.display())]
    Spawn {
        /// What was being started.
        program: PathBuf,
        /// Why it could not be.
        #[source]
        source: io::Error,
    },
    /// The socket never answered.
    #[error("no server answered on {} within {timeout:?}", path.display())]
    NotReady {
        /// The socket that stayed silent.
        path: PathBuf,
        /// How long it was given.
        timeout: Duration,
    },
    /// The readiness probe itself failed.
    #[error(transparent)]
    Probe(#[from] ProbeError),
}

/// The path of the currently running executable.
///
/// The daemon is this same binary in its server role (04 §1: "one binary, two
/// roles"), so re-executing `current_exe` is what keeps a client and the server
/// it starts at the same version — starting `amx` off `$PATH` could start a
/// different build than the one the user just ran.
pub fn current_exe() -> Result<PathBuf, DaemonError> {
    std::env::current_exe().map_err(DaemonError::Exe)
}

/// Start `program args…` detached from this process's terminal.
///
/// Returns the new process's pid. The child is not waited on: it is expected to
/// outlive this process, and once this process exits it is reparented to init,
/// which reaps it. (Until then a daemon that dies early stays a zombie in this
/// process's table — visible, harmless, and gone the moment the client exits.)
pub fn spawn_detached(program: &Path, args: &[OsString]) -> Result<u32, DaemonError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: `pre_exec` runs in the forked child between `fork` and `exec`,
    // where only async-signal-safe calls are allowed. `setsid` is one (POSIX
    // lists it), the closure allocates nothing, takes no lock and touches no
    // state of this process, and its only effect is the new session the child
    // needs to stop being part of this terminal's session.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }

    command
        .spawn()
        .map(|child| child.id())
        .map_err(|source| DaemonError::Spawn {
            program: program.to_path_buf(),
            source,
        })
}

/// Poll `socket` until a server answers on it, or `timeout` elapses.
///
/// A [`Probe::Stale`] socket is not an error here and not removed: during a
/// race between two starting servers the loser's socket file may briefly exist
/// unbound, and the answer to "is anyone there yet" is simply "not yet".
pub async fn await_ready(socket: &Path, timeout: Duration) -> Result<(), DaemonError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if probe(socket)? == Probe::Running {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DaemonError::NotReady {
                path: socket.to_path_buf(),
                timeout,
            });
        }
        tokio::time::sleep(READY_POLL).await;
    }
}
