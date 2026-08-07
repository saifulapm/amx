//! What `amx server` runs: the actors of one session, under one supervisor.
//!
//! 04 §2: "The server is a set of tokio actors with typed mailboxes, supervised
//! by a root task with `CancellationToken` + `JoinSet` (structured shutdown;
//! nothing detached, everything joined)." This function is the assembly of that
//! sentence — `Core`, `Gateway`, the config watcher and the signal watch are
//! spawned through [`Runtime::spawn`] and nowhere else, and the only way out is
//! through [`Runtime::shutdown`], which returns when the `JoinSet` is empty.
//!
//! Stopping is therefore one path with three entrances: a `SIGTERM` (what
//! `amx session stop` sends), a `SIGINT` (what a `ctrl+c` on a foreground
//! server sends), and a direct [`Ctx::cancel`] (what a test uses, and what an
//! in-process client will use once "local run" is server + client in one
//! process tree). All three cancel the same token, and the socket is removed on
//! the way out by the gateway that bound it.

use std::path::PathBuf;
use std::sync::Arc;

use amx_core::{Ctx, Scheduled};
use thiserror::Error;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::actor::core::{Core, RestoreOptions};
use crate::actor::gateway::{Gateway, GatewayError, GatewayReport};
use crate::actor::persist::{PERSIST_MAILBOX, Persist};
use crate::actor::{CoreHandle, PersistHandle};
use crate::config_rt::ConfigRuntime;
use crate::platform::watch::watch_config;
use crate::runtime::{Runtime, ShutdownReport};

/// Depth of the `Core` actor's mailbox.
///
/// Bounded, like every mailbox here: a client that calls faster than the `Core`
/// folds is slowed at its own `send`, which is the backpressure that keeps a
/// burst from becoming an unbounded queue.
pub const CORE_MAILBOX: usize = 256;

/// What, besides [`Ctx::cancel`], is allowed to stop the server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StopOn {
    /// `SIGTERM` and `SIGINT` cancel the session.
    ///
    /// What the daemon uses: `amx session stop` signals the pid the socket's
    /// peer credentials name, and this is the half that hears it.
    Signals,
    /// Only the session's own cancellation token stops it.
    ///
    /// What a test uses, so that one test's shutdown cannot be another test's
    /// process-wide signal.
    Cancellation,
}

/// The server could not start.
#[derive(Debug, Error)]
pub enum ServeError {
    /// The session socket could not be taken — most often because this session
    /// is already running, which is not a failure so much as an answer.
    #[error(transparent)]
    Gateway(#[from] GatewayError),
}

/// What one server run did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ServeReport {
    /// The gateway's connection accounting.
    pub gateway: GatewayReport,
    /// The supervisor's task accounting.
    pub shutdown: ShutdownReport,
}

impl ServeReport {
    /// Whether every task and every connection was joined without panicking.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.gateway.clean() && self.shutdown.clean()
    }
}

/// Run one session's server until it is stopped, then join everything.
///
/// Binds the socket before spawning the accept loop, so a caller that loses the
/// bind race (another server got there first) is told immediately, by the error
/// rather than by a process that starts and does nothing.
pub async fn serve(ctx: Ctx, stop: StopOn) -> Result<ServeReport, ServeError> {
    let (core_tx, core_rx) = mpsc::channel(CORE_MAILBOX);
    // Bound before anything is spawned: losing this race is the ordinary
    // outcome for the second `amx` of two started at once, and it must cost a
    // returned error rather than a set of actors that have to be torn down.
    let gateway = Gateway::bind(ctx.clone(), CoreHandle::new(core_tx.clone()))?;

    let mut runtime = Runtime::new(ctx.clone());
    let mut core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
    // Read before anything spawns a process: restore below is the session's
    // first pane spawn, and it must use the shell the user configured rather
    // than the one this server would have picked a moment earlier.
    let config = ConfigRuntime::load(&ctx);
    core.set_config(config.subscribe());
    let persist_config = config.subscribe();
    spawn_config_watcher(&mut runtime, &ctx, config);
    // Between the bind and the accept loop (D-M1-9): the bind has claimed the
    // session, so this server is the one that owns its state, and the earliest
    // client that can possibly connect already sees the restored session. A
    // restore that fails does not stop the server — a fresh session with a
    // full loss report beats a refused start — which is why nothing here
    // returns an error.
    core.restore_from_disk(&RestoreOptions { home: home_dir() });

    // Persistence is assembled after the restore it must not re-save: the
    // subscription is taken here, so the events restore just published sit
    // behind it and the actor opens on a clean "nothing dirty" slate
    // (`docs/07-m1-plan.md` §2). `Core` learns where persistence listens for
    // one message only — the final capture it pushes on its way down.
    let (persist_tx, persist_rx) = mpsc::channel(PERSIST_MAILBOX);
    core.set_persist(PersistHandle::new(persist_tx));
    let events = ctx.bus.subscribe();
    let persist = Persist::new(ctx.clone(), CoreHandle::new(core_tx), persist_config);
    runtime.spawn(async move {
        let _persist = persist.run(persist_rx, events).await;
    });

    runtime.spawn(async move {
        // Output rides two paths out of a folded batch. Grid traffic flows
        // from each pane's published frames through the per-client grid
        // streams `stream.bind` spawns into each connection's priority
        // writer; layout-level batches re-project the active client's
        // viewport and resize the panes whose rects moved (both inside
        // `Core::run`). The sink itself is the remaining seam — tests hang
        // assertions on it, and per-batch consumers land through it.
        let _core = core.run(core_rx, |_: &Scheduled| {}).await;
    });

    let (report_tx, report_rx) = oneshot::channel();
    runtime.spawn(async move {
        let _ = report_tx.send(gateway.run().await);
    });

    if stop == StopOn::Signals {
        runtime.spawn(watch_signals(ctx.cancel.clone()));
    }

    ctx.cancel.cancelled().await;
    let shutdown = runtime.shutdown().await;
    let gateway = report_rx.await.unwrap_or_default();
    Ok(ServeReport { gateway, shutdown })
}

/// Put the config watcher under the supervisor, if the watch can be
/// established.
///
/// A watch that cannot be set up (no permission to create the config
/// directory, an inotify instance limit already reached) costs hot reloading
/// and nothing else: the configuration read at startup stays in force for the
/// life of the server, which is what every pre-M1 amx did. Refusing to serve
/// the session over it would trade a working multiplexer for a missing
/// convenience.
fn spawn_config_watcher(runtime: &mut Runtime, ctx: &Ctx, config: ConfigRuntime) {
    match watch_config(ctx, ctx.cancel.clone()) {
        Ok(watcher) => {
            let bus = Arc::clone(&ctx.bus);
            runtime.spawn(config.run(bus, watcher));
        }
        Err(err) => {
            tracing::warn!(
                path = %ctx.config_path.display(),
                error = %err,
                "config changes will not be picked up until this session restarts",
            );
        }
    }
}

/// Where a restored pane whose saved directory has vanished respawns.
///
/// One of the two places the server reads its own environment rather than a
/// `Ctx` — `$SHELL` for a pane's command is the other — because `$HOME` is a
/// property of the user running the server, not of the session. Everything
/// below this line takes it as a value, so a test degrades into a tempdir.
/// A `$HOME` that is unset or is not a directory falls back to the directory
/// the server itself was started in, and to `/` behind that: restore needs
/// *somewhere* to put a pane, and refusing to restore it would turn a missing
/// directory into a lost pane.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Cancel `cancel` on `SIGTERM` or `SIGINT`, and return when it is cancelled.
///
/// Returning on cancellation is what makes this a `Runtime` task like any
/// other: the shutdown that this task may itself have started still joins it.
async fn watch_signals(cancel: CancellationToken) {
    let (mut terminate, mut interrupt) = match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(terminate), Ok(interrupt)) => (terminate, interrupt),
        (terminate, interrupt) => {
            // Nothing to do but say so: a server that cannot hear `SIGTERM` is
            // still a working server, it just has to be stopped another way.
            let err = terminate.err().or(interrupt.err());
            tracing::error!(error = ?err, "could not install signal handlers");
            cancel.cancelled().await;
            return;
        }
    };

    tokio::select! {
        _ = terminate.recv() => {
            tracing::info!("SIGTERM: shutting the session down");
            cancel.cancel();
        }
        _ = interrupt.recv() => {
            tracing::info!("SIGINT: shutting the session down");
            cancel.cancel();
        }
        () = cancel.cancelled() => {}
    }
}
