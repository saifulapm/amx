//! The `Gateway` actor: one socket per session, one task per client.
//!
//! 04 §1: "One socket per session (`$XDG_RUNTIME_DIR/amx/<session>/sock`,
//! 0600). Not two, not three (fixes W3's surface sprawl). Stale-socket
//! disambiguation by connect probe."
//!
//! Both halves of that are here. The socket is created inside a `0700`
//! directory and then set to `0600`, so it is unreachable by another user even
//! during the window between `bind` and `chmod`. A socket file that already
//! exists is probed by connecting to it: a server that answers means this
//! session is already running and binding must fail; a refused connection means
//! the file outlived its process and is removed. Nothing is inferred from a
//! pid file, and there is no lock to go stale in turn.
//!
//! Every accepted connection is spawned into a `JoinSet` this actor owns and
//! joined before [`run`](Gateway::run) returns, so "no leaked connection task"
//! is a counted fact in [`GatewayReport`] rather than a hope (04 §2).

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use amx_core::Ctx;
use thiserror::Error;
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::actor::{CoreHandle, StatusView};
use crate::conn;
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

/// Observable connection accounting, cloneable and cheap to read.
#[derive(Clone, Debug, Default)]
pub struct GatewayProbe {
    live: Arc<AtomicUsize>,
    accepted: Arc<AtomicU64>,
}

impl GatewayProbe {
    /// Connections currently being served.
    #[must_use]
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    /// Connections accepted since the gateway bound.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Acquire)
    }
}

/// Decrements the live count however its task ends, panic included.
struct LiveGuard(Arc<AtomicUsize>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// What the accept loop did before it stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct GatewayReport {
    /// Connections accepted.
    pub accepted: u64,
    /// Connection tasks joined.
    ///
    /// Equal to `accepted` on a clean shutdown; a smaller number would mean a
    /// task escaped the `JoinSet`, which is why both are counted separately.
    pub joined: usize,
    /// How many of those tasks panicked.
    pub panicked: usize,
}

impl GatewayReport {
    /// Whether every accepted connection was joined and none panicked.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.panicked == 0 && self.joined as u64 == self.accepted
    }
}

/// The socket surface of one session.
#[derive(Debug)]
pub struct Gateway {
    ctx: Ctx,
    core: CoreHandle,
    listener: UnixListener,
    probe: GatewayProbe,
    status: StatusView,
}

impl Gateway {
    /// Take this session's socket.
    ///
    /// Must be called inside a tokio runtime: the listener registers with the
    /// reactor as it binds. Fails rather than stealing the socket if a live
    /// server answers on it.
    ///
    /// Stale-socket cleanup is [`probe::clear_if_stale`], inode guard
    /// included: between the probe and the unlink another starting server may
    /// have replaced the file with its own not-yet-listening socket, and
    /// unlinking that one would strand a live server. If the guard skips the
    /// removal and this bind then loses the race, the path is probed once
    /// more so the caller hears [`GatewayError::AlreadyRunning`] rather than
    /// a bare bind failure.
    pub fn bind(ctx: Ctx, core: CoreHandle) -> Result<Self, GatewayError> {
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
        // Whatever survived the probe still owns the path, and the refusal
        // must be this code's, not `bind(2)`'s: Linux refuses a leftover
        // entry with `EADDRINUSE`, but darwin's `bind(2)` follows a trailing
        // symlink and would plant the socket at its target — a dangling
        // symlink probes `Absent` (connect follows it to nothing) and would
        // slip straight through to a mislocated bind. A racer that finished
        // listening inside the window is reported running, same as losing
        // the bind race below.
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
        Ok(Self {
            ctx,
            core,
            listener,
            probe: GatewayProbe::default(),
            status: StatusView::new(),
        })
    }

    /// Live connection accounting.
    #[must_use]
    pub const fn probe(&self) -> &GatewayProbe {
        &self.probe
    }

    /// The [`StatusView`] every connection this gateway accepts will read.
    ///
    /// Created here rather than passed in, because it is a handle on shared
    /// state and the gateway is what hands it to connections. `AgentHub` takes
    /// a clone of *this* view at assembly and writes through it
    /// (`docs/08-m2-plan.md` §3's "update `StatusView`, then publish the
    /// event"); an empty view is exactly what a session with no hub yet has,
    /// and a wait against one simply finds no status, which is the truth.
    #[must_use]
    pub fn status_view(&self) -> StatusView {
        self.status.clone()
    }

    /// The socket this gateway is listening on.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.ctx.socket
    }

    /// Accept connections until the session is cancelled, then join every
    /// connection task and remove the socket.
    pub async fn run(self) -> GatewayReport {
        // Connections observe a child of the session token, so a fatal accept
        // error can stop the connections this gateway owns without cancelling
        // the whole session — and cancelling the session still stops them,
        // because cancelling a parent cancels its children.
        let clients = self.ctx.cancel.child_token();
        let mut tasks = JoinSet::new();
        let mut report = GatewayReport::default();

        loop {
            tokio::select! {
                () = self.ctx.cancel.cancelled() => break,
                Some(joined) = tasks.join_next(), if !tasks.is_empty() => {
                    account(&mut report, joined);
                }
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _addr)) => {
                        report.accepted += 1;
                        self.probe.accepted.fetch_add(1, Ordering::Release);
                        self.probe.live.fetch_add(1, Ordering::Release);
                        tasks.spawn(client_task(
                            stream,
                            self.client_ctx(&clients),
                            self.core.clone(),
                            self.status.clone(),
                            LiveGuard(Arc::clone(&self.probe.live)),
                        ));
                    }
                    Err(err) if transient(&err) => {
                        tracing::debug!(error = %err, "transient accept failure");
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "accept failed, closing the gateway");
                        break;
                    }
                },
            }
        }

        clients.cancel();
        while let Some(joined) = tasks.join_next().await {
            account(&mut report, joined);
        }
        // The socket is this process's to remove: a later `bind` probes what it
        // finds, so leaving it behind would make the next start pay for a
        // connect that can only be refused.
        let _ = fs::remove_file(&self.ctx.socket);
        report
    }

    /// The context a connection task runs under: this session's paths and bus,
    /// with the gateway's own client cancellation token.
    fn client_ctx(&self, clients: &CancellationToken) -> Ctx {
        Ctx {
            cancel: clients.clone(),
            ..self.ctx.clone()
        }
    }
}

async fn client_task(
    stream: tokio::net::UnixStream,
    ctx: Ctx,
    core: CoreHandle,
    status: StatusView,
    guard: LiveGuard,
) {
    let _guard = guard;
    if let Err(err) = conn::serve(stream, ctx, core, status).await {
        tracing::debug!(error = %err, "connection ended");
    }
}

fn account(report: &mut GatewayReport, joined: Result<(), tokio::task::JoinError>) {
    report.joined += 1;
    if joined.is_err() {
        report.panicked += 1;
    }
}

/// Whether an accept failure is worth retrying rather than closing over.
fn transient(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

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
