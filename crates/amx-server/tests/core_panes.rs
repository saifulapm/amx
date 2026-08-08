//! The `Core`↔`PaneHost` liveness contracts: saturated mailboxes must never
//! deadlock the session, close must kill even when the pane's mailbox is
//! full, and `Core::run` joins every pane task before it returns.
//!
//! Every test here runs on the current-thread scheduler on purpose: nothing
//! runs concurrently with the test body, so a mailbox filled with `try_send`
//! stays full until the test starts awaiting — which is what makes "both
//! mailboxes are full at the same instant" a constructed fact rather than a
//! race the test hopes to hit.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use amx_core::{
    Bus, Ctx, Delivery, Event, GridGeneration, InvalidationCause, PaneId, RowId, RowRange,
    Scheduled, SessionName, Subscription,
};
use amx_proto::control::{pane, workspace};
use amx_server::actor::core::Core;
use amx_server::actor::{
    CoreCommand, CoreHandle, PaneCall, PaneCommand, PaneReport, PaneWiring, StreamCall,
    WorkspaceCall,
};
use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// How long a test waits for something that must happen.
const PATIENCE: Duration = Duration::from_secs(15);

/// A quiet command for panes that must produce no output of their own.
fn silent() -> Option<Vec<String>> {
    Some(vec!["/bin/sh".into(), "-c".into(), "sleep 300".into()])
}

/// A `Ctx` with fabricated, never-touched-on-disk paths.
fn fresh_ctx(tag: &str) -> Ctx {
    let root = PathBuf::from("/amx-test-core-panes").join(tag);
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        runtime_dir: root.join("runtime"),
        socket: root.join("runtime/sock"),
        state_dir: root.join("state"),
        config_path: root.join("config/amx/config.toml"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

/// A `Core` on the real actor loop with a mailbox the test controls the
/// depth of, so it can be saturated deterministically.
struct Harness {
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    task: JoinHandle<Core>,
}

impl Harness {
    fn start(tag: &str, mailbox: usize) -> Self {
        let ctx = fresh_ctx(tag);
        let (tx, rx) = mpsc::channel(mailbox);
        let core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));
        let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));
        Self { ctx, tx, task }
    }

    /// Create a workspace and return its root pane.
    async fn seed_workspace(&self) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Workspace(WorkspaceCall::Create {
                params: workspace::CreateParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        let created = answer
            .await
            .expect("core answered")
            .expect("create succeeds");
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Session(
                amx_server::actor::SessionCall::State {
                    params: amx_proto::control::session::StateParams::default(),
                    reply,
                },
            ))
            .await
            .expect("core mailbox is open");
        let state = answer.await.expect("core answered").expect("state answers");
        state
            .workspaces
            .iter()
            .find(|ws| ws.workspace == created.workspace)
            .expect("the workspace just created")
            .focus
            .expect("a fresh workspace focuses its root pane")
    }

    /// Split a silent pane off `from`, with an explicit cwd so the split
    /// itself never asks anyone anything.
    async fn split_silent(&self, from: PaneId) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Pane(PaneCall::Split {
                params: pane::SplitParams {
                    pane: from,
                    direction: pane::SplitDirection::Vertical,
                    command: silent(),
                    cwd: Some(PathBuf::from("/")),
                },
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("split succeeds")
            .pane
    }

    /// Split a pane off `from` running `script` under `/bin/sh`.
    async fn split_running(&self, from: PaneId, script: &str) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Pane(PaneCall::Split {
                params: pane::SplitParams {
                    pane: from,
                    direction: pane::SplitDirection::Vertical,
                    command: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
                    cwd: Some(PathBuf::from("/")),
                },
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("split succeeds")
            .pane
    }

    /// The history window `session.state` answers with for `pane`.
    async fn history_window(&self, pane: PaneId) -> (RowId, RowId) {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Session(
                amx_server::actor::SessionCall::State {
                    params: amx_proto::control::session::StateParams::default(),
                    reply,
                },
            ))
            .await
            .expect("core mailbox is open");
        let state = answer.await.expect("core answered").expect("state answers");
        let row = state
            .panes
            .iter()
            .find(|row| row.pane == pane)
            .expect("the pane is in the state tree");
        (row.history_head, row.history_floor)
    }

    /// Close `pane` through the public verb, which hangs its terminal up.
    async fn close(&self, pane: PaneId) {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Pane(PaneCall::Close {
                params: pane::CloseParams { pane },
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("close succeeds");
    }

    /// The live plumbing of `pane`, over the running loop.
    async fn wiring_of(&self, pane: PaneId) -> PaneWiring {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("the pane is backed by a live actor")
    }

    /// Let every actor drain what the setup queued and go quiet, so the
    /// synchronous phase that follows starts from parked tasks with empty
    /// mailboxes. The ping below proves the Core's mailbox drained; the nap
    /// is for the pane actors, whose parked state has no observable signal.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(300)).await; // deliberate
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Session(amx_server::actor::SessionCall::Ping {
                params: amx_proto::control::session::PingParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        let _ = answer.await.expect("core answered");
    }
}

/// A report filler for saturating the `Core` mailbox.
fn damage_report(pane: PaneId) -> CoreCommand {
    CoreCommand::PaneReport {
        pane,
        report: PaneReport::Damage {
            generation: GridGeneration::FIRST,
        },
    }
}

async fn wait_for_event(events: &mut Subscription, mut want: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let delivery = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an event before the deadline")
            .expect("the bus is open");
        match delivery {
            Delivery::Event(envelope) if want(&envelope.event) => return envelope.event,
            Delivery::Event(_) => {}
            Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
        }
    }
}

/// The Core↔PaneHost deadlock, constructed exactly: both panes are wedged
/// reporting into a full `Core` mailbox when the `Core` starts resolving a
/// split's inherited cwd from one of them. A `Core` that waits on the pane —
/// or on its reply, unbounded — never runs its mailbox again and the session
/// is dead; the fix answers the split from the recorded-cwd fallback instead.
#[tokio::test]
async fn split_makes_progress_while_core_and_panes_are_saturated() {
    let harness = Harness::start("saturated-split", 2);
    let root = harness.seed_workspace().await;
    let a = harness.split_silent(root).await;
    let b = harness.split_silent(root).await;
    let wiring_a = harness.wiring_of(a).await;
    let wiring_b = harness.wiring_of(b).await;
    harness.settle().await;

    // Synchronous phase: nothing below awaits, so nothing gets to drain.
    // Both panes are handed a resize (the command whose fold reports damage),
    // the split is queued, and the mailbox is topped up to capacity. On the
    // old code the wake order — B, A, Core — parks both panes in a blocking
    // report-send against the full mailbox before the Core touches the split,
    // and the one slot the split's recv frees goes to B, leaving A wedged
    // exactly when the Core awaits A's cwd answer forever.
    wiring_b
        .handle
        .try_send(PaneCommand::Resize { rows: 30, cols: 90 })
        .expect("pane B's mailbox has room");
    wiring_a
        .handle
        .try_send(PaneCommand::Resize { rows: 31, cols: 91 })
        .expect("pane A's mailbox has room");
    let (reply, answer) = oneshot::channel();
    harness
        .tx
        .try_send(CoreCommand::Pane(PaneCall::Split {
            params: pane::SplitParams {
                pane: a,
                direction: pane::SplitDirection::Horizontal,
                command: silent(),
                // The point of the test: no override, so the Core must
                // resolve the foreground cwd of a pane that may be wedged.
                cwd: None,
            },
            reply,
        }))
        .expect("the split fits the core mailbox");
    while harness.tx.try_send(damage_report(a)).is_ok() {}

    let split = tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("the split must complete: a saturated session may degrade, never deadlock")
        .expect("core answered")
        .expect("split succeeds");
    assert_ne!(split.pane, a);

    harness.ctx.cancel.cancel();
    let _ = harness.task.await;
}

/// A full pane mailbox must not be able to swallow a close: the old
/// `try_send(Kill)` was dropped exactly then, orphaning the child behind a
/// success reply. The hang-up path bypasses the mailbox entirely.
#[tokio::test]
async fn close_kills_the_pane_even_when_its_mailbox_is_full() {
    let harness = Harness::start("close-full-mailbox", 64);
    let root = harness.seed_workspace().await;
    let b = harness.split_silent(root).await;
    let wiring_b = harness.wiring_of(b).await;
    harness.settle().await;
    let mut events = harness.ctx.bus.subscribe();

    // Synchronous phase: the close is queued first (so the Core serves it
    // before the pane gets a chance to drain), then the pane's mailbox is
    // filled to capacity with writes.
    let (reply, answer) = oneshot::channel();
    harness
        .tx
        .try_send(CoreCommand::Pane(PaneCall::Close {
            params: pane::CloseParams { pane: b },
            reply,
        }))
        .expect("the close fits the core mailbox");
    let mut queued = 0_u32;
    while wiring_b
        .handle
        .try_send(PaneCommand::Write(Bytes::from_static(b"x")))
        .is_ok()
    {
        queued += 1;
    }
    assert!(
        queued > 0,
        "the pane mailbox must be full when the close runs"
    );

    tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("a close reply before the deadline")
        .expect("core answered")
        .expect("close succeeds");

    // The child dies even though no Kill could have fit the mailbox: the
    // exit arrives on the bus, which only happens over a dead pty.
    let exited = wait_for_event(
        &mut events,
        |event| matches!(event, Event::PaneExited { pane, .. } if *pane == b),
    )
    .await;
    assert!(matches!(exited, Event::PaneExited { .. }));

    harness.ctx.cancel.cancel();
    let _ = harness.task.await;
}

/// 04 §2: nothing detached, everything joined. `Core::run` returning is the
/// moment every pane task must already be gone — a `Shutdown` command fires
/// no cancellation token, so on the old code the pane actor simply kept
/// running with nobody left to join it.
#[tokio::test]
async fn core_run_joins_every_pane_task_before_returning() {
    let harness = Harness::start("join-on-shutdown", 64);
    let root = harness.seed_workspace().await;
    let wiring = harness.wiring_of(root).await;
    assert!(!wiring.handle.is_closed(), "the pane actor is serving");

    harness
        .tx
        .send(CoreCommand::Shutdown)
        .await
        .expect("core mailbox is open");
    let core = tokio::time::timeout(PATIENCE, harness.task)
        .await
        .expect("the core loop exits")
        .expect("the core task did not panic");

    assert!(
        wiring.handle.is_closed(),
        "the pane task must be joined before Core::run returns"
    );
    assert!(
        core.pane_handle(root).is_none(),
        "no pane host survives the drain"
    );
}

// ------------------------------------------------- one publisher per event kind

/// How long the bus must stay quiet before a transition counts as announced
/// once and only once.
///
/// The duplicate this pins against arrived *behind* the pane actor's own
/// publish — the actor publishes, then reports, then `Core` folds — so a test
/// that stopped reading at the first envelope would have passed against the
/// bug. It stops reading at silence instead.
const QUIET: Duration = Duration::from_millis(500);

/// A pane whose whole life is the transitions D-M3-2 names: it titles itself,
/// fills the grid past its 24 rows so scrollback commits, and exits.
const FOUR_TRANSITIONS: &str = concat!(
    r"printf '\033]0;one-publisher\007'; ",
    r#"i=0; while [ $i -lt 80 ]; do echo "row $i"; i=$((i+1)); done; "#,
    "sleep 600",
);

/// The title [`FOUR_TRANSITIONS`] sets.
const TITLE: &str = "one-publisher";

/// `docs/09-m3-plan.md` D-M3-2: every transition owes exactly one sequence
/// number, and for a pane-thread fact the publisher is the pane actor.
///
/// Until this landed, seven pane event kinds were published by the pane actor
/// *and* six of them republished by `Core` from the report that followed, so a
/// title change or an exit burned two sequence numbers. That was tolerable
/// while every consumer read the bus at-least-once; the M3 reconnect-resync
/// makes sequence numbers a continuity artifact that crosses process
/// generations, and a duplicate halves the replay window exactly when a
/// reconnect storm is spending it.
///
/// Driven through a real pane and a real `Core` on the real actor loop,
/// because the duplicate only existed where the two met — either half alone
/// publishes once and looks correct.
#[tokio::test]
async fn a_pane_transition_publishes_exactly_one_event() {
    let harness = Harness::start("one-publisher", 64);
    let root = harness.seed_workspace().await;
    harness.settle().await;

    let mut events = harness.ctx.bus.subscribe();
    let pane = harness.split_running(root, FOUR_TRANSITIONS).await;
    // Read the bus continuously from here on, in two phases with no pause
    // between them: the title and the commits arrive while the pane is alive,
    // and the exit only once it is closed. A script that exited on its own
    // reached EOF before the parser's first frame boundary and committed no
    // scrollback at all.
    let mut seen = collect_until(&mut events, |seen| {
        commits(seen, pane)
            .iter()
            .any(|range| range.last.get() >= 40)
    })
    .await;
    harness.close(pane).await;
    seen.extend(collect_until_quiet_after_exit(&mut events, pane).await);

    let titles = seen
        .iter()
        .filter(|event| match event {
            Event::PaneTitle { pane: p, title } => *p == pane && title == TITLE,
            _ => false,
        })
        .count();
    assert_eq!(
        titles, 1,
        "one title change, one announcement; the bus carried {titles} of them:\n{seen:#?}"
    );

    let exits = seen
        .iter()
        .filter(|event| matches!(event, Event::PaneExited { pane: p, .. } if *p == pane))
        .count();
    assert_eq!(
        exits, 1,
        "a child exits once; the bus carried {exits} announcements of it:\n{seen:#?}"
    );

    // Commits are the kind that comes in a stream rather than once, so the
    // claim is about *distinctness*: the pane commits each range once, and a
    // republisher shows up as the same range announced twice.
    let committed = commits(&seen, pane);
    assert!(
        !committed.is_empty(),
        "80 lines through a 24-row grid must commit scrollback:\n{seen:#?}"
    );
    let mut distinct = committed.clone();
    distinct.sort_by_key(|range| (range.first, range.last));
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        committed.len(),
        "a committed range was announced more than once: {committed:?}"
    );

    harness.ctx.cancel.cancel();
    let _ = harness.task.await;
}

/// The other half of the rule, stated where it is exact: a report is a mailbox
/// message, and folding one publishes nothing.
///
/// Damage is why the rule is per *kind* rather than "`Core` publishes
/// everything": damage reports reach `Core` by `try_send` and are droppable
/// under saturation, so a `Core` that owned the publish could starve
/// `pane.wait_output` on a busy session. It is also why damage cannot be
/// counted the way a commit can — the drops are deliberate — so every kind is
/// driven straight into the mailbox here, where the count is exactly zero.
///
/// The reports are addressed to a pane id no actor backs, which is what makes
/// the count *exactly* zero rather than nearly: `Core` folds a report by pane
/// id and never asks whether the pane is live, so any event naming this id can
/// only have come from the fold. Addressing them to a running pane instead
/// races that pane's own first frame, which is a legitimate `PaneDamage` from
/// the legitimate publisher.
///
/// The folds are asserted in the same breath, against a pane that *is* in the
/// layout so `session.state` answers for it — deleting the folds along with
/// the publishes would be the easy way to make this test pass and would break
/// that answer.
#[tokio::test]
async fn a_pane_report_folds_without_publishing() {
    let harness = Harness::start("reports-are-not-events", 64);
    let root = harness.seed_workspace().await;
    let folded = harness.split_silent(root).await;
    let unbacked = PaneId::new_v4();
    harness.settle().await;

    let mut events = harness.ctx.bus.subscribe();
    for pane in [unbacked, folded] {
        for report in reports() {
            harness
                .tx
                .send(CoreCommand::PaneReport { pane, report })
                .await
                .expect("core mailbox is open");
        }
    }

    // The window moved: commit put the head one past row 9, eviction raised
    // the floor to 3, invalidation pulled the head back to row 7.
    let (head, floor) = harness.history_window(folded).await;
    assert_eq!(
        (head, floor),
        (RowId::from_raw(7), RowId::from_raw(3)),
        "the folds beside the deleted publishes have to stay"
    );

    let stray: Vec<Event> = drain_quiet(&mut events)
        .await
        .into_iter()
        .filter(|event| names(event, unbacked))
        .collect();
    assert!(
        stray.is_empty(),
        "folding a pane report published {} event(s) about a pane no actor \
         backs; reports are mailbox messages and the pane actor already \
         announced every one of these:\n{stray:#?}",
        stray.len()
    );

    harness.ctx.cancel.cancel();
    let _ = harness.task.await;
}

/// One of every report a `PaneHost` can send, with values chosen so the folds
/// they feed are distinguishable from each other.
fn reports() -> Vec<PaneReport> {
    vec![
        PaneReport::Damage {
            generation: GridGeneration::FIRST,
        },
        PaneReport::Committed {
            range: RowRange::new(RowId::from_raw(0), RowId::from_raw(9)),
            hashes: Vec::new(),
        },
        PaneReport::Evicted {
            oldest_row: RowId::from_raw(3),
        },
        PaneReport::Invalidated {
            from_row: RowId::from_raw(7),
            cause: InvalidationCause::Clear,
        },
        PaneReport::Bell,
        PaneReport::Exited { status: Some(0) },
    ]
}

/// Whether `event` is one of the seven pane kinds and is about `pane`.
fn names(event: &Event, pane: PaneId) -> bool {
    match event {
        Event::PaneDamage { pane: p, .. }
        | Event::PaneTitle { pane: p, .. }
        | Event::PaneResized { pane: p, .. }
        | Event::HistoryCommitted { pane: p, .. }
        | Event::HistoryInvalidated { pane: p, .. }
        | Event::HistoryEvicted { pane: p, .. }
        | Event::PaneExited { pane: p, .. } => *p == pane,
        _ => false,
    }
}

/// The ranges `pane` announced as committed, in the order they arrived.
fn commits(seen: &[Event], pane: PaneId) -> Vec<RowRange> {
    seen.iter()
        .filter_map(|event| match event {
            Event::HistoryCommitted { pane: p, range } if *p == pane => Some(*range),
            _ => None,
        })
        .collect()
}

/// Read the bus into a vector until `enough` is satisfied by what has arrived.
async fn collect_until(
    events: &mut Subscription,
    mut enough: impl FnMut(&[Event]) -> bool,
) -> Vec<Event> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    while !enough(&seen) {
        let delivery = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("the bus went quiet too early:\n{seen:#?}"))
            .expect("the bus is open");
        match delivery {
            Delivery::Event(envelope) => seen.push(envelope.event),
            Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
        }
    }
    seen
}

/// Every event the bus carries until `pane` has exited and the bus has been
/// quiet for [`QUIET`].
async fn collect_until_quiet_after_exit(events: &mut Subscription, pane: PaneId) -> Vec<Event> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    let mut exited = false;
    loop {
        match tokio::time::timeout(QUIET, events.recv()).await {
            Ok(Some(Delivery::Event(envelope))) => {
                assert!(
                    Instant::now() < deadline,
                    "the bus never went quiet after the exit:\n{seen:#?}"
                );
                exited |=
                    matches!(&envelope.event, Event::PaneExited { pane: p, .. } if *p == pane);
                seen.push(envelope.event);
            }
            Ok(Some(Delivery::Gap { from, to })) => {
                panic!("the test fell behind the bus: {from}..={to}")
            }
            Ok(None) => return seen,
            Err(_) => {
                assert!(exited, "the pane never exited; the bus carried:\n{seen:#?}");
                return seen;
            }
        }
    }
}

/// Whatever the subscription holds once it has been quiet for a beat.
async fn drain_quiet(events: &mut Subscription) -> Vec<Event> {
    let mut seen = Vec::new();
    while let Ok(delivery) = tokio::time::timeout(QUIET, events.recv()).await {
        match delivery {
            Some(Delivery::Event(envelope)) => seen.push(envelope.event),
            Some(Delivery::Gap { from, to }) => {
                panic!("the test fell behind the bus: {from}..={to}")
            }
            None => break,
        }
    }
    seen
}
