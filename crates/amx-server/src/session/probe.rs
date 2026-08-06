//! The connect probe: a socket that answers is a session that is running.
//!
//! 04 §1: "Stale-socket disambiguation by connect probe (herdr's lock-free
//! single-instance trick, kept)." The whole rule is three outcomes of one
//! `connect(2)`:
//!
//! | connect | means | [`Probe`] |
//! |---|---|---|
//! | succeeds | a server is listening | [`Probe::Running`] |
//! | `ENOENT` | no server has ever bound here | [`Probe::Absent`] |
//! | `ECONNREFUSED` | the file outlived its process | [`Probe::Stale`] |
//!
//! There is no fourth source of truth. A pid file would need its own staleness
//! rule, a lock file would need its own recovery rule, and both would have to
//! agree with the socket — which is the failure mode this trick removes rather
//! than manages.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// What a connect probe found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    /// Nothing is at the path: this session has never run, or was deleted.
    Absent,
    /// The socket file is there and nothing answers on it: its server died
    /// without cleaning up. Binding over it is safe.
    Stale,
    /// A server answered. This session is running.
    Running,
}

impl Probe {
    /// Whether a server answered.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether binding this socket would be safe.
    #[must_use]
    pub const fn is_free(self) -> bool {
        matches!(self, Self::Absent | Self::Stale)
    }
}

/// A socket could not be probed.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// `connect(2)` failed for a reason that is neither "nothing there" nor
    /// "nobody listening" — a permissions failure, most likely, which must not
    /// be read as "free to bind over".
    #[error("could not probe {path}: {source}")]
    Connect {
        /// The socket that was probed.
        path: PathBuf,
        /// Why the probe failed.
        #[source]
        source: io::Error,
    },
    /// A stale socket file could not be removed.
    #[error("could not remove the stale socket {path}: {source}")]
    Remove {
        /// The socket that could not be removed.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
}

/// Probe `socket`.
///
/// Connects and immediately disconnects: the server sees a peer that closes
/// before saying hello and drops it, which costs one accept and no state. The
/// probe is deliberately synchronous — it is the first thing `amx` does, before
/// there is a tokio runtime to speak of, and a `connect` to a Unix socket
/// either completes or fails immediately.
pub fn probe(socket: &Path) -> Result<Probe, ProbeError> {
    match UnixStream::connect(socket) {
        Ok(_answered) => Ok(Probe::Running),
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound => Ok(Probe::Absent),
            io::ErrorKind::ConnectionRefused => Ok(Probe::Stale),
            _ => Err(ProbeError::Connect {
                path: socket.to_path_buf(),
                source: err,
            }),
        },
    }
}

/// Probe `socket` and remove the file if it turns out to be stale.
///
/// Removal is guarded by the socket's identity, not just its path. Between the
/// probe and the unlink another starting server may have replaced the file with
/// its own — a socket is reachable only after `listen(2)`, so there is a window
/// in which a *live* server's socket refuses connections — and unlinking that
/// one would leave a running server nobody can reach. So the inode is compared
/// before and after: if it changed, the file is not ours to remove and the
/// result is reported as [`Probe::Stale`] without touching it. The caller's
/// next step is to start a server anyway, and `bind` is the arbiter that
/// decides which of the two survives.
pub fn clear_if_stale(socket: &Path) -> Result<Probe, ProbeError> {
    let before = match fs::symlink_metadata(socket) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Probe::Absent),
        Err(source) => {
            return Err(ProbeError::Connect {
                path: socket.to_path_buf(),
                source,
            });
        }
    };

    let found = probe(socket)?;
    if found != Probe::Stale {
        return Ok(found);
    }

    match fs::symlink_metadata(socket) {
        Ok(after) if after.dev() == before.dev() && after.ino() == before.ino() => {
            fs::remove_file(socket).map_err(|source| ProbeError::Remove {
                path: socket.to_path_buf(),
                source,
            })?;
        }
        // Replaced or already gone: someone else is mid-bind on this path, and
        // whatever is there now is not the file this probe refused.
        _ => {}
    }
    Ok(Probe::Stale)
}

/// Connect to `socket` and ask the kernel for the listening process's pid.
///
/// `SO_PEERCRED` and its equivalents answer "who is on the other end of this
/// connection" from the kernel's own bookkeeping, so the pid cannot be stale or
/// forged the way a pid file's contents can: it belongs to the process that is
/// holding this socket open right now. `Ok(None)` means the connection was made
/// but the platform did not supply a pid.
pub async fn server_pid(socket: &Path) -> Result<Option<u32>, ProbeError> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|source| ProbeError::Connect {
            path: socket.to_path_buf(),
            source,
        })?;
    let cred = stream.peer_cred().map_err(|source| ProbeError::Connect {
        path: socket.to_path_buf(),
        source,
    })?;
    Ok(cred.pid().and_then(|pid| u32::try_from(pid).ok()))
}
