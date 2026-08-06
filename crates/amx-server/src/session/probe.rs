//! The connect probe: a socket that answers is a session that is running.
//!
//! 04 §1: "Stale-socket disambiguation by connect probe (herdr's lock-free
//! single-instance trick, kept)." The whole rule is three outcomes:
//!
//! | the probe finds | means | [`Probe`] |
//! |---|---|---|
//! | an answered hello | a server is serving | [`Probe::Running`] |
//! | `ENOENT` | no server has ever bound here | [`Probe::Absent`] |
//! | `ECONNREFUSED` | the file outlived its process | [`Probe::Stale`] |
//!
//! There is no fourth source of truth. A pid file would need its own staleness
//! rule, a lock file would need its own recovery rule, and both would have to
//! agree with the socket — which is the failure mode this trick removes rather
//! than manages.
//!
//! A successful `connect(2)` alone is not the running answer, because it only
//! proves a listening socket existed at that instant. A closed listener lives
//! on in any process that inherited a copy of its descriptor — fork copies
//! every open descriptor, and close-on-exec closes the copies only at exec —
//! and a connect in that window completes into a backlog nothing will ever
//! accept. So the probe says hello and waits: an answer is a server, and a
//! connection that dies unanswered sends the probe back to `connect(2)`,
//! where the settled path answers `ECONNREFUSED`. `Stale` is still declared
//! on `ECONNREFUSED` and nothing else: a live server's bound-but-not-yet-
//! listening socket refuses too, which is why refusal is answered downstream
//! by `bind(2)` arbitration, never by anything looser here.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_proto::frame::CONTROL_CHANNEL;
use amx_proto::{ClientInfo, FrameHeader, Hello};
use thiserror::Error;

/// How long an established connection may sit unanswered before the probe
/// takes the safe reading. Whatever holds the socket open that long without
/// hanging up is alive, and a live holder is never [`Probe::Stale`].
const ANSWER_PATIENCE: Duration = Duration::from_secs(1);

/// How many dead connections the probe chases before conceding the path is
/// churning. One inherited copy of a dead listener resolves in one retry;
/// anything still connectable after this many is being kept alive on purpose.
const ATTEMPTS: usize = 3;

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
/// Connects, says hello, and hangs up on the first answering byte: the server
/// sees a handshake whose peer leaves after the welcome, which costs one
/// accept and no state. The probe is deliberately synchronous — it is the
/// first thing `amx` does, before there is a tokio runtime to speak of.
pub fn probe(socket: &Path) -> Result<Probe, ProbeError> {
    let fail = |source: io::Error| ProbeError::Connect {
        path: socket.to_path_buf(),
        source,
    };
    for _ in 0..ATTEMPTS {
        match UnixStream::connect(socket) {
            Ok(stream) => match confirm(stream).map_err(fail)? {
                Confirmation::Answered | Confirmation::Silent => return Ok(Probe::Running),
                Confirmation::Died => {}
            },
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound => return Ok(Probe::Absent),
                io::ErrorKind::ConnectionRefused => return Ok(Probe::Stale),
                // A listener torn down mid-connect; the next attempt sees the
                // path's settled state.
                io::ErrorKind::ConnectionReset | io::ErrorKind::WouldBlock => {}
                _ => return Err(fail(err)),
            },
        }
    }
    // Still connectable after every dead connection: something keeps reviving
    // the socket, and whatever it is, it is not a corpse to bind over.
    Ok(Probe::Running)
}

/// What became of one connection the probe opened.
enum Confirmation {
    /// The server answered the hello.
    Answered,
    /// Nothing answered within [`ANSWER_PATIENCE`], but the connection held.
    Silent,
    /// The connection died unanswered: the listener went away under the probe.
    Died,
}

/// Ask an established connection for proof that a server is on it.
///
/// Writes the ordinary opening frame — a hello no server mutates anything on
/// (04 §4: a non-attach connection must never mutate the session by
/// connecting) — and waits for the first byte of the answer. Which byte it is
/// does not matter; that a peer wrote it is the proof.
fn confirm(mut stream: UnixStream) -> io::Result<Confirmation> {
    stream.set_read_timeout(Some(ANSWER_PATIENCE))?;
    stream.set_write_timeout(Some(ANSWER_PATIENCE))?;

    let hello = Hello {
        proto: amx_proto::version::window(),
        client: ClientInfo {
            name: "amx-probe".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            term: None,
        },
        ..Hello::default()
    };
    let payload = serde_json::to_vec(&hello).map_err(io::Error::other)?;
    let len = u32::try_from(payload.len()).map_err(io::Error::other)?;
    let mut frame = FrameHeader::new(len, CONTROL_CHANNEL).encode().to_vec();
    frame.extend_from_slice(&payload);
    if let Err(err) = stream.write_all(&frame) {
        return outcome_of(err);
    }

    let mut answer = [0_u8; 1];
    loop {
        return match stream.read(&mut answer) {
            Ok(0) => Ok(Confirmation::Died),
            Ok(_) => Ok(Confirmation::Answered),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => outcome_of(err),
        };
    }
}

/// Classify an I/O failure on an established probe connection.
fn outcome_of(err: io::Error) -> io::Result<Confirmation> {
    match err.kind() {
        // The timeouts set on the stream expiring: the peer holds on quietly.
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Ok(Confirmation::Silent),
        io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe => Ok(Confirmation::Died),
        _ => Err(err),
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
