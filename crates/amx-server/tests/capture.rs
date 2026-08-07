//! Capture: what the `Persist` actor is handed, and the last one of all (U06).
//!
//! The `Core` side of the capture split (`docs/07-m1-plan.md` §2). Capture
//! happens on `Core` because `Core` owns the state; what these tests pin is
//! that it reports the session *as it is* — labels, layout, focus, short
//! numbers and directories — and that neither of its two hazards can bite: a
//! pane that will not answer its cwd probe must not stall the actor, and the
//! final capture pushed on the way down must not be able to wedge the drain
//! (R-M1-2).
//!
//! Restore, the other direction, is `tests/restore.rs`; it is also what these
//! tests use to put a session in place, since it is the only way to build one
//! with known labels and short numbers before `pane.rename` lands (U07).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;
use std::time::Duration;

use amx_core::{Ctx, Direction, Layout, PaneId, Scheduled, ShortNumber, WorkspaceId};
use amx_server::actor::core::{Core, RestoreOptions};
use amx_server::actor::{
    Capture, CoreCommand, CoreHandle, PersistCommand, PersistHandle, SessionCall,
};
use amx_server::persist::{PaneSnapshot, Snapshot, VERSION, WorkspaceSnapshot};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

mod support;

use support::{PATIENCE, TICK, TempDir, ctx_under};

/// A `Core` over a fresh temp tree, before its actor loop starts.
///
/// Restore runs on an owned `Core` — the serve path applies the snapshot
/// between the bind and the accept loop, so there is no actor to talk to yet —
/// which is why this hands out the `Core` itself rather than a mailbox.
struct Fixture {
    dir: TempDir,
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    rx: mpsc::Receiver<CoreCommand>,
    core: Core,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        std::fs::create_dir_all(&ctx.state_dir).expect("create the state dir");
        std::fs::create_dir_all(dir.path().join("home")).expect("create the home dir");
        let (tx, rx) = mpsc::channel(64);
        let core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));
        Self {
            dir,
            ctx,
            tx,
            rx,
            core,
        }
    }

    /// The directory a vanished cwd degrades into.
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn opts(&self) -> RestoreOptions {
        RestoreOptions { home: self.home() }
    }

    /// A directory under the fixture's tree, created.
    fn dir_named(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).expect("create the directory");
        path
    }

    /// Start the actor loop over this `Core`.
    fn start(self) -> Running {
        let Self {
            dir,
            ctx,
            tx,
            rx,
            core,
        } = self;
        let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));
        Running {
            _dir: dir,
            ctx,
            tx,
            task,
        }
    }

    /// Stop without ever running: cancels first, so `run` breaks straight to
    /// its drain and joins every pane the restore spawned.
    async fn drain(self) -> Core {
        self.ctx.cancel.cancel();
        self.core.run(self.rx, |_: &Scheduled| {}).await
    }
}

/// A `Core` serving its mailbox.
struct Running {
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    task: JoinHandle<Core>,
    _dir: TempDir,
}

impl Running {
    /// A capture over the live path — the one that refreshes cwds.
    async fn capture(&self) -> Capture {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Session(SessionCall::Capture {
                sidecars: false,
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer.await.expect("core answered the capture")
    }

    /// Stop the loop and hand the `Core` back, panes joined.
    async fn into_core(self) -> Core {
        self.ctx.cancel.cancel();
        self.task.await.expect("core task did not panic")
    }
}

// ------------------------------------------------------------------ builders

fn pane_row(id: PaneId, short: u32, label: Option<&str>, cwd: Option<PathBuf>) -> PaneSnapshot {
    PaneSnapshot {
        id,
        short: ShortNumber::new(short),
        label: label.map(str::to_owned),
        cwd,
    }
}

fn workspace_row(
    id: WorkspaceId,
    short: u32,
    label: Option<&str>,
    layout: Layout,
    focus: Option<PaneId>,
) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        id,
        short: ShortNumber::new(short),
        label: label.map(str::to_owned),
        layout,
        focus,
    }
}

/// A layout of `panes`, split rightwards from the first.
fn row_of(panes: &[PaneId]) -> Layout {
    let mut layout = Layout::with_root(panes[0]);
    for pair in panes.windows(2) {
        layout
            .split(pair[0], Direction::Right, pair[1], 0.5)
            .expect("split the restored layout");
    }
    layout
}

fn snapshot_of(workspaces: Vec<WorkspaceSnapshot>, panes: Vec<PaneSnapshot>) -> Snapshot {
    Snapshot {
        version: VERSION,
        focused_workspace: workspaces.first().map(|ws| ws.id),
        workspaces,
        panes,
    }
}

/// Every entry of `severity` about `entity`.
async fn wait_until(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !cond().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(TICK).await;
    }
}

// ------------------------------------------------------------------- capture

#[tokio::test]
async fn capture_reflects_labels_layout_focus_shorts_and_cwds() {
    let mut fx = Fixture::new("cap");
    let (ws, left, right) = (WorkspaceId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let (editor, logs) = (fx.dir_named("editor"), fx.dir_named("logs"));
    let saved = snapshot_of(
        vec![workspace_row(
            ws,
            3,
            Some("build"),
            row_of(&[left, right]),
            Some(right),
        )],
        vec![
            pane_row(left, 7, Some("editor"), Some(editor)),
            pane_row(right, 9, None, Some(logs)),
        ],
    );

    let opts = fx.opts();
    fx.core.restore(saved.clone(), &opts);

    // The strongest statement available: what a capture writes is byte-for-byte
    // what the previous save wrote — labels, the layout tree with its ratios,
    // the focus, the short numbers, the cwds and the focused workspace. Any of
    // those dropped on either leg shows up here.
    assert_eq!(fx.core.capture_cheap(), saved);

    fx.drain().await;
}

#[tokio::test]
async fn capture_falls_back_to_stored_cwd_when_the_probe_stalls() {
    let mut fx = Fixture::new("stall");
    let pane = PaneId::new_v4();
    // The stored cwd is a symlink; the process that chdirs through it reports
    // the target. So the two answers are distinguishable, and which one lands
    // in the capture says which path ran.
    let real = fx.dir_named("real");
    let link = fx.dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink the pane's cwd");

    let ws = WorkspaceId::new_v4();
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(ws, 1, None, Layout::with_root(pane), None)],
            vec![pane_row(pane, 1, None, Some(link.clone()))],
        ),
        &opts,
    );
    // A budget of zero is the stall taken to its limit: the answer is out of
    // time before it could be asked for, so the capture uses what it already
    // has. Racing a real probe against a short-but-nonzero budget would be
    // pacing against the wall clock, which the rig forbids and which the
    // machine wins about as often as it loses.
    fx.core.set_capture_budget(Duration::ZERO);

    let running = fx.start();
    let capture = running.capture().await;
    assert_eq!(
        capture.snapshot.panes[0].cwd.as_deref(),
        Some(link.as_path()),
        "a probe that did not answer in time must leave the stored cwd alone",
    );

    running.into_core().await;
}

#[tokio::test]
async fn capture_refreshes_each_panes_cwd_from_its_foreground_process() {
    let mut fx = Fixture::new("probe");
    let pane = PaneId::new_v4();
    let real = fx.dir_named("real");
    let link = fx.dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink the pane's cwd");
    let resolved = std::fs::canonicalize(&real).expect("canonicalize the target");

    let ws = WorkspaceId::new_v4();
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(ws, 1, None, Layout::with_root(pane), None)],
            vec![pane_row(pane, 1, None, Some(link))],
        ),
        &opts,
    );

    let running = fx.start();
    // The shell has to reach the foreground of its pty before `tcgetpgrp` can
    // name it, so the refresh is asserted as an outcome the session converges
    // on, not as one that holds the instant the pane exists.
    wait_until("the capture reports the pane's live cwd", async || {
        running.capture().await.snapshot.panes[0].cwd.as_deref() == Some(resolved.as_path())
    })
    .await;

    running.into_core().await;
}

// ------------------------------------------------------------------ shutdown

#[tokio::test]
async fn core_shutdown_pushes_a_final_capture_without_blocking() {
    let mut fx = Fixture::new("push");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                2,
                Some("build"),
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 5, Some("editor"), Some(work.clone()))],
        ),
        &opts,
    );

    let (persist_tx, mut persist_rx) = mpsc::channel(4);
    fx.core.set_persist(PersistHandle::new(persist_tx));

    let running = fx.start();
    running.into_core().await;

    let pushed = persist_rx
        .try_recv()
        .expect("the shutdown path pushes one last capture");
    let PersistCommand::Snapshot(snapshot) = pushed else {
        panic!("the final push is a snapshot, not a flush");
    };
    // Built from stored state: the panes were about to be killed, so nothing
    // on the drain path waited on one.
    assert_eq!(snapshot.workspaces.len(), 1);
    assert_eq!(snapshot.workspaces[0].label.as_deref(), Some("build"));
    assert_eq!(snapshot.panes[0].id, pane);
    assert_eq!(snapshot.panes[0].cwd.as_deref(), Some(work.as_path()));
}

#[tokio::test]
async fn a_full_persistence_mailbox_never_wedges_the_core_drain() {
    let mut fx = Fixture::new("wedge");
    // Depth one, already full: `try_send` is what keeps this from being a
    // shutdown that never finishes (R-M1-2).
    let (persist_tx, _persist_rx) = mpsc::channel(1);
    persist_tx
        .try_send(PersistCommand::Snapshot(Box::new(Snapshot::empty())))
        .expect("fill the mailbox");
    fx.core.set_persist(PersistHandle::new(persist_tx));

    let running = fx.start();
    tokio::time::timeout(PATIENCE, running.into_core())
        .await
        .expect("the core drain finished with a full persistence mailbox");
}

// --------------------------------------------------------------- the bus
