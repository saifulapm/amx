//! Taking the session socket, and every way that can be refused.
//!
//! 04 §1's second half: "Stale-socket disambiguation by connect probe." The
//! rules are the delicate part of this actor and they are read twice — once by
//! [`Gateway::bind`](super::Gateway::bind) and once by a retired gateway taking
//! its socket back after an aborted handoff (§3's 11–12 row) — so they live in
//! one function rather than in two that would have to agree.
//!
//! A socket file that already exists is never evidence of a running server: it
//! is probed by connecting to it, and only an answer means this session is
//! already running. Nothing is inferred from a pid file, and there is no lock to
//! go stale in turn.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use amx_core::Ctx;
use thiserror::Error;
use tokio::net::UnixListener;

use crate::session::probe;

/// Mode the session socket is created with: owner read/write only.
pub const SOCKET_MODE: u32 = 0o600;

/// Mode the session runtime directory is created with.
pub const RUNTIME_DIR_MODE: u32 = 0o700;

/// The gateway could not take the session socket.
#[derive(Debug, Error)]
pub enum GatewayError {
    /// Another server is already listening on this session's socket.
    #[error("a server is already listening on {path}")]
    AlreadyRunning {
        /// The socket that answered.
        path: PathBuf,
    },
    /// The socket file could not be probed for staleness.
    #[error("could not probe {path}: {source}")]
    Probe {
        /// The socket that was probed.
        path: PathBuf,
        /// Why the probe failed.
        source: io::Error,
    },
    /// The runtime directory could not be prepared.
    #[error("could not prepare {path}: {source}")]
    RuntimeDir {
        /// The directory.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// The socket could not be bound.
    #[error("could not bind {path}: {source}")]
    Bind {
        /// The socket path.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
}

/// Prepare the runtime directory and take the session socket.
///
/// Must be called inside a tokio runtime: the listener registers with the
/// reactor as it binds. Fails rather than stealing the socket if a live server
/// answers on it.
///
/// Stale-socket cleanup is [`probe::clear_if_stale`], inode guard included:
/// between the probe and the unlink another starting server may have replaced
/// the file with its own not-yet-listening socket, and unlinking that one would
/// strand a live server. If the guard skips the removal and this bind then loses
/// the race, the path is probed once more so the caller hears
/// [`GatewayError::AlreadyRunning`] rather than a bare bind failure.
pub(super) fn take_socket(ctx: &Ctx) -> Result<UnixListener, GatewayError> {
    prepare_dir(&ctx.runtime_dir)?;
    match probe::clear_if_stale(&ctx.socket) {
        Ok(found) if found.is_running() => {
            return Err(GatewayError::AlreadyRunning {
                path: ctx.socket.clone(),
            });
        }
        Ok(_) => {}
        Err(err) => return Err(probe_error(&ctx.socket, err)),
    }
    // Whatever survived the probe still owns the path, and the refusal must be
    // this code's, not `bind(2)`'s: Linux refuses a leftover entry with
    // `EADDRINUSE`, but darwin's `bind(2)` follows a trailing symlink and would
    // plant the socket at its target — a dangling symlink probes `Absent`
    // (connect follows it to nothing) and would slip straight through to a
    // mislocated bind. A racer that finished listening inside the window is
    // reported running, same as losing the bind race below.
    if fs::symlink_metadata(&ctx.socket).is_ok() {
        return Err(match probe::probe(&ctx.socket) {
            Ok(found) if found.is_running() => GatewayError::AlreadyRunning {
                path: ctx.socket.clone(),
            },
            _ => GatewayError::Bind {
                path: ctx.socket.clone(),
                source: io::Error::from(io::ErrorKind::AddrInUse),
            },
        });
    }
    let listener = match UnixListener::bind(&ctx.socket) {
        Ok(listener) => listener,
        Err(source) if source.kind() == io::ErrorKind::AddrInUse => {
            // Lost the bind race: whatever took the path first owns it.
            return Err(match probe::probe(&ctx.socket) {
                Ok(found) if found.is_running() => GatewayError::AlreadyRunning {
                    path: ctx.socket.clone(),
                },
                _ => GatewayError::Bind {
                    path: ctx.socket.clone(),
                    source,
                },
            });
        }
        Err(source) => {
            return Err(GatewayError::Bind {
                path: ctx.socket.clone(),
                source,
            });
        }
    };
    fs::set_permissions(&ctx.socket, fs::Permissions::from_mode(SOCKET_MODE)).map_err(
        |source| GatewayError::Bind {
            path: ctx.socket.clone(),
            source,
        },
    )?;
    Ok(listener)
}

/// Create the session's runtime directory, unreachable by another user.
fn prepare_dir(dir: &Path) -> Result<(), GatewayError> {
    fs::create_dir_all(dir).map_err(|source| GatewayError::RuntimeDir {
        path: dir.to_path_buf(),
        source,
    })?;
    fs::set_permissions(dir, fs::Permissions::from_mode(RUNTIME_DIR_MODE)).map_err(|source| {
        GatewayError::RuntimeDir {
            path: dir.to_path_buf(),
            source,
        }
    })
}

/// Flatten a probe failure into the gateway's error vocabulary.
fn probe_error(path: &Path, err: probe::ProbeError) -> GatewayError {
    let (probe::ProbeError::Connect { source, .. } | probe::ProbeError::Remove { source, .. }) =
        err;
    GatewayError::Probe {
        path: path.to_path_buf(),
        source,
    }
}
