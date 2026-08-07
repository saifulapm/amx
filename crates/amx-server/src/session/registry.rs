//! Enumerating and managing the sessions on this machine (04 §1).
//!
//! "A daemon that outlives terminals must be discoverable and stoppable from
//! the CLI." Discoverable is [`list`]: walk the runtime root, probe every
//! session directory in it, and report the ones that answer — a directory is
//! evidence that a session *ran*, never that it is running. Stoppable is
//! [`stop`]: ask the kernel which process holds the other end of the socket and
//! send it a `SIGTERM`, which the server turns into the same cancellation a
//! test triggers directly.
//!
//! [`delete`] is the third verb and the destructive one, so it refuses a
//! running session outright rather than racing it: removing a live server's
//! runtime directory would leave a process holding an unreachable socket, which
//! is precisely the state the connect probe exists to make impossible.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_core::{Ctx, CtxError, Env, SessionName};
use rustix::process::{Pid, Signal};
use thiserror::Error;

use crate::session::probe::{ProbeError, clear_if_stale, probe, server_pid};

/// How often [`stop`] re-probes while waiting for the server to go.
const STOP_POLL: Duration = Duration::from_millis(5);

/// A session with a server answering on its socket.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Running {
    /// The session's name.
    pub session: SessionName,
    /// Its runtime directory.
    pub runtime_dir: PathBuf,
    /// Its socket, which answered a connect probe just now.
    pub socket: PathBuf,
}

/// Sessions could not be enumerated.
#[derive(Debug, Error)]
pub enum ListError {
    /// The environment does not describe a runtime directory.
    #[error(transparent)]
    Ctx(#[from] CtxError),
    /// The runtime root could not be read.
    #[error("could not read {}: {source}", path.display())]
    Read {
        /// The directory.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// A session's socket could not be probed.
    #[error(transparent)]
    Probe(#[from] ProbeError),
}

/// A session could not be stopped.
#[derive(Debug, Error)]
pub enum StopError {
    /// The socket could not be probed.
    #[error(transparent)]
    Probe(#[from] ProbeError),
    /// The socket answered but the platform would not say which process owns
    /// it, so there is nothing to signal.
    #[error("a server answered on {} but its pid could not be read", path.display())]
    UnknownPeer {
        /// The socket that answered.
        path: PathBuf,
    },
    /// The signal could not be delivered.
    #[error("could not signal pid {pid}: {source}")]
    Signal {
        /// The process that was to be signalled.
        pid: u32,
        /// Why it could not be.
        #[source]
        source: io::Error,
    },
    /// The server was still answering when the deadline passed.
    #[error("the server on {} was still running after {timeout:?}", path.display())]
    StillRunning {
        /// The socket that kept answering.
        path: PathBuf,
        /// How long it was given.
        timeout: Duration,
    },
}

/// What [`stop`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StopOutcome {
    /// The server was signalled and is gone.
    Stopped {
        /// The process that was stopped.
        pid: u32,
    },
    /// Nothing was answering: there was nothing to stop.
    NotRunning,
}

/// A session could not be deleted.
#[derive(Debug, Error)]
pub enum DeleteError {
    /// The socket could not be probed.
    #[error(transparent)]
    Probe(#[from] ProbeError),
    /// The session is running. Stop it first.
    #[error("session {session} is running; stop it first")]
    Running {
        /// The session that was left alone.
        session: SessionName,
    },
    /// There was nothing to delete.
    #[error("no such session: {session}")]
    NoSuchSession {
        /// The name that matched nothing.
        session: SessionName,
    },
    /// A directory could not be removed.
    #[error("could not remove {}: {source}", path.display())]
    Remove {
        /// The directory.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// A directory on the context is not one this session owns, so the
    /// recursive delete refused to run on it.
    #[error("refusing to remove {}: it is not session {session}'s own directory", path.display())]
    NotSessionOwned {
        /// The path that was left alone.
        path: PathBuf,
        /// The session it was supposed to belong to.
        session: SessionName,
    },
}

/// The directory every session's runtime directory sits in.
///
/// Derived from [`Ctx::for_session`] rather than rebuilt, so the place sessions
/// are *listed* from and the place a session *binds* in cannot drift apart:
/// there is exactly one expression of `<runtime root>/amx/<session>` in the
/// tree and this reads the parent off it. The cost of borrowing that derivation
/// is that listing also needs a *state* root to be configured, which it does not
/// use — the same environment failure every other verb hits first.
pub fn runtime_root(env: &Env) -> Result<PathBuf, CtxError> {
    let ctx = Ctx::for_session(SessionName::default(), env)?;
    Ok(ctx
        .runtime_dir
        .parent()
        .unwrap_or(&ctx.runtime_dir)
        .to_path_buf())
}

/// Every session with a server answering on its socket, by name.
///
/// Directories that no longer have a live server are skipped, not reported as
/// dead entries: 04 §1's list is of "running servers", and a stale directory is
/// an artifact of a server that already exited. [`delete`] is what removes one.
pub fn list(env: &Env) -> Result<Vec<Running>, ListError> {
    let root = runtime_root(env)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        // No root at all means no session has ever run here, which is an empty
        // list rather than a failure.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(ListError::Read { path: root, source }),
    };

    let mut running = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ListError::Read {
            path: root.clone(),
            source,
        })?;
        let Some(session) = entry
            .file_name()
            .to_str()
            .and_then(|name| SessionName::new(name).ok())
        else {
            continue;
        };
        let runtime_dir = entry.path();
        let socket = runtime_dir.join(amx_core::ctx::SOCKET_NAME);
        if probe(&socket)?.is_running() {
            running.push(Running {
                session,
                runtime_dir,
                socket,
            });
        }
    }
    running.sort_by(|a, b| a.session.cmp(&b.session));
    Ok(running)
}

/// Stop the session `ctx` names and wait for its socket to go quiet.
///
/// The server is signalled, not killed: `SIGTERM` reaches the signal watch in
/// [`serve`](super::serve::serve), which cancels the session token, and the
/// gateway unlinks the socket as it drains. A `SIGKILL` would skip all of that
/// and leave exactly the stale socket the connect probe then has to clean up.
pub async fn stop(ctx: &Ctx, timeout: Duration) -> Result<StopOutcome, StopError> {
    if !probe(&ctx.socket)?.is_running() {
        return Ok(StopOutcome::NotRunning);
    }
    let pid = server_pid(&ctx.socket)
        .await?
        .ok_or_else(|| StopError::UnknownPeer {
            path: ctx.socket.clone(),
        })?;
    signal_term(pid)?;

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if !probe(&ctx.socket)?.is_running() {
            // A server that shut down cleanly took its socket with it. One that
            // did not is stale from here on, and this is the moment to say so.
            clear_if_stale(&ctx.socket)?;
            return Ok(StopOutcome::Stopped { pid });
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(StopError::StillRunning {
                path: ctx.socket.clone(),
                timeout,
            });
        }
        tokio::time::sleep(STOP_POLL).await;
    }
}

/// Remove a stopped session's runtime and state directories.
///
/// Refuses a running session: see the module doc.
///
/// Both directories are removed *recursively*, because since M1 the state
/// directory has contents — `session.json` and, when history is on, a
/// `history/` full of per-pane sidecars (D-M1-4). A recursive delete driven by
/// a path is worth being careful about, so this one is pinned twice over: the
/// paths come from [`Ctx`], never from an argument, and [`session_owned`]
/// re-checks that each is the `<root>/amx/<session>` directory this very
/// context names before a single entry is unlinked. `remove_dir_all` itself
/// does not follow symbolic links — a symlink inside the tree is unlinked, not
/// descended — so a planted link cannot walk the delete out of the session's
/// own directory either.
pub fn delete(ctx: &Ctx) -> Result<(), DeleteError> {
    if probe(&ctx.socket)?.is_running() {
        return Err(DeleteError::Running {
            session: ctx.session.clone(),
        });
    }
    let removed =
        remove_tree(&ctx.runtime_dir, &ctx.session)? | remove_tree(&ctx.state_dir, &ctx.session)?;
    if removed {
        Ok(())
    } else {
        Err(DeleteError::NoSuchSession {
            session: ctx.session.clone(),
        })
    }
}

/// Recursively remove `dir` if it is there; `true` if it was.
fn remove_tree(dir: &Path, session: &SessionName) -> Result<bool, DeleteError> {
    if !session_owned(dir, session) {
        return Err(DeleteError::NotSessionOwned {
            path: dir.to_path_buf(),
            session: session.clone(),
        });
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(DeleteError::Remove {
            path: dir.to_path_buf(),
            source,
        }),
    }
}

/// Whether `dir` is the directory `session` owns under one of amx's roots.
///
/// The shape [`Ctx::for_session`] builds and the only shape a recursive delete
/// may touch: a component named for the session, directly inside amx's own
/// level of a shared root — `amx`, or `amx-<uid>` under the world-writable
/// runtime fallbacks. `Ctx`'s fields are public, so this is what keeps a
/// hand-assembled context from turning `session delete` into an arbitrary
/// recursive remove.
fn session_owned(dir: &Path, session: &SessionName) -> bool {
    let named_for_the_session = dir.file_name().is_some_and(|name| name == session.as_str());
    let under_amx = dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|level| level.to_str())
        .is_some_and(|level| {
            level == amx_core::ctx::AMX_DIR
                || level
                    .strip_prefix(amx_core::ctx::AMX_DIR)
                    .is_some_and(|rest| rest.starts_with('-'))
        });
    named_for_the_session && under_amx
}

/// Send `SIGTERM` to `pid`.
fn signal_term(pid: u32) -> Result<(), StopError> {
    let target = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| StopError::Signal {
            pid,
            source: io::Error::from(io::ErrorKind::InvalidInput),
        })?;
    rustix::process::kill_process(target, Signal::TERM).map_err(|source| StopError::Signal {
        pid,
        source: source.into(),
    })
}
