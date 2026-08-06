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

use crate::actor::CoreHandle;
use crate::conn;

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
}

impl Gateway {
    /// Take this session's socket.
    ///
    /// Must be called inside a tokio runtime: the listener registers with the
    /// reactor as it binds. Fails rather than stealing the socket if a live
    /// server answers on it.
    pub fn bind(ctx: Ctx, core: CoreHandle) -> Result<Self, GatewayError> {
        prepare_dir(&ctx.runtime_dir)?;
        clear_stale(&ctx.socket)?;
        let listener = UnixListener::bind(&ctx.socket).map_err(|source| GatewayError::Bind {
            path: ctx.socket.clone(),
            source,
        })?;
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
        })
    }

    /// Live connection accounting.
    #[must_use]
    pub const fn probe(&self) -> &GatewayProbe {
        &self.probe
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

async fn client_task(stream: tokio::net::UnixStream, ctx: Ctx, core: CoreHandle, guard: LiveGuard) {
    let _guard = guard;
    if let Err(err) = conn::serve(stream, ctx, core).await {
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

/// Probe an existing socket file: answer means occupied, refusal means stale.
fn clear_stale(path: &Path) -> Result<(), GatewayError> {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_probe) => Err(GatewayError::AlreadyRunning {
            path: path.to_path_buf(),
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)
            .map_err(|source| GatewayError::Probe {
                path: path.to_path_buf(),
                source,
            }),
        Err(source) => Err(GatewayError::Probe {
            path: path.to_path_buf(),
            source,
        }),
    }
}
