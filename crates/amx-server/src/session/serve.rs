//! What `amx server` runs: the actors of one session, under one supervisor.
//!
//! 04 §2: "The server is a set of tokio actors with typed mailboxes, supervised
//! by a root task with `CancellationToken` + `JoinSet` (structured shutdown;
//! nothing detached, everything joined)." This function is the assembly of that
//! sentence — `Core`, `AgentHub`, `Gateway`, `Persist`, the config watcher and
//! the signal watch are spawned through [`Runtime::spawn`] and nowhere else,
//! and the only way out is through [`Runtime::shutdown`], which returns when
//! the `JoinSet` is empty.
//!
//! Stopping is therefore one path with three entrances: a `SIGTERM` (what
//! `amx session stop` sends), a `SIGINT` (what a `ctrl+c` on a foreground
//! server sends), and a direct [`Ctx::cancel`] (what a test uses, and what an
//! in-process client will use once "local run" is server + client in one
//! process tree). All three cancel the same token, and the socket is removed on
//! the way out by the gateway that bound it.

use amx_core::{Ctx, Scheduled};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::actor::agent_hub::AgentHub;
use crate::actor::core::{Core, RestoreOptions};
use crate::actor::gateway::{Gateway, GatewayError, GatewayReport};
use crate::actor::persist::{PERSIST_MAILBOX, Persist};
use crate::actor::{AGENT_MAILBOX, AgentHandle, CoreHandle, PersistHandle};
use crate::config_rt::ConfigRuntime;
use crate::handoff::export::{self, DrainWedged, JOB_MAILBOX, POST_COMMIT_DRAIN};
use crate::runtime::{Runtime, ShutdownReport};
use crate::session::scaffold::{SignalWatch, home_dir, spawn_config_watcher};

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
    /// A handoff committed and then this process would not go away.
    ///
    /// W01's watchdog firing (`docs/notes/m3-shutdown-wedge.md` §6). Not an
    /// error about the session — the successor owns it and is serving — but an
    /// error about *this* process, which is why it is one at all: the exporter
    /// exits non-zero with the drain census in the message rather than sitting
    /// there holding descriptors of terminals it no longer serves.
    #[error(transparent)]
    DrainWedged(#[from] DrainWedged),
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
    // Installed before the socket is bound, because the socket file appearing
    // is what makes this process findable: an `amx session stop` typed the
    // instant the path exists is a `SIGTERM` at a process that has not run
    // another statement yet, and with no handler registered that is the default
    // disposition — killed outright, no drain, no final capture, the socket
    // left behind. W01 saw exactly that, once, and could not place it.
    // Installed before the socket is bound, because the socket file appearing
    // is what makes this process findable: an `amx session stop` typed the
    // instant the path exists is a `SIGTERM` at a process that has not run
    // another statement yet, and with no handler registered that is the default
    // disposition — killed outright, no drain, no final capture, the socket
    // left behind. W01 saw exactly that, once, and could not place it.
    let signals = (stop == StopOn::Signals).then(SignalWatch::install);

    let (core_tx, core_rx) = mpsc::channel(CORE_MAILBOX);
    // Bound before anything is spawned: losing this race is the ordinary
    // outcome for the second `amx` of two started at once, and it must cost a
    // returned error rather than a set of actors that have to be torn down.
    let mut gateway = Gateway::bind(ctx.clone(), CoreHandle::new(core_tx.clone()))?;

    let mut runtime = Runtime::new(ctx.clone());
    // First, and before the restore below can spend real time spawning panes:
    // the socket is already bound, so an `amx session stop` typed now finds a
    // server — and a `SIGTERM` reaching a process with no handler installed is
    // the default disposition, which kills it outright with no drain, no final
    // capture, and the socket left behind. W01 saw exactly that, once.
    // The handlers are already registered; this only puts the task that reads
    // them under the supervisor, so a cancellation that arrived during the
    // bind is answered by the ordinary shutdown below.
    // The handlers are already registered; this only puts the task that reads
    // them under the supervisor, so a cancellation that arrived during the
    // bind is answered by the ordinary shutdown below.
    if let Some(signals) = signals {
        signals.spawn(&mut runtime, ctx.cancel.clone());
    }
    let mut core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
    // Read before anything spawns a process: restore below is the session's
    // first pane spawn, and it must use the shell the user configured rather
    // than the one this server would have picked a moment earlier.
    let config = ConfigRuntime::load(&ctx);
    core.set_config(config.subscribe());
    let persist_config = config.subscribe();
    spawn_config_watcher(&mut runtime, &ctx, config);

    // Assembled *before* the restore, unlike persistence below and for the
    // opposite reason: the restore is this session's first pane spawn, and a
    // hub that was not listening yet would miss every `PaneStarted` it
    // produces — a restored session of five agents would come back untracked
    // until each pane happened to respawn. Its bus subscription is taken here
    // too, so the `PaneCreated` events the restore publishes land in it.
    let (agent_tx, agent_rx) = mpsc::channel(AGENT_MAILBOX);
    let agent_events = ctx.bus.subscribe();
    core.set_agent(AgentHandle::new(agent_tx.clone()));
    // The hub is the view's only writer, and **the gateway's view is the one it
    // writes**. Its readers are the long-poll calls (`wait`, `agent.prompt
    // --wait`) and the status reads behind `agent.start`'s readiness, all of
    // which run on connection tasks and read the clone the gateway hands them.
    // A hub given a `StatusView` of its own would be a session where every one
    // of those waits watches a view nobody writes and times out (V17: the two
    // halves existed, and this line is where they meet).
    gateway.set_agent(AgentHandle::new(agent_tx));
    let hub = AgentHub::new(
        ctx.clone(),
        CoreHandle::new(core_tx.clone()),
        gateway.status_view(),
    );
    runtime.spawn("agent-hub", async move {
        let _agents = hub.run(agent_rx, agent_events).await;
    });
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
    runtime.spawn("persist", async move {
        let _persist = persist.run(persist_rx, events).await;
    });

    // The export orchestrator, and the two capabilities it needs that nothing
    // else may have: a way to retire the gateway (§3 step 11) and a queue
    // `Core` puts a frozen session on. It is spawned as a named task like every
    // other actor, so a drain waiting on a handoff says `handoff` rather than
    // going quiet — which is the whole reason W01 taught the runtime to name
    // things. The ledger is taken before `Core` moves into its task, because
    // the post-commit drain below is bounded only when it says ownership moved.
    let (jobs_tx, jobs_rx) = mpsc::channel(JOB_MAILBOX);
    core.set_handoff(jobs_tx);
    let handoff = core.handoff_ledger();
    let gateway_control = gateway.control();
    runtime.spawn(
        "handoff",
        export::orchestrate(jobs_rx, gateway_control, ctx.cancel.clone()),
    );

    runtime.spawn("core", async move {
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
    runtime.spawn("gateway", async move {
        let _ = report_tx.send(gateway.run().await);
    });

    ctx.cancel.cancelled().await;
    // Ordinarily unbounded, and deliberately so (04 §2): bounding every drain
    // would turn a wedge into a silent leak of whatever the unjoined task owns.
    // After a handoff commits that trade reverses — this process owns nothing
    // the session needs any more — so the drain gets a deadline, the census,
    // and a non-zero exit instead of a silent park. See `handoff::export::drain`
    // and `docs/notes/m3-shutdown-wedge.md` §6.
    let shutdown = if handoff.fenced() {
        export::bounded_drain(runtime, &ctx, POST_COMMIT_DRAIN).await?
    } else {
        runtime.shutdown().await
    };
    let gateway = report_rx.await.unwrap_or_default();
    Ok(ServeReport { gateway, shutdown })
}
