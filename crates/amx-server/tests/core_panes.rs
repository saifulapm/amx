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
    Bus, Ctx, Delivery, Event, GridGeneration, PaneId, Scheduled, SessionName, Subscription,
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
