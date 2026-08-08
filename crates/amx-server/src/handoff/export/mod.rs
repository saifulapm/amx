//! The export orchestrator: a running server hands itself over, or aborts back
//! to a running server.
//!
//! `docs/09-m3-plan.md` §3 is the protocol and D-M3-6 is why it departs from
//! herdr's where it does. [`protocol`](crate::handoff::protocol) makes the
//! ordering unrepresentable; this module is what actually walks it, and what
//! owns the four things the state machine deliberately does not:
//!
//! 1. **the pre-flight** ([`caps`]), which runs on the connection task that
//!    asked for the handoff, before `Core` has heard of it — so "refused before
//!    any pane is touched" is a fact about which code has run, not about the
//!    order of statements inside one function;
//! 2. **the freeze**, which is `Core`'s
//!    ([`actor::core`](crate::actor::core)): quiesce every pane, capture each
//!    one on its parser thread, and hand this module a manifest and a
//!    descriptor per pane;
//! 3. **the retirement** (§3 step 11), which is the gateway's
//!    ([`GatewayControl`]) and is deliberately late — the session socket
//!    answers until the successor says `restored`, which is what narrows
//!    R-M3-5's window to the unlink-to-bind gap;
//! 4. **the fence** (§3 step 13, R-M3-10), which is [`Ledger::fence`] and is
//!    armed *before* `committed` goes out, because the hazard is a push that
//!    happens while nobody is looking.
//!
//! # Blocking, and where it is done
//!
//! Both of W05's state machines are blocking calls over a
//! `std::os::unix::net::UnixStream`: ancillary data has no async surface in
//! rustix, and a buffered reader on that socket would swallow the byte a
//! descriptor rides beside. So every transition here runs inside
//! `spawn_blocking`, in three regions — steps 1–10, step 12, and steps 13–15 —
//! with the retirement and the fence in the async gaps between them, which is
//! exactly where §3 puts them.
//!
//! # What the exporter does at the end
//!
//! Nothing dramatic. After `committed` it reads the advisory `owned` for half a
//! second and cancels the session; the ordinary shutdown releases the pane
//! actors, and the children see no hangup because the importer holds the same
//! open file descriptions (unix(7), D-M3-3). The one thing that is *not*
//! ordinary is the drain: see [`drain`].

pub mod caps;
pub mod drain;
pub mod ledger;

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;

use amx_core::{Ctx, PaneId};
use amx_proto::control::session::{HandoffAttempt, HandoffOutcome, HandoffStage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use caps::{Caps, PreFlightError, StagedBinary, Successor, pre_flight};
pub use drain::{DrainWedged, POST_COMMIT_DRAIN, bounded as bounded_drain};
pub use ledger::Ledger;

use super::manifest::Manifest;
use super::protocol::{
    Exporter, HandoffError, HandoffListener, Stage, Timeouts, Token, exporter, socket_path,
};
use crate::actor::gateway::GatewayControl;
use crate::pty::PtyActorHandle;

/// How many handoff jobs may queue for the orchestrator.
///
/// One: a session hands itself over once, and [`Ledger::claim`] refuses the
/// second caller before a job is ever made.
pub const JOB_MAILBOX: usize = 1;

/// One frozen pane, as far as the export path is concerned.
///
/// `Core` owns the `PaneHost`; what travels here is the pty actor's own
/// mailbox, which is the part [`PaneHost::quiesce`](crate::actor::PaneHost::quiesce)
/// and [`resume`](crate::actor::PaneHost::resume) send on. Both block, so the
/// orchestrator's rollback runs them on the blocking pool rather than on a
/// runtime thread — and a `PaneHost` could not go there anyway, because `Core`
/// is still serving with it.
#[derive(Clone, Debug)]
pub struct FrozenPane {
    /// Which pane this is, for the log line a failed resume produces.
    pub pane: PaneId,
    /// Its pty actor.
    pub pty: PtyActorHandle,
}

/// Everything §3 needs once the panes are frozen and captured.
///
/// Built by `Core` in one mailbox turn — the session is only ever frozen for
/// the length of that turn — and handed here whole.
#[derive(Debug)]
pub struct Job {
    /// The session's paths, bus and cancellation token.
    pub ctx: Ctx,
    /// The staged binary, already pre-flighted.
    pub binary: PathBuf,
    /// The stage bounds this attempt runs under.
    pub timeouts: Timeouts,
    /// The session, as bytes.
    pub manifest: Manifest,
    /// One duplicate master per manifest entry, in the same order.
    pub masters: Vec<OwnedFd>,
    /// Every pane that was quiesced, for the rollback.
    pub panes: Vec<FrozenPane>,
    /// The fence, the in-flight flag and the attempt slot.
    pub ledger: Arc<Ledger>,
}

/// The orchestrator task: run one export at a time, forever.
///
/// Spawned by the serve assembly through
/// [`Runtime::spawn`](crate::runtime::Runtime::spawn) and named there, so a
/// drain that is waiting on a handoff says so. A committed handoff cancels the
/// session and this task returns with it; anything else leaves the server
/// serving and the task waiting for the next job that will almost certainly
/// never come.
pub async fn orchestrate(
    mut jobs: mpsc::Receiver<Job>,
    gateway: GatewayControl,
    cancel: CancellationToken,
) {
    loop {
        let job = tokio::select! {
            () = cancel.cancelled() => break,
            job = jobs.recv() => match job {
                Some(job) => job,
                None => break,
            },
        };
        let ctx = job.ctx.clone();
        let successor = Box::new(StagedBinary::new(job.binary.clone()));
        let committed = run(job, &gateway, successor).await.outcome == HandoffOutcome::Committed;
        if committed {
            // Ownership has moved; this process's only remaining job is to
            // stop holding descriptors of a terminal it no longer serves.
            ctx.cancel.cancel();
            break;
        }
    }
}

/// Walk §3 steps 1–15 against `successor`, and say what happened.
///
/// Public so that a test can drive it against a peer that is not a process.
/// Never returns without having either resumed every pane it was given or
/// committed: the two endings [`Ending`](super::protocol::Ending) names are the
/// only two this function has.
///
/// The outcome is written to the ledger here rather than by the caller, because
/// the caller that matters — the connection that asked for the handoff — is
/// disconnected by the gateway's retirement several stages before there is an
/// outcome to hand back. `session.report` is where it is read (D-M3-8).
pub async fn run(
    job: Job,
    gateway: &GatewayControl,
    successor: Box<dyn Successor>,
) -> HandoffAttempt {
    let ledger = Arc::clone(&job.ledger);
    let attempt = walk(job, gateway, successor).await;
    ledger.record(attempt.clone());
    ledger.release();
    attempt
}

/// §3 itself, from the first blocking region to the last.
async fn walk(job: Job, gateway: &GatewayControl, successor: Box<dyn Successor>) -> HandoffAttempt {
    let Job {
        ctx,
        binary,
        timeouts,
        manifest,
        masters,
        panes,
        ledger,
    } = job;

    let document = match serde_json::to_value(&manifest) {
        Ok(document) => document,
        Err(err) => {
            resume_all(panes).await;
            return aborted(
                binary,
                HandoffStage::Quiesce,
                format!("the manifest could not be encoded: {err}"),
            );
        }
    };
    let socket = socket_path(&ctx.runtime_dir, std::process::id());
    let listener = match HandoffListener::bind(socket.clone()) {
        Ok(listener) => listener,
        Err(err) => {
            resume_all(panes).await;
            return aborted(
                binary,
                HandoffStage::Quiesce,
                format!(
                    "the handoff socket {} could not be bound: {err}",
                    socket.display()
                ),
            );
        }
    };
    let token = Token::mint();

    // ------------------------------------------------ steps 1–10, blocking
    let opening = tokio::task::spawn_blocking(move || {
        open(Opening {
            listener,
            successor,
            socket,
            token,
            timeouts,
            document,
            masters,
        })
    })
    .await;
    let mut reached = match opening {
        Ok(Ok(reached)) => reached,
        Ok(Err(err)) => {
            resume_all(panes).await;
            return aborted(binary, last_completed(err.stage()), err.to_string());
        }
        Err(join) => {
            resume_all(panes).await;
            return aborted(
                binary,
                HandoffStage::Quiesce,
                format!("the handoff task did not finish: {join}"),
            );
        }
    };
    if ctx.cancel.is_cancelled() {
        resume_all(panes).await;
        return aborted(
            binary,
            HandoffStage::Restore,
            "the session was stopped while it was handing itself over".to_owned(),
        );
    }

    // --------------------------------------------------- step 11: retiring
    if let Err(err) = gateway.retire().await {
        resume_all(panes).await;
        return aborted(binary, HandoffStage::Restore, err.to_string());
    }

    // ------------------------------------------------- step 12, blocking
    let ready = tokio::task::spawn_blocking(move || reached.exporter.await_ready()).await;
    let exporter = match ready {
        Ok(Ok(exporter)) => exporter,
        Ok(Err(err)) => {
            let reason = err.to_string();
            restore_socket(gateway).await;
            resume_all(panes).await;
            return aborted(binary, last_completed(err.stage()), reason);
        }
        Err(join) => {
            restore_socket(gateway).await;
            resume_all(panes).await;
            return aborted(
                binary,
                HandoffStage::Retire,
                format!("the handoff task did not finish: {join}"),
            );
        }
    };

    // --------------------------------------- step 13: the fence, then commit
    //
    // Armed before the word goes out, never after: the whole hazard R-M3-10
    // names is a final capture that lands while nobody is looking.
    ledger.fence();
    let committed = tokio::task::spawn_blocking(move || {
        exporter.commit().map(|exporter| exporter.await_owned())
    })
    .await;
    match committed {
        Ok(Ok(acked)) => {
            reached.successor.disown();
            // The exporter's own copies close here. The child sees no hangup:
            // the importer holds the same open file descriptions (unix(7)).
            drop(reached.masters);
            tracing::info!(
                binary = %binary.display(),
                acked,
                "the session has been handed over; this server is done",
            );
            HandoffAttempt {
                outcome: HandoffOutcome::Committed,
                stage: HandoffStage::Commit,
                binary,
                reason: None,
            }
        }
        Ok(Err(err)) => {
            // §3's table puts a failed commit on the abort side: the importer
            // never heard the word, so it strict-aborts and this server is the
            // one that owns the snapshot again.
            ledger.unfence();
            let reason = err.to_string();
            restore_socket(gateway).await;
            resume_all(panes).await;
            aborted(binary, last_completed(err.stage()), reason)
        }
        Err(join) => {
            ledger.unfence();
            restore_socket(gateway).await;
            resume_all(panes).await;
            aborted(
                binary,
                HandoffStage::Ready,
                format!("the handoff task did not finish: {join}"),
            )
        }
    }
}

/// What steps 1–10 leave the exporter holding.
struct Reached {
    exporter: Exporter<exporter::Retiring>,
    /// Kept alive so it is not killed: a dropped [`StagedBinary`] reaps its
    /// child, which is right at every stage except the one after the commit.
    successor: Box<dyn Successor>,
    /// Kept alive so the descriptors are not closed under the importer before
    /// it has taken its own references.
    masters: Vec<OwnedFd>,
}

/// What the blocking half of steps 1–10 is handed.
struct Opening {
    listener: HandoffListener,
    successor: Box<dyn Successor>,
    socket: PathBuf,
    token: Token,
    timeouts: Timeouts,
    document: serde_json::Value,
    masters: Vec<OwnedFd>,
}

/// §3 steps 1–10, every one of them a blocking call.
fn open(opening: Opening) -> Result<Reached, HandoffError> {
    let Opening {
        listener,
        mut successor,
        socket,
        token,
        timeouts,
        document,
        masters,
    } = opening;
    successor
        .start(&socket, &token)
        .map_err(|source| HandoffError::Io {
            stage: Stage::Token,
            source,
        })?;
    let exporter = listener
        .accept(token, timeouts)?
        .authenticate()?
        .send_manifest(document)?
        .await_validated()?;
    let borrowed: Vec<BorrowedFd<'_>> = masters.iter().map(AsFd::as_fd).collect();
    let exporter = exporter.send_masters(&borrowed)?;
    drop(borrowed);
    let exporter = exporter.await_restored()?;
    Ok(Reached {
        exporter,
        successor,
        masters,
    })
}

/// Undo the freeze: every pane the capture quiesced writes and reads again.
///
/// Blocking, on the blocking pool, and best effort per pane — a pane whose
/// actor has already gone is a pane that resumed by ending, and refusing to
/// resume the others because of it would turn one dead pane into a frozen
/// session.
async fn resume_all(panes: Vec<FrozenPane>) {
    if panes.is_empty() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        for pane in panes {
            if let Err(err) = pane.pty.resume() {
                tracing::warn!(pane = %pane.pane, error = %err, "a pane could not be resumed");
            }
        }
    })
    .await;
}

/// Take the session socket back after an abort that had already retired it.
async fn restore_socket(gateway: &GatewayControl) {
    if let Err(err) = gateway.restore().await {
        tracing::error!(
            error = %err,
            "the handoff aborted and this server could not take its socket back",
        );
    }
}

/// The attempt an abort produces.
fn aborted(binary: PathBuf, stage: HandoffStage, reason: String) -> HandoffAttempt {
    tracing::warn!(
        binary = %binary.display(),
        ?stage,
        reason = %reason,
        "the handoff aborted; this session goes on serving",
    );
    HandoffAttempt {
        outcome: HandoffOutcome::Aborted,
        stage,
        binary,
        reason: Some(reason),
    }
}

/// The last stage that *completed*, given the protocol stage that failed.
///
/// `session.report` says how far an attempt got, and a failure at stage N means
/// N−1 was the last thing that worked. The freeze is already behind every stage
/// this table covers — `Core` quiesces and captures before the orchestrator is
/// handed anything — which is why the earliest entry is
/// [`HandoffStage::Quiesce`] and not [`HandoffStage::PreFlight`].
const fn last_completed(stage: Stage) -> HandoffStage {
    match stage {
        Stage::Token | Stage::Manifest | Stage::Validated => HandoffStage::Quiesce,
        Stage::Masters => HandoffStage::Manifest,
        Stage::Restored => HandoffStage::Descriptors,
        // The gateway is retired before `await_ready` is called, so a failure
        // there is a failure with the socket already handed over.
        Stage::Ready => HandoffStage::Retire,
        Stage::Committed => HandoffStage::Ready,
        Stage::Owned => HandoffStage::Commit,
    }
}
