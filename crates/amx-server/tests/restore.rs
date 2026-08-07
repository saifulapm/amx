//! Restore and the loss report (U06).
//!
//! Restore puts yesterday's session back at startup, before the gateway
//! accepts anyone (D-M1-9). Everything asserted here is asserted against real
//! state and real processes — a restored pane is a pty with a shell on the far
//! end, and a pruned one is a spawn that genuinely failed. What capture writes
//! is `tests/capture.rs`.
//!
//! The loss report is the point of the second half. 04 §6: panes and
//! workspaces that fail to respawn "produce a restore report shown in the
//! status line and queryable via `amx session report` — never log-only". So
//! every failure below is checked for its *report entry*, not for a log line
//! and not merely for the absence of a crash: a restore that quietly dropped a
//! workspace would pass a test that only counted survivors.
//!
//! Directories are passed, never read from the environment: `RestoreOptions`
//! carries the home a vanished cwd degrades into, which is 04 §2's "no env-var
//! globals, no test mutexes" applied to the one path restore needs. That is
//! also what makes the spawn-failure cases deterministic — a home that does not
//! exist is a `chdir` that fails, on every platform, without touching a
//! process-wide variable.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::{Path, PathBuf};

use amx_core::{
    Ctx, Direction, Event, Layout, PaneId, RowId, RowRange, Scheduled, ShortNumber, WorkspaceId,
};
use amx_proto::control::session::{
    self, RestoreEntity, RestoreLoss, RestoreSeverity, StateParams, StateReply,
};
use amx_proto::stream::history::put_row;
use amx_server::actor::core::{Core, RestoreOptions};
use amx_server::actor::{
    Capture, CoreCommand, CoreHandle, PaneWiring, Reply, SessionCall, StreamCall,
};
use amx_server::dispatch::Router;
use amx_server::persist::io::SyncAll;
use amx_server::persist::{
    PaneSnapshot, SidecarHeader, Snapshot, VERSION, WorkspaceSnapshot, sidecar, snapshot,
};
use amx_vt::{CellWide, Row, Snapshot as VtSnapshot};
use serde_json::json;
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

    /// The options a session whose home is also gone restores under: every
    /// spawn that falls back to it fails.
    fn opts_without_home(&self) -> RestoreOptions {
        RestoreOptions {
            home: self.dir.path().join("home-that-was-deleted"),
        }
    }

    /// A directory under the fixture's tree, created.
    fn dir_named(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).expect("create the directory");
        path
    }

    /// A path under the fixture's tree that does not exist.
    fn missing(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
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
    /// Send one command and await its reply, like the dispatch layer does.
    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> CoreCommand) -> T {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(make(reply))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("the call succeeded")
    }

    async fn state(&self) -> StateReply {
        self.call(|reply| {
            CoreCommand::Session(SessionCall::State {
                params: StateParams {},
                reply,
            })
        })
        .await
    }

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

    async fn wiring_of(&self, pane: PaneId) -> PaneWiring {
        self.call(|reply| CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
            .await
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
fn entries(
    report: &session::RestoreReport,
    severity: RestoreSeverity,
    entity: RestoreEntity,
) -> Vec<&RestoreLoss> {
    report
        .entries
        .iter()
        .filter(|entry| entry.severity == severity && entry.entity == entity)
        .collect()
}

/// Read the report `core` is holding, over its own mailbox.
async fn report_of(running: &Running) -> session::RestoreReport {
    running
        .call(|reply| {
            CoreCommand::Session(SessionCall::Report {
                params: session::ReportParams {},
                reply,
            })
        })
        .await
        .report
}

/// One row of a grid snapshot as text.
fn line(row: &Row) -> String {
    let mut out = String::new();
    for cell in row.cells() {
        if cell.wide == CellWide::SpacerTail {
            continue;
        }
        match std::str::from_utf8(row.text(cell)) {
            Ok("") | Err(_) => out.push(' '),
            Ok(text) => out.push_str(text),
        }
    }
    out.trim_end().to_owned()
}

/// The whole visible grid as text.
fn screen(snapshot: &VtSnapshot) -> String {
    snapshot
        .grid()
        .iter()
        .map(line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Poll `cond` until it holds, failing the test if it never does.
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

#[tokio::test]
async fn restore_rebuilds_workspaces_panes_and_shorts_with_the_same_uuids() {
    let mut fx = Fixture::new("ids");
    let (build, notes) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let (a, b, c) = (PaneId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let saved = snapshot_of(
        vec![
            workspace_row(build, 4, Some("build"), row_of(&[a, b]), Some(b)),
            workspace_row(notes, 7, None, Layout::with_root(c), Some(c)),
        ],
        vec![
            pane_row(a, 11, Some("editor"), Some(work.clone())),
            pane_row(b, 12, None, Some(work.clone())),
            pane_row(c, 13, Some("notes"), Some(work)),
        ],
    );

    let opts = fx.opts();
    let summary = fx.core.restore(saved, &opts);
    assert_eq!(summary.restored, 5, "two workspaces and three panes");
    assert!(summary.is_clean(), "nothing here should have been lost");

    let running = fx.start();
    let state = running.state().await;
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|ws| ws.workspace)
            .collect::<Vec<_>>(),
        vec![build, notes],
    );
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|ws| ws.short.get())
            .collect::<Vec<_>>(),
        vec![4, 7],
        "short numbers are stable across restarts (04 §6)",
    );
    assert_eq!(state.workspaces[0].label.as_deref(), Some("build"));
    assert_eq!(state.workspaces[0].focus, Some(b));
    assert_eq!(state.focused_workspace, Some(build));
    assert_eq!(
        state
            .panes
            .iter()
            .map(|pane| (pane.pane, pane.short.get()))
            .collect::<Vec<_>>(),
        vec![(a, 11), (b, 12), (c, 13)],
    );
    assert_eq!(state.panes[0].label.as_deref(), Some("editor"));
    // Every restored pane has a live process behind it, not just a rectangle.
    for pane in [a, b, c] {
        let _ = running.wiring_of(pane).await;
    }
    assert!(state.restore.is_some_and(|restore| restore.is_clean()));

    running.into_core().await;
}

#[tokio::test]
async fn a_new_workspace_after_a_restore_takes_the_next_free_short() {
    // The shorts counter has to move past what came back, or the first
    // workspace created after a restart collides with a restored one.
    let mut fx = Fixture::new("next");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                4,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 9, None, Some(work))],
        ),
        &opts,
    );

    let running = fx.start();
    let created: amx_proto::control::workspace::CreateReply = running
        .call(|reply| {
            CoreCommand::Workspace(amx_server::actor::WorkspaceCall::Create {
                params: amx_proto::control::workspace::CreateParams {
                    label: None,
                    focus: false,
                },
                reply,
            })
        })
        .await;
    assert_eq!(created.short.get(), 5, "the next free workspace number");
    let state = running.state().await;
    let shorts: Vec<u32> = state.panes.iter().map(|pane| pane.short.get()).collect();
    assert_eq!(shorts, vec![9, 10], "and the next free pane number");

    running.into_core().await;
}

#[tokio::test]
async fn restored_session_suppresses_the_first_attach_seed() {
    let mut fx = Fixture::new("seed");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(work))],
        ),
        &opts,
    );

    let running = fx.start();
    // Two attaches, the way two clients would: the seed is already a no-op for
    // a session that has workspaces, and a restored session is one.
    for _ in 0..2 {
        running
            .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
            .await;
    }
    let state = running.state().await;
    assert_eq!(state.workspaces.len(), 1, "restore must not be re-seeded");
    assert_eq!(state.workspaces[0].workspace, ws);
    assert_eq!(state.panes.len(), 1);
    assert_eq!(state.panes[0].pane, pane);

    running.into_core().await;
}

#[tokio::test]
async fn empty_or_missing_snapshot_falls_through_to_the_normal_seed() {
    // No file at all.
    let mut fx = Fixture::new("fresh");
    let opts = fx.opts();
    assert!(
        fx.core.restore_from_disk(&opts).is_none(),
        "a session that was never saved has nothing to restore",
    );

    // And a file describing a session with nothing in it, which is what a
    // server that was stopped before its first workspace leaves behind.
    snapshot::save(&fx.ctx.state_dir, &Snapshot::empty(), &SyncAll).expect("write the snapshot");
    assert!(fx.core.restore_from_disk(&opts).is_none());

    let running = fx.start();
    running
        .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
        .await;
    let state = running.state().await;
    assert_eq!(state.workspaces.len(), 1, "the ordinary first-attach seed");
    assert!(
        state.restore.is_none(),
        "a server that restored nothing must not claim it restored something",
    );

    running.into_core().await;
}

// ----------------------------------------------------- prune-and-report table

#[tokio::test]
async fn missing_cwd_respawns_in_home_and_reports_degraded() {
    let mut fx = Fixture::new("degr");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let gone = fx.missing("a-directory-that-was-deleted");
    let home = fx.home();

    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, Some("editor"), Some(gone.clone()))],
        ),
        &opts,
    );

    assert_eq!(summary.restored, 2, "the workspace and its pane came back");
    assert_eq!((summary.lost, summary.degraded), (0, 1));

    let running = fx.start();
    let report = report_of(&running).await;
    let degraded = entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane);
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].pane, Some(pane));
    assert_eq!(degraded[0].label.as_deref(), Some("editor"));
    assert_eq!(
        degraded[0].path.as_deref(),
        Some(gone.as_path()),
        "the entry names the directory that vanished, not the one it settled for",
    );

    // The pane is alive, in the home directory it degraded into.
    let capture = running.capture().await;
    assert_eq!(
        capture.snapshot.panes[0].cwd.as_deref(),
        Some(home.as_path())
    );
    let _ = running.wiring_of(pane).await;

    running.into_core().await;
}

#[tokio::test]
async fn spawn_failure_prunes_the_pane_and_reports_lost() {
    let mut fx = Fixture::new("lost");
    let (ws, good, doomed) = (WorkspaceId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let gone = fx.missing("a-directory-that-was-deleted");

    // The home it would fall back to does not exist either, so the respawn
    // fails in `chdir` — a real spawn failure, not a simulated one.
    let opts = fx.opts_without_home();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                row_of(&[good, doomed]),
                Some(doomed),
            )],
            vec![
                pane_row(good, 1, None, Some(work)),
                pane_row(doomed, 2, Some("agent"), Some(gone.clone())),
            ],
        ),
        &opts,
    );

    assert_eq!(summary.restored, 2, "the workspace and its surviving pane");
    assert_eq!((summary.lost, summary.degraded), (1, 0));

    let running = fx.start();
    let report = report_of(&running).await;
    let lost = entries(&report, RestoreSeverity::Lost, RestoreEntity::Pane);
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].pane, Some(doomed));
    assert_eq!(lost[0].label.as_deref(), Some("agent"));
    assert!(
        !lost[0].reason.is_empty(),
        "a loss the user reads has to say why",
    );
    assert!(
        entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane).is_empty(),
        "a pane that was pruned was not also respawned somewhere else",
    );

    // The layout collapsed onto the survivor, exactly as a close would leave it.
    let state = running.state().await;
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(
        state.panes.iter().map(|pane| pane.pane).collect::<Vec<_>>(),
        vec![good]
    );
    assert_eq!(
        state.workspaces[0].focus,
        Some(good),
        "focus that pointed at the pruned pane moves to what is left",
    );

    running.into_core().await;
}

#[tokio::test]
async fn workspace_losing_every_pane_is_pruned_and_reported() {
    let mut fx = Fixture::new("wsprune");
    let (kept, doomed) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let (alive, dead) = (PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let gone = fx.missing("a-directory-that-was-deleted");

    let opts = fx.opts_without_home();
    let summary = fx.core.restore(
        Snapshot {
            version: VERSION,
            // The focused workspace is the one about to be pruned: focus has to
            // land somewhere real rather than on a workspace that is gone.
            focused_workspace: Some(doomed),
            workspaces: vec![
                workspace_row(kept, 1, None, Layout::with_root(alive), Some(alive)),
                workspace_row(
                    doomed,
                    2,
                    Some("agents"),
                    Layout::with_root(dead),
                    Some(dead),
                ),
            ],
            panes: vec![
                pane_row(alive, 1, None, Some(work)),
                pane_row(dead, 2, None, Some(gone)),
            ],
        },
        &opts,
    );

    assert_eq!(summary.restored, 2, "one workspace and one pane");
    assert_eq!(
        (summary.lost, summary.degraded),
        (2, 0),
        "the pane that could not spawn, and the workspace it emptied",
    );

    let running = fx.start();
    let report = report_of(&running).await;
    let lost_ws = entries(&report, RestoreSeverity::Lost, RestoreEntity::Workspace);
    assert_eq!(lost_ws.len(), 1);
    assert_eq!(lost_ws[0].workspace, Some(doomed));
    assert_eq!(lost_ws[0].label.as_deref(), Some("agents"));

    let state = running.state().await;
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|ws| ws.workspace)
            .collect::<Vec<_>>(),
        vec![kept],
    );
    assert_eq!(
        state.focused_workspace,
        Some(kept),
        "a session left with workspaces and no focus would render as nothing",
    );

    running.into_core().await;
}

#[tokio::test]
async fn a_snapshot_from_a_newer_amx_is_a_whole_session_loss() {
    let mut fx = Fixture::new("newer");
    let bytes = serde_json::to_vec(&json!({
        "version": VERSION + 1,
        "workspaces": [],
        "panes": [],
    }))
    .expect("encode the future snapshot");
    std::fs::write(snapshot::path(&fx.ctx.state_dir), bytes).expect("write the snapshot");

    let opts = fx.opts();
    let summary = fx
        .core
        .restore_from_disk(&opts)
        .expect("a snapshot that cannot be read is a loss, not an absence");
    assert_eq!((summary.restored, summary.lost), (0, 1));

    let running = fx.start();
    let report = report_of(&running).await;
    let lost = entries(&report, RestoreSeverity::Lost, RestoreEntity::Session);
    assert_eq!(lost.len(), 1);
    assert!(
        lost[0].reason.contains("newer amx"),
        "the user is told which way the version window failed: {}",
        lost[0].reason,
    );

    // The session still starts, and still seeds: a refused start would be a
    // worse answer than a fresh session with a report.
    running
        .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
        .await;
    assert_eq!(running.state().await.workspaces.len(), 1);

    running.into_core().await;
}

// ------------------------------------------------------------------ sidecars

/// Write `rows` as `pane`'s scrollback sidecar.
fn save_sidecar(state_dir: &Path, pane: PaneId, rows: &[(&str, bool)]) {
    let mut packed = Vec::new();
    for (text, wrapped) in rows {
        put_row(text.as_bytes(), *wrapped, &mut packed);
    }
    let range = RowRange::new(
        RowId::from_raw(0),
        RowId::from_raw(rows.len().saturating_sub(1) as u64),
    );
    sidecar::save(
        state_dir,
        &SidecarHeader::new(pane, range),
        &packed,
        &SyncAll,
    )
    .expect("write the sidecar");
}

#[tokio::test]
async fn saved_scrollback_is_replayed_into_the_restored_pane() {
    let mut fx = Fixture::new("replay");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    // The second line was soft-wrapped when it was saved: replay joins it back
    // into one logical line and lets the pane's current width re-wrap it.
    save_sidecar(
        &fx.ctx.state_dir,
        pane,
        &[
            ("cargo test --all", false),
            ("running 3 ", true),
            ("tests", false),
        ],
    );

    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(work))],
        ),
        &opts,
    );
    assert!(summary.is_clean(), "a readable sidecar costs nothing");

    let running = fx.start();
    let wiring = running.wiring_of(pane).await;
    wait_until(
        "the restored scrollback is on the pane's grid",
        async || {
            let text = screen(&wiring.frames.latest());
            text.contains("cargo test --all") && text.contains("running 3 tests")
        },
    )
    .await;

    running.into_core().await;
}

#[tokio::test]
async fn an_unreadable_sidecar_degrades_the_pane_instead_of_losing_it() {
    let mut fx = Fixture::new("badside");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    std::fs::create_dir_all(sidecar::dir(&fx.ctx.state_dir)).expect("create history/");
    std::fs::write(
        sidecar::path(&fx.ctx.state_dir, pane),
        b"this is not a sidecar",
    )
    .expect("write the damaged sidecar");

    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(work))],
        ),
        &opts,
    );
    assert_eq!(
        (summary.restored, summary.lost, summary.degraded),
        (2, 0, 1),
        "a pane's history is not the pane",
    );

    let running = fx.start();
    let report = report_of(&running).await;
    let degraded = entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane);
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].pane, Some(pane));
    assert_eq!(
        degraded[0].path.as_deref(),
        Some(sidecar::path(&fx_state_dir(&running), pane).as_path()),
    );
    let _ = running.wiring_of(pane).await;

    running.into_core().await;
}

/// The state directory of a running fixture.
fn fx_state_dir(running: &Running) -> PathBuf {
    running.ctx.state_dir.clone()
}

// ----------------------------------------------------------------- the wire

#[tokio::test]
async fn session_report_returns_entries_and_session_state_carries_counts() {
    let mut fx = Fixture::new("wire");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let gone = fx.missing("a-directory-that-was-deleted");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, Some("editor"), Some(gone.clone()))],
        ),
        &opts,
    );

    let running = fx.start();
    // Through the real dispatch table, the way a client reaches it.
    let mut router = Router::new(CoreHandle::new(running.tx.clone()));

    let reply = amx_server::dispatch::handle(&mut router, "session.report", Some(json!({})))
        .await
        .expect("session.report is implemented");
    let report: session::ReportReply = serde_json::from_value(reply).expect("decode the report");
    assert_eq!(report.report.entries.len(), 1);
    let entry = &report.report.entries[0];
    assert_eq!(entry.severity, RestoreSeverity::Degraded);
    assert_eq!(entry.entity, RestoreEntity::Pane);
    assert_eq!(entry.label.as_deref(), Some("editor"));
    assert_eq!(entry.path.as_deref(), Some(gone.as_path()));

    let reply = amx_server::dispatch::handle(&mut router, "session.state", Some(json!({})))
        .await
        .expect("session.state answers");
    let state: StateReply = serde_json::from_value(reply).expect("decode the state");
    let summary = state.restore.expect("a server that restored says so");
    assert_eq!(
        (summary.restored, summary.lost, summary.degraded),
        (2, 0, 1)
    );
    assert!(!summary.is_clean(), "this is what the ⚠ indicator reads");

    running.into_core().await;
}

#[tokio::test]
async fn restore_publishes_one_session_restored_with_the_counts() {
    let mut fx = Fixture::new("bus");
    let mut events = fx.ctx.bus.subscribe();
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let gone = fx.missing("a-directory-that-was-deleted");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(gone))],
        ),
        &opts,
    );

    // `SessionRestored` is the last thing restore publishes, so draining up to
    // it also proves the per-entity events came first — a client that replays
    // the bus sees the workspace and the pane appear before it is told what the
    // restore cost.
    let mut created = 0;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let summary = loop {
        let delivery = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an event before the deadline")
            .expect("the bus is open");
        let amx_core::Delivery::Event(envelope) = delivery else {
            panic!("the test fell behind the bus");
        };
        match envelope.event {
            Event::WorkspaceCreated { .. } | Event::PaneCreated { .. } => created += 1,
            event @ Event::SessionRestored { .. } => break event,
            _ => {}
        }
    };
    assert_eq!(
        created, 2,
        "restore publishes the ordinary per-entity events too",
    );
    assert_eq!(
        summary,
        Event::SessionRestored {
            workspaces: 1,
            panes: 1,
            lost: 0,
            degraded: 1,
        },
    );

    fx.drain().await;
}
