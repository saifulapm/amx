//! W06: the export path — a running server hands itself over, or aborts back
//! to a running server.
//!
//! `docs/09-m3-plan.md` §3 is the protocol, D-M3-6 is where it departs from
//! herdr's, and R-M3-10 is the one hazard that has no equivalent there. The
//! suite drives the real thing at every layer it can: a real session socket, a
//! real `Core` with real ptys behind its panes, the real gateway retiring and
//! restoring, the real persistence actor. The only stand-in is the successor
//! itself, because the successor is `amx server --handoff-import` and that is
//! W07's — so [`Fake`](importer::Fake) drives W05's importer state machine over
//! the same socket, through the seam the orchestrator takes for exactly this.
//!
//! The abort matrix is asserted against `HandoffError::ending()` rather than
//! re-derived: §3's crash table has exactly two survivor states and W05 already
//! computes which one a stage falls into, so a table written out again here
//! would be a second opinion nobody asked for.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

// A test crate root's module directory is `tests/`, so these two need their
// paths spelled out to live beside their one suite rather than next to
// everybody's.
#[path = "handoff_export/importer.rs"]
mod importer;
#[path = "handoff_export/rig.rs"]
mod rig;

use std::os::unix::fs::MetadataExt as _;
use std::time::Duration;

use amx_proto::control::session::{HandoffOutcome, HandoffStage};
use amx_server::handoff::export::{self, Caps};
use amx_server::handoff::protocol::{Ending, HandoffError, Stage};
use amx_server::runtime::{CENSUS_FILE, Runtime};
use amx_server::session::probe::{self, Probe};
use importer::{Fake, Seen, StopAt};
use rig::{Rig, TempDir, ctx_under, fixture_binary, wait_until};

/// A stage bound short enough that a stalled test fails rather than hangs, and
/// long enough that a loaded runner is not mistaken for one.
const STAGE: Duration = Duration::from_secs(10);

/// What a successor that can be handed this session prints.
fn good_caps() -> String {
    let caps = Caps::of_this_build();
    format!(
        "echo '{}'",
        serde_json::to_string(&caps).expect("encode caps")
    )
}

/// What a successor that reads a schema this build does not write prints.
fn stale_caps() -> String {
    let mut caps = Caps::of_this_build();
    caps.handoff = (99, 99);
    caps.version = "9.9.9".to_owned();
    format!(
        "echo '{}'",
        serde_json::to_string(&caps).expect("encode caps")
    )
}

// ------------------------------------------------------------- the whole walk

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handoff_to_a_fake_importer_walks_every_stage_in_order() {
    let mut rig = Rig::start("walk").await;
    let root = rig.panes().await[0];
    rig.split_echoing(root).await;
    let binary = fixture_binary(rig.root(), "successor", &good_caps());

    let reply = rig.handoff(&binary, Some(10_000)).await;
    assert!(reply.accepted, "the pre-flight passed: {reply:?}");
    assert!(reply.reason.is_none());

    let job = rig.jobs.recv().await.expect("core froze the session");
    assert_eq!(
        job.manifest.panes.len(),
        2,
        "both live panes are in the manifest",
    );
    assert_eq!(
        job.masters.len(),
        job.manifest.panes.len(),
        "one descriptor per manifest entry, in transfer order",
    );
    assert_eq!(
        job.manifest.session,
        session_id_of(&rig).await,
        "the successor continues this session's identity, not a fresh one",
    );
    assert!(
        job.manifest.seq > 0,
        "the bus head crosses so the successor's sequence numbers continue",
    );

    let (fake, log) = Fake::new(rig.ctx.socket.clone());
    let attempt = export::run(job, &rig.control, Box::new(fake.within(STAGE))).await;

    assert_eq!(attempt.outcome, HandoffOutcome::Committed, "{attempt:?}");
    assert_eq!(attempt.stage, HandoffStage::Commit, "{attempt:?}");
    assert_eq!(attempt.reason, None, "a committed handoff needs no reason");

    let seen: Vec<Seen> = log.iter().collect();
    assert_eq!(
        seen,
        vec![
            Seen::Authenticated,
            Seen::Manifest(2),
            Seen::Masters(2),
            Seen::SocketBeforeRestored(Probe::Running),
            Seen::Restored,
            Seen::SocketAfterRetire(Probe::Absent),
            Seen::FileAfterRetire(false),
            Seen::Ready,
            Seen::Committed,
            Seen::Owned,
        ],
        "§3's stages, in §3's order, each observed from the successor's side",
    );

    assert!(rig.ledger.fenced(), "the commit fenced persistence");
    let reported = rig
        .reported_handoff()
        .await
        .expect("a handoff that happened is in session.report");
    assert_eq!(reported.outcome, HandoffOutcome::Committed);
    assert_eq!(reported.binary, binary);

    assert!(
        rig.stop().await.shutdown.clean(),
        "the session stopped without a panicked task"
    );
}

// -------------------------------------------------------------- the pre-flight

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_flight_refuses_before_any_pane_is_touched() {
    let mut rig = Rig::start("pref").await;
    let root = rig.panes().await[0];
    let echoing = rig.split_echoing(root).await;
    assert!(
        rig.echoes(echoing, "before-the-refusal").await,
        "the pane is live to begin with",
    );

    // A successor that reads a manifest schema this build does not write.
    let stale = fixture_binary(rig.root(), "stale", &stale_caps());
    let reply = rig.handoff(&stale, None).await;
    assert!(!reply.accepted, "{reply:?}");
    let reason = reply.reason.clone().expect("a refusal says why");
    assert!(
        reason.contains("99..=99") && reason.contains("nothing was touched"),
        "the refusal names both windows: {reason}",
    );

    // A binary that is not there at all.
    let missing = rig.root().join("never-built");
    let gone = rig.handoff(&missing, None).await;
    assert!(!gone.accepted, "{gone:?}");
    assert!(
        gone.reason
            .as_deref()
            .is_some_and(|why| why.contains("never-built")),
        "{gone:?}",
    );

    // Nothing was frozen: no job was queued, and the pane still echoes without
    // anything having had to resume it.
    assert!(
        rig.jobs.try_recv().is_err(),
        "a refused pre-flight never freezes the session",
    );
    assert!(
        rig.echoes(echoing, "after-the-refusal").await,
        "a refused handoff leaves every pane exactly as it was",
    );
    assert!(
        probe::probe(&rig.ctx.socket).expect("probe").is_running(),
        "and the session socket never stopped answering",
    );

    let reported = rig
        .reported_handoff()
        .await
        .expect("a refusal is in session.report");
    assert_eq!(reported.outcome, HandoffOutcome::Refused);
    assert_eq!(reported.stage, HandoffStage::PreFlight);
    assert_eq!(reported.binary, missing);
    assert!(reported.reason.is_some());

    assert!(
        rig.stop().await.shutdown.clean(),
        "the session stopped without a panicked task"
    );
}

// ------------------------------------------------------------- the abort matrix

/// Where a fake importer stops, the protocol stage the exporter fails at, and
/// the last stage `session.report` should say completed.
const MATRIX: &[(StopAt, Stage, HandoffStage)] = &[
    (StopAt::Authenticating, Stage::Token, HandoffStage::Quiesce),
    (StopAt::Validating, Stage::Validated, HandoffStage::Quiesce),
    (
        StopAt::Restoring,
        Stage::Restored,
        HandoffStage::Descriptors,
    ),
    (StopAt::Binding, Stage::Ready, HandoffStage::Retire),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abort_at_any_pre_commit_stage_resumes_every_pane_and_reaccepts() {
    for (stop_at, stage, completed) in MATRIX {
        // W05 computes §3's crash table; this is the assertion that the three
        // stages driven below really are the pre-commit ones, taken from there
        // rather than restated.
        assert_eq!(
            HandoffError::Aborted {
                stage: *stage,
                reason: "the peer said so".to_owned(),
            }
            .ending(),
            Ending::Abort,
            "{stage} is a pre-commit stage",
        );

        let mut rig = Rig::start("abrt").await;
        let root = rig.panes().await[0];
        let echoing = rig.split_echoing(root).await;
        assert!(rig.echoes(echoing, "before-the-abort").await);
        let binary = fixture_binary(rig.root(), "successor", &good_caps());

        let reply = rig.handoff(&binary, Some(10_000)).await;
        assert!(reply.accepted, "{reply:?}");
        let job = rig.jobs.recv().await.expect("core froze the session");
        let panes = job.panes.len();
        assert_eq!(
            panes, 2,
            "both panes were quiesced before anything was sent"
        );

        let (fake, _log) = Fake::new(rig.ctx.socket.clone());
        let attempt = export::run(
            job,
            &rig.control,
            Box::new(fake.stopping_at(*stop_at).within(STAGE)),
        )
        .await;

        assert_eq!(
            attempt.outcome,
            HandoffOutcome::Aborted,
            "{stop_at:?}: {attempt:?}",
        );
        assert_eq!(attempt.stage, *completed, "{stop_at:?}: {attempt:?}");
        assert!(
            attempt.reason.is_some(),
            "{stop_at:?}: an abort says why: {attempt:?}",
        );
        assert!(
            !rig.ledger.fenced(),
            "{stop_at:?}: nothing before the commit may fence persistence",
        );

        // Clients connect again — which for the `Binding` row means the
        // gateway took its own socket back after having retired it.
        wait_until(&format!("{stop_at:?}: the socket answers again"), || {
            probe::probe(&rig.ctx.socket).is_ok_and(|found| found.is_running())
        })
        .await;

        // Panes echo again.
        assert!(
            rig.echoes(echoing, "after-the-abort").await,
            "{stop_at:?}: an aborted handoff resumes every pane it froze",
        );
        assert!(
            rig.echoes(root, "root-after-the-abort").await,
            "{stop_at:?}: including the ones nothing else asserts about",
        );

        // And `session report` names the reason.
        let reported = rig
            .reported_handoff()
            .await
            .expect("an aborted handoff is in session.report");
        assert_eq!(reported.outcome, HandoffOutcome::Aborted);
        assert_eq!(reported.stage, *completed);
        assert_eq!(reported.reason, attempt.reason);

        // The session is claimable again, so a second attempt is possible.
        let second = rig.handoff(&binary, Some(10_000)).await;
        assert!(
            second.accepted,
            "{stop_at:?}: an aborted handoff releases the session: {second:?}",
        );
        let job = rig.jobs.recv().await.expect("core froze the session again");
        drop(job);

        assert!(
            rig.stop().await.shutdown.clean(),
            "the session stopped without a panicked task"
        );
    }
}

// ---------------------------------------------------------------- the socket

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_answers_until_restored_and_is_gone_after_retire() {
    let mut rig = Rig::start("sock").await;
    assert!(
        probe::probe(&rig.ctx.socket).expect("probe").is_running(),
        "the session is serving before anything is handed over",
    );
    let binary = fixture_binary(rig.root(), "successor", &good_caps());

    let reply = rig.handoff(&binary, Some(10_000)).await;
    assert!(reply.accepted, "{reply:?}");
    let job = rig.jobs.recv().await.expect("core froze the session");
    // Frozen, and still answering: the retirement is deliberately late, which
    // is what narrows R-M3-5 to the unlink-to-bind gap.
    assert!(
        probe::probe(&rig.ctx.socket).expect("probe").is_running(),
        "a frozen session still answers its socket (§3 step 4)",
    );

    let (fake, log) = Fake::new(rig.ctx.socket.clone());
    let attempt = export::run(job, &rig.control, Box::new(fake.within(STAGE))).await;
    assert_eq!(attempt.outcome, HandoffOutcome::Committed, "{attempt:?}");

    let seen: Vec<Seen> = log.iter().collect();
    assert!(
        seen.contains(&Seen::SocketBeforeRestored(Probe::Running)),
        "the exporter was still serving the instant before `restored`: {seen:?}",
    );
    assert!(
        seen.contains(&Seen::SocketAfterRetire(Probe::Absent)),
        "and nothing answered it once the gateway retired: {seen:?}",
    );
    assert!(
        seen.contains(&Seen::FileAfterRetire(false)),
        "the socket file itself is unlinked, not merely silent: {seen:?}",
    );

    assert!(
        rig.stop().await.shutdown.clean(),
        "the session stopped without a panicked task"
    );
}

// ------------------------------------------------------------- R-M3-10's fence

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_final_snapshot_is_written_after_commit() {
    // The exporter, handed over. Its shutdown must write nothing.
    //
    // The session is changed (a second pane) immediately before the handoff, so
    // that the exporter's dying view genuinely differs from whatever is on disk
    // — a final capture identical to the last save is skipped by `Persist`
    // itself, and a test built on one would pass with the fence removed.
    let mut rig = Rig::start("fenc").await;
    let root = rig.panes().await[0];
    rig.split_echoing(root).await;
    let binary = fixture_binary(rig.root(), "successor", &good_caps());

    let reply = rig.handoff(&binary, Some(10_000)).await;
    assert!(reply.accepted, "{reply:?}");
    let job = rig.jobs.recv().await.expect("core froze the session");
    let (fake, _log) = Fake::new(rig.ctx.socket.clone());
    let attempt = export::run(job, &rig.control, Box::new(fake.within(STAGE))).await;
    assert_eq!(attempt.outcome, HandoffOutcome::Committed, "{attempt:?}");
    assert!(rig.ledger.fenced(), "the commit armed the fence");

    // The debounced save is fenced at the same place and deterministically:
    // `Core` answers no capture at all past the commit, so a save that fires
    // between the commit and the exit writes nothing either.
    assert!(
        rig.persist
            .flush()
            .await
            .expect("persistence answered")
            .is_err(),
        "past the commit the snapshot belongs to the successor, so `Core` \
         refuses to be captured at all",
    );

    // The inode, not the mtime: every save is a temp file renamed over the
    // target, so a write that happened is a file that is a different file.
    let snapshot = rig.snapshot();
    let before = inode(&snapshot);
    let stopped = rig.stop().await;
    let after = inode(&snapshot);
    assert_eq!(
        before, after,
        "the exporter's dying view would have overwritten the successor's \
         session.json (R-M3-10); the final push must stay fenced",
    );
    drop(stopped);

    // And the control: the same shutdown, with no handoff, does write one —
    // so the assertion above is about the fence and not about persistence
    // having quietly stopped working. The change comes *after* the reading, so
    // the shutdown has something new to say whether or not the debounce
    // happened to fire first.
    let control = Rig::start("ctrl").await;
    let snapshot = control.snapshot();
    let before = inode(&snapshot);
    let root = control.panes().await[0];
    control.split_echoing(root).await;
    let stopped = control.stop().await;
    let after = inode(&snapshot);
    drop(stopped);
    assert_ne!(
        before, after,
        "an ordinary shutdown pushes a final capture; if it does not, the \
         fence above is proving nothing",
    );
}

/// The snapshot file's identity, or `None` if it has never been written.
fn inode(path: &std::path::Path) -> Option<u64> {
    path.metadata().ok().map(|found| found.ino())
}

// --------------------------------------------------------- W01's drain watchdog

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drain_after_commit_is_bounded() {
    let dir = TempDir::new("drn");
    let ctx = ctx_under(dir.path());

    // A healthy drain is not bounded into failing: it returns what it joined.
    let mut healthy = Runtime::new(ctx.clone());
    let token = ctx.cancel.clone();
    healthy.spawn("obedient", async move { token.cancelled().await });
    let report = export::bounded_drain(healthy, &ctx, Duration::from_secs(5))
        .await
        .expect("a task that returns on cancellation is joined");
    assert_eq!(report.joined, 1);

    // And one that will not return is a wedge, named rather than waited on.
    let ctx = ctx_under(dir.path());
    let mut wedged = Runtime::new(ctx.clone());
    let token = ctx.cancel.clone();
    wedged.spawn("obedient", async move { token.cancelled().await });
    wedged.spawn("stubborn", stubborn());
    let bound = Duration::from_millis(400);
    let err = export::bounded_drain(wedged, &ctx, bound)
        .await
        .expect_err("a task that ignores cancellation is not waited on forever");

    assert_eq!(err.waited, bound);
    assert!(
        err.census.contains("stubborn"),
        "the watchdog names the actor that held the drain, which is what makes \
         it a diagnosis rather than a timeout: {}",
        err.census,
    );
    assert!(
        !err.census.contains("obedient"),
        "and names only what was still outstanding: {}",
        err.census,
    );
    assert!(
        err.census.contains("pid"),
        "the census identifies the process it is about: {}",
        err.census,
    );
    assert!(
        ctx.runtime_dir.join(CENSUS_FILE).exists(),
        "the census is on disk too, because a daemonized server's stderr is \
         /dev/null",
    );
    assert!(
        err.to_string().contains("after the handoff committed"),
        "the error says which drain this was: {err}",
    );
}

/// A task that never observes cancellation.
///
/// The shape W01 found in the field twice over: something parked on an await
/// that watches no token. Standing in for it here rather than hunting the real
/// one, because what is under test is the bound, not the bug.
async fn stubborn() {
    std::future::pending::<()>().await;
}

/// This session's identity, as `ping` reports it.
async fn session_id_of(rig: &Rig) -> amx_core::SessionId {
    use amx_proto::control::session::PingParams;
    use amx_server::actor::{CoreCommand, SessionCall};

    let (reply, answer) = tokio::sync::oneshot::channel();
    rig.core
        .send(CoreCommand::Session(SessionCall::Ping {
            params: PingParams {},
            reply,
        }))
        .await
        .expect("core mailbox is open");
    answer
        .await
        .expect("core answered")
        .expect("ping answers")
        .session
}
