//! What `amx server --handoff-import` runs: the same session, in a new process.
//!
//! [`serve`](super::serve) assembles a session from nothing and a snapshot;
//! this assembles one from a manifest and a fistful of descriptors
//! (`docs/09-m3-plan.md` §3, D-M3-5). The five actors are the same five, in the
//! same order and for the same reasons — read `serve.rs`'s comments for those,
//! because this file only records where the order *differs* and why.
//!
//! It differs in exactly one place, and §3 is what forces it. `serve` binds the
//! session socket **first**, because losing that race must cost an error rather
//! than a set of actors to tear down. An importer cannot: until the exporter
//! hears `restored` it keeps its listener, so that an `amx` typed mid-handoff
//! finds a live server rather than daemonizing a rival (D-M3-6 point 4). So the
//! bind moves to the middle — after the panes are inherited, before the commit
//! — and everything that hangs off the gateway moves with it: the
//! [`StatusView`](crate::actor::StatusView) is the gateway's to create, so the
//! hub is assembled after the bind rather than before the restore. Nothing is
//! lost by that here, because the reason `serve` builds the hub early is that
//! its restore *spawns* panes whose `PaneStarted` it would otherwise miss, and
//! this path spawns nothing: the panes are announced explicitly, once the hub
//! is listening, by [`Core::announce_inherited`].
//!
//! # The staged commit, as a sequence of blocking calls
//!
//! Both handoff state machines are blocking — ancillary data has no async
//! surface worth inventing for a path that runs once per upgrade — and W05's
//! note is explicit that the importer's assembly owns the `spawn_blocking`.
//! Every transition below therefore moves the machine onto the blocking pool
//! and takes it back, which is also what makes the ordering readable: the
//! stages are the `await`s.
//!
//! # Strict abort
//!
//! §3's hardest row. Anything that fails before `committed` — a manifest from
//! outside the window, a pane that will not quiesce, a socket that never comes
//! free, an exporter that dies wedged rather than dead — unlinks whatever this
//! process bound and returns an error, having served nothing. There is no
//! partial import, and there is no method here that could produce one: the only
//! way past [`Importer::await_commit`] is for it to have returned.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use amx_core::{Bus, Ctx, Scheduled};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use super::serve::{CORE_MAILBOX, ServeReport, StopOn};
use crate::actor::agent_hub::AgentHub;
use crate::actor::agent_hub::inherit::InheritedPane;
use crate::actor::core::{AdoptError, Core};
use crate::actor::gateway::{Gateway, GatewayError};
use crate::actor::persist::{PERSIST_MAILBOX, Persist};
use crate::actor::{AGENT_MAILBOX, AgentHandle, CoreHandle, PersistHandle};
use crate::config_rt::ConfigRuntime;
use crate::handoff::manifest::{Manifest, ManifestError};
use crate::handoff::protocol::importer::Restoring;
use crate::handoff::protocol::{Ending, HandoffError, Importer, Timeouts, Token};
use crate::platform::watch::watch_config;
use crate::runtime::Runtime;

/// How often the probe loop looks at the session socket path.
///
/// The wait is normally a single millisecond — the exporter unlinks the file
/// and reports `restored` in the same breath — so this is the granularity of
/// the unusual case, bounded by [`Timeouts::socket_free`] (§3's 5 s).
const SOCKET_POLL: Duration = Duration::from_millis(10);

/// Everything the import needs that the session context does not carry.
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// The exporter's private handoff socket, from this process's argv.
    pub socket: PathBuf,
    /// The secret that opens it, read from this process's stdin (D-M3-6
    /// point 1).
    pub token: Token,
    /// §3's stage bounds. Tests shrink them; nothing else does.
    pub timeouts: Timeouts,
    /// What, besides the session's own token, may stop the successor.
    pub stop: StopOn,
}

/// The import did not end with this process owning the session.
///
/// Every variant is a pre-commit failure — there is nothing past the commit
/// that can fail, the advisory `owned` being unfailable by construction — so
/// [`ending`](ImportFailed::ending) answers [`Ending::Abort`] for all of them.
/// It asks [`HandoffError::ending`] rather than restating §3's table.
#[derive(Debug, Error)]
pub enum ImportFailed {
    /// A stage of §3 failed.
    #[error(transparent)]
    Handoff(#[from] HandoffError),
    /// The manifest is not a manifest, or not one this build reads.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The session could not be assembled from what crossed.
    #[error(transparent)]
    Adopt(#[from] AdoptError),
    /// The session socket never came free, or could not be taken.
    #[error("the session socket did not come free: {0}")]
    Socket(GatewayError),
    /// A blocking stage's thread did not come back.
    #[error("the handoff stage did not complete: {0}")]
    Joined(#[from] tokio::task::JoinError),
}

impl ImportFailed {
    /// What §3's crash table leaves this process holding.
    ///
    /// Always [`Ending::Abort`]: unlink what was bound, serve nothing, exit
    /// non-zero. The handoff variant defers to the protocol's own computation
    /// of the table so the two can never drift.
    #[must_use]
    pub fn ending(&self) -> Ending {
        match self {
            Self::Handoff(err) => err.ending(),
            _ => Ending::Abort,
        }
    }
}

/// Take a session over from a running server, and then serve it.
///
/// Returns when the successor is stopped, exactly as
/// [`serve`](super::serve::serve) does — the two paths are the same session
/// from the commit onwards.
///
/// # Errors
///
/// [`ImportFailed`] for anything that goes wrong before ownership transfers.
/// Whatever this process bound is unlinked on the way out and no client is ever
/// answered, which is §3's strict abort: **no partial import ever serves.**
pub async fn import(ctx: Ctx, opts: ImportOptions) -> Result<ServeReport, ImportFailed> {
    let ImportOptions {
        socket,
        token,
        timeouts,
        stop,
    } = opts;

    // Steps 2–9: connect, prove who this is, read the session, take every
    // descriptor. All of it blocking, all of it off the runtime's threads.
    let received =
        tokio::task::spawn_blocking(move || receive(&socket, &token, timeouts)).await??;
    let Received {
        manifest,
        masters,
        importer,
    } = received;

    // The bus continues the exporter's sequence space, which is what keeps
    // `Welcome.seq` monotonic across the swap and makes a client's `Resume`
    // mean the same thing on both sides of it (D-M3-4). The replay buffer
    // starts empty: those events belonged to a process that is gone.
    let ctx = Ctx {
        bus: Arc::new(Bus::new_at(ctx.bus.replay_capacity(), manifest.seq)),
        ..ctx
    };

    let (core_tx, core_rx) = mpsc::channel(CORE_MAILBOX);
    let mut runtime = Runtime::new(ctx.clone());
    // The identity is the whole point of a handoff: a reconnecting client
    // branches on `Welcome.session`, and a fresh id would tell every one of
    // them to throw away exactly the state this went to the trouble of
    // carrying across (D-M3-5).
    let mut core =
        Core::new(ctx.clone(), CoreHandle::new(core_tx.clone())).with_session_id(manifest.session);
    let config = ConfigRuntime::load(&ctx);
    core.set_config(config.subscribe());
    let persist_config = config.subscribe();
    spawn_config_watcher(&mut runtime, &ctx, config);

    // The hub's mailbox exists before the hub does, so `Core` can be told
    // where to report while the hub itself waits for the gateway that owns the
    // view it writes.
    let (agent_tx, agent_rx) = mpsc::channel(AGENT_MAILBOX);
    let agent_events = ctx.bus.subscribe();
    core.set_agent(AgentHandle::new(agent_tx.clone()));

    // Step 9: the session, assembled and touching nothing. Every pane is
    // quiesced and every inherited terminal is behind a closed gate.
    // A locally-detected fault leaves no state machine to abort from
    // afterwards — a failed transition consumes it — so the abort is written
    // at the one moment there is still somebody to tell, and the exporter can
    // resume its quiesced panes the moment it hears why.
    let adopted = match core.adopt_manifest(&manifest, masters) {
        Ok(adopted) => adopted,
        Err(err) => {
            importer.abort(&err.to_string());
            return Err(unwind(core, ctx, runtime, err.into()).await);
        }
    };
    let inherited = adopted
        .panes
        .iter()
        .map(|(pane, workspace)| InheritedPane {
            pane: *pane,
            workspace: *workspace,
            status: manifest
                .agents
                .iter()
                .find(|entry| entry.pane == *pane)
                .map(|entry| entry.agent.clone()),
        })
        .collect();

    // Assembled after the adoption it must not re-save, which is `serve`'s
    // reason for putting it after the restore: the subscription opens on a
    // clean "nothing dirty" slate. Here the slate is clean by construction —
    // an import publishes nothing — and the snapshot already on disk is the
    // one the exporter wrote, which the successor now owns.
    let (persist_tx, persist_rx) = mpsc::channel(PERSIST_MAILBOX);
    core.set_persist(PersistHandle::new(persist_tx));
    let events = ctx.bus.subscribe();
    let persist = Persist::new(
        ctx.clone(),
        CoreHandle::new(core_tx.clone()),
        persist_config,
    );
    runtime.spawn("persist", async move {
        let _persist = persist.run(persist_rx, events).await;
    });

    // Step 10: saying this is what makes the exporter retire its gateway.
    let importer = match tokio::task::spawn_blocking(move || importer.restored()).await? {
        Ok(importer) => importer,
        Err(err) => return Err(unwind(core, ctx, runtime, err.into()).await),
    };

    // Steps 11–12: the socket changes hands. The exporter unlinks it on its
    // side of `restored`; this is the other side of that, bounded by §3's
    // `socket_free` — the one constant the protocol defines and never uses,
    // because this loop is the only thing that needs it.
    let mut gateway = match bind_when_free(&ctx, CoreHandle::new(core_tx.clone()), timeouts).await {
        Ok(gateway) => gateway,
        Err(err) => {
            importer.abort(&err.to_string());
            return Err(unwind(core, ctx, runtime, ImportFailed::Socket(err)).await);
        }
    };
    let hub = AgentHub::new(
        ctx.clone(),
        CoreHandle::new(core_tx.clone()),
        gateway.status_view(),
    )
    .inherit(inherited, adopted.attention.clone());
    gateway.set_agent(AgentHandle::new(agent_tx));
    runtime.spawn("agent-hub", async move {
        let _agents = hub.run(agent_rx, agent_events).await;
    });
    // Awaited, not fired and forgotten: the hub is listening, `Core` is not
    // running yet, and a session of two hundred panes must not lose the ones
    // past the mailbox's depth.
    core.announce_inherited().await;

    // `Core` runs *before* the successor says it is ready, so that a client
    // that connects into the window between `ready` and `committed` is
    // answered from a frozen session rather than parked on a mailbox nobody is
    // draining (§3 step 12: "new connections see frozen grids and full
    // state"). Nothing it can be asked to do can move a pane: every one of
    // them is quiesced, and every inherited terminal is behind a closed gate
    // until [`Adopted::take_over`] opens it.
    runtime.spawn("core", async move {
        let _core = core.run(core_rx, |_: &Scheduled| {}).await;
    });
    let (report_tx, report_rx) = oneshot::channel();
    runtime.spawn("gateway", async move {
        let _ = report_tx.send(gateway.run().await);
    });
    let importer = match tokio::task::spawn_blocking(move || importer.ready()).await? {
        Ok(importer) => importer,
        Err(err) => return Err(stop_serving(ctx, runtime, err.into()).await),
    };

    // Step 13. Until this returns, this process owns nothing: a wedged
    // exporter that is not dead must never meet a live importer, so a commit
    // that does not arrive means unlink and exit, never serve.
    let importer = match tokio::task::spawn_blocking(move || importer.await_commit()).await? {
        Ok(importer) => importer,
        Err(err) => return Err(stop_serving(ctx, runtime, err.into()).await),
    };

    // Step 14: ownership has transferred, so the terminals open and the panes
    // resume. Blocking, per pane, which is why it is not on a runtime thread.
    let refused = tokio::task::spawn_blocking(move || adopted.take_over()).await?;
    for (pane, err) in refused {
        // Not fatal, and nothing to abort back to: the session is this
        // process's now. One terminal that will not resume is one pane a user
        // has to close, and the rest of the session is serving.
        tracing::error!(%pane, error = %err, "an inherited pane did not resume");
    }
    tokio::task::spawn_blocking(move || importer.owned()).await?;
    tracing::info!(session = %ctx.session, "the handoff committed; this server owns the session");

    if stop == StopOn::Signals {
        runtime.spawn("signals", watch_signals(ctx.cancel.clone()));
    }
    ctx.cancel.cancelled().await;
    let shutdown = runtime.shutdown().await;
    let gateway = report_rx.await.unwrap_or_default();
    Ok(ServeReport { gateway, shutdown })
}

/// What §3 steps 2–9 produce.
struct Received {
    manifest: Box<Manifest>,
    masters: Vec<std::os::fd::OwnedFd>,
    importer: Importer<Restoring>,
}

/// Steps 2–9, on a blocking thread: connect, authenticate, read, receive.
///
/// The manifest is checked *before* a single descriptor is accepted, and the
/// number of descriptors accepted comes from the manifest that was just
/// checked — W05's note is emphatic that taking it from anywhere else reopens
/// the pairing the index byte closes.
fn receive(
    socket: &std::path::Path,
    token: &Token,
    timeouts: Timeouts,
) -> Result<Received, ImportFailed> {
    let importer = Importer::connect(socket, token, timeouts)?;
    let (document, importer) = importer.recv_manifest()?;
    let manifest: Manifest = match serde_json::from_value(document) {
        Ok(manifest) => manifest,
        Err(err) => {
            // A document this side cannot read is this side's verdict, and the
            // exporter is owed it: it is still holding a quiesced session that
            // it can resume the moment it hears why.
            let reason = ManifestError::Malformed(err.to_string());
            importer.abort(&reason.to_string());
            return Err(reason.into());
        }
    };
    if let Err(err) = manifest.check_version() {
        importer.abort(&err.to_string());
        return Err(err.into());
    }
    let importer = importer.validated(manifest.panes.len())?;
    let (masters, importer) = importer.recv_masters()?;
    Ok(Received {
        manifest: Box::new(manifest),
        masters,
        importer,
    })
}

/// Wait for the session socket path to come free, then take it.
///
/// The exporter holds its listener until it hears `restored` and unlinks the
/// file as it retires (§3 step 11), so this is a race with a retirement rather
/// than with another server: [`Gateway::bind`]'s own probe answers
/// `AlreadyRunning` for as long as the exporter is still answering, and this
/// loop simply asks again. The residual window — between the unlink and this
/// bind — is R-M3-5, where a freshly typed `amx` can daemonize a rival that
/// then loses the bind race cleanly.
async fn bind_when_free(
    ctx: &Ctx,
    core: CoreHandle,
    timeouts: Timeouts,
) -> Result<Gateway, GatewayError> {
    let deadline = tokio::time::Instant::now() + timeouts.socket_free;
    loop {
        let err = match Gateway::bind(ctx.clone(), core.clone()) {
            Ok(gateway) => return Ok(gateway),
            Err(err) => err,
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(err);
        }
        tokio::time::sleep(SOCKET_POLL).await;
    }
}

/// Give up everything this process took, and keep the failure that caused it.
///
/// The strict abort's second half, for a failure that arrives *after* the
/// gateway bound: cancelling the session stops the accept loop, and the
/// gateway removes the socket file it created on its way out — which is what
/// makes "unlink anything it bound" a consequence of the ordinary shutdown
/// rather than a second cleanup path that could disagree with it.
///
/// The panes are not killed. Their terminals are the exporter's again the
/// moment this process exits, their children were never signalled, and the
/// descriptors close with the process (§3's crash table, rows 8–12).
async fn stop_serving(ctx: Ctx, runtime: Runtime, err: ImportFailed) -> ImportFailed {
    ctx.cancel.cancel();
    let _ = runtime.shutdown().await;
    err
}

/// The same abort, for a failure that arrives before `Core` was spawned.
///
/// `Core` is dropped first and explicitly. It holds the only other sender into
/// the hub's mailbox, and a hub drains that mailbox *to closure* before it
/// returns (`docs/08-m2-plan.md` §3): shutting the runtime down while a live
/// `Core` value still held a sender would leave the drain waiting for a
/// message from an actor that was never started.
///
/// Dropping it also drops the inherited pane hosts, which closes this
/// process's copies of the descriptors — the children are untouched, because
/// the exporter still holds the open file descriptions it is about to resume
/// reading (§3's crash table, rows 8–12).
async fn unwind(
    core: crate::actor::core::Core,
    ctx: Ctx,
    runtime: Runtime,
    err: ImportFailed,
) -> ImportFailed {
    drop(core);
    stop_serving(ctx, runtime, err).await
}

/// Cancel `cancel` on `SIGTERM` or `SIGINT`, and return when it is cancelled.
///
/// The successor is an ordinary server from the commit onwards, and `amx
/// session stop` signals whatever process the socket's peer credentials name —
/// which is this one. Spelled again for the reason
/// [`spawn_config_watcher`] is.
async fn watch_signals(cancel: tokio_util::sync::CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};

    let (mut terminate, mut interrupt) = match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(terminate), Ok(interrupt)) => (terminate, interrupt),
        (terminate, interrupt) => {
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

/// Put the config watcher under the supervisor, if the watch can be
/// established.
///
/// [`serve`](super::serve)'s helper, spelled again rather than shared: that
/// file is the export path's this wave (W06), and a live upgrade that silently
/// stopped reloading `config.toml` would be a capability lost to a merge
/// conflict. Folding the two assemblies' shared scaffolding into one is a W14
/// seam, named here so it is a decision rather than an oversight.
fn spawn_config_watcher(runtime: &mut Runtime, ctx: &Ctx, config: ConfigRuntime) {
    match watch_config(ctx, ctx.cancel.clone()) {
        Ok(watcher) => {
            let bus = Arc::clone(&ctx.bus);
            runtime.spawn("config-watch", config.run(bus, watcher));
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
