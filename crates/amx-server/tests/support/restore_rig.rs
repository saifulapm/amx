//! The persistence rig: a `Core` over a temp tree, restored by hand.
//!
//! Restore runs on an *owned* `Core` — the serve path applies the snapshot
//! between the gateway bind and the accept loop, so there is no actor to talk
//! to yet (D-M1-9) — which is why this hands out the `Core` itself and starts
//! the loop afterwards, rather than handing out a mailbox like
//! [`Server`](super::Server) does.
//!
//! Directories are passed, never read from the environment: `RestoreOptions`
//! carries the home a vanished cwd degrades into, which is 04 §2's "no env-var
//! globals, no test mutexes" applied to the one path restore needs. It is also
//! what makes the spawn-failure cases deterministic — a home that does not
//! exist is a `chdir` that fails, on every platform, without touching a
//! process-wide variable.

#![allow(dead_code, reason = "each test binary uses a subset of the rig")]

use std::path::{Path, PathBuf};

use amx_core::{Ctx, Direction, Layout, PaneId, Scheduled, ShortNumber, WorkspaceId};
use amx_core::{RowId, RowRange};
use amx_proto::control::session::{
    self, RestoreEntity, RestoreLoss, RestoreSeverity, StateParams, StateReply,
};
use amx_proto::stream::history::put_row;
use amx_server::actor::core::{Core, RestoreOptions};
use amx_server::actor::{
    Capture, CoreCommand, CoreHandle, PaneWiring, Reply, SessionCall, StreamCall,
};
use amx_server::persist::io::SyncAll;
use amx_server::persist::{
    PaneSnapshot, SidecarHeader, Snapshot, VERSION, WorkspaceSnapshot, sidecar,
};
use amx_vt::{CellWide, Row, Snapshot as VtSnapshot};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{PATIENCE, TICK, TempDir, ctx_under};

/// A `Core` over a fresh temp tree, before its actor loop starts.
///
/// Restore runs on an owned `Core` — the serve path applies the snapshot
/// between the bind and the accept loop, so there is no actor to talk to yet —
/// which is why this hands out the `Core` itself rather than a mailbox.
pub struct Fixture {
    pub dir: TempDir,
    pub ctx: Ctx,
    pub tx: mpsc::Sender<CoreCommand>,
    pub rx: mpsc::Receiver<CoreCommand>,
    pub core: Core,
}

impl Fixture {
    pub fn new(tag: &str) -> Self {
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
    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    pub fn opts(&self) -> RestoreOptions {
        RestoreOptions { home: self.home() }
    }

    /// The options a session whose home is also gone restores under: every
    /// spawn that falls back to it fails.
    pub fn opts_without_home(&self) -> RestoreOptions {
        RestoreOptions {
            home: self.dir.path().join("home-that-was-deleted"),
        }
    }

    /// A directory under the fixture's tree, created.
    pub fn dir_named(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).expect("create the directory");
        path
    }

    /// A path under the fixture's tree that does not exist.
    pub fn missing(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Start the actor loop over this `Core`.
    pub fn start(self) -> Running {
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
    pub async fn drain(self) -> Core {
        self.ctx.cancel.cancel();
        self.core.run(self.rx, |_: &Scheduled| {}).await
    }
}

/// A `Core` serving its mailbox.
pub struct Running {
    pub ctx: Ctx,
    pub tx: mpsc::Sender<CoreCommand>,
    pub task: JoinHandle<Core>,
    _dir: TempDir,
}

impl Running {
    /// Send one command and await its reply, like the dispatch layer does.
    pub async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> CoreCommand) -> T {
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

    pub async fn state(&self) -> StateReply {
        self.call(|reply| {
            CoreCommand::Session(SessionCall::State {
                params: StateParams {},
                reply,
            })
        })
        .await
    }

    /// A capture over the live path — the one that refreshes cwds.
    pub async fn capture(&self) -> Capture {
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

    pub async fn wiring_of(&self, pane: PaneId) -> PaneWiring {
        self.call(|reply| CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
            .await
    }

    /// Stop the loop and hand the `Core` back, panes joined.
    pub async fn into_core(self) -> Core {
        self.ctx.cancel.cancel();
        self.task.await.expect("core task did not panic")
    }
}

// ------------------------------------------------------------------ builders

pub fn pane_row(id: PaneId, short: u32, label: Option<&str>, cwd: Option<PathBuf>) -> PaneSnapshot {
    PaneSnapshot {
        id,
        short: ShortNumber::new(short),
        label: label.map(str::to_owned),
        cwd,
        argv: None,
        agent: None,
    }
}

pub fn workspace_row(
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
pub fn row_of(panes: &[PaneId]) -> Layout {
    let mut layout = Layout::with_root(panes[0]);
    for pair in panes.windows(2) {
        layout
            .split(pair[0], Direction::Right, pair[1], 0.5)
            .expect("split the restored layout");
    }
    layout
}

pub fn snapshot_of(workspaces: Vec<WorkspaceSnapshot>, panes: Vec<PaneSnapshot>) -> Snapshot {
    Snapshot {
        version: VERSION,
        focused_workspace: workspaces.first().map(|ws| ws.id),
        workspaces,
        panes,
    }
}

/// Every entry of `severity` about `entity`.
pub fn entries(
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
pub async fn report_of(running: &Running) -> session::RestoreReport {
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
pub fn line(row: &Row) -> String {
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
pub fn screen(snapshot: &VtSnapshot) -> String {
    snapshot
        .grid()
        .iter()
        .map(line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Poll `cond` until it holds, failing the test if it never does.
pub async fn wait_until(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !cond().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(TICK).await;
    }
}

/// Write `rows` as `pane`'s scrollback sidecar.
pub fn save_sidecar(state_dir: &Path, pane: PaneId, rows: &[(&str, bool)]) {
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
