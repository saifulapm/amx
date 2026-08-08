//! A real session to hand over: `Core`, `Gateway`, `Persist`, live panes.
//!
//! Assembled the way `session/serve.rs` assembles one, with the one difference
//! that makes the export path testable at all: the orchestrator's job queue
//! ends *here* rather than in a spawned task, so a test can take the frozen
//! session `Core` produces and walk §3 against a peer of its choosing.
//! Everything else — the socket, the gateway, the pty behind each pane, the
//! persistence actor — is the real thing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use amx_core::{Bus, Ctx, PaneId, Scheduled, SessionName};
use amx_proto::control::{pane, session, workspace};
use amx_server::actor::core::Core;
use amx_server::actor::gateway::{Gateway, GatewayControl, GatewayReport};
use amx_server::actor::persist::{PERSIST_MAILBOX, Persist};
use amx_server::actor::{
    CoreCommand, CoreHandle, PaneCall, PaneCommand, PersistHandle, SessionCall, WorkspaceCall,
};
use amx_server::handoff::export::{Job, Ledger};
use amx_server::runtime::{Runtime, ShutdownReport};
use amx_vt::{CellWide, Row, Snapshot};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// How long a test waits for something that must happen.
pub const PATIENCE: Duration = Duration::from_secs(15);

/// How often a poll loop looks at its condition.
pub const TICK: Duration = Duration::from_millis(5);

/// A directory under `$TMPDIR`, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Short on purpose: this prefixes a unix socket path, and `sun_path` is
        // about a hundred bytes on both platforms.
        let brief: String = tag.chars().take(4).collect();
        let path = std::env::temp_dir().join(format!("x{}-{n}-{brief}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `Ctx` whose every path is under `root`.
pub fn ctx_under(root: &Path) -> Ctx {
    let runtime_dir = root.join("run");
    std::fs::create_dir_all(&runtime_dir).expect("create the runtime dir");
    Ctx {
        session: SessionName::new("export").expect("valid session name"),
        socket: runtime_dir.join("sock"),
        runtime_dir,
        state_dir: root.join("state"),
        config_path: root.join("config/amx/config.toml"),
        bus: Arc::new(Bus::new(256)),
        cancel: CancellationToken::new(),
    }
}

/// One session, assembled and serving.
pub struct Rig {
    pub ctx: Ctx,
    pub core: CoreHandle,
    pub persist: PersistHandle,
    pub ledger: Arc<Ledger>,
    pub control: GatewayControl,
    /// Where `Core` puts a frozen session. The test is the orchestrator.
    pub jobs: mpsc::Receiver<Job>,
    runtime: Runtime,
    gateway: oneshot::Receiver<GatewayReport>,
    dir: TempDir,
}

impl Rig {
    /// Assemble a session with a live pane in it.
    pub async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        let mut runtime = Runtime::new(ctx.clone());

        let (core_tx, core_rx) = mpsc::channel(64);
        let gateway =
            Gateway::bind(ctx.clone(), CoreHandle::new(core_tx.clone())).expect("bind the socket");
        let control = gateway.control();

        let mut core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
        // The cwd probes a capture would otherwise wait on ask panes that are
        // about to be frozen; nothing here asserts about a refreshed cwd.
        core.set_capture_budget(Duration::ZERO);
        let (jobs_tx, jobs) = mpsc::channel(1);
        core.set_handoff(jobs_tx);
        let ledger = core.handoff_ledger();

        let (persist_tx, persist_rx) = mpsc::channel(PERSIST_MAILBOX);
        let persist = PersistHandle::new(persist_tx);
        core.set_persist(persist.clone());
        let events = ctx.bus.subscribe();
        let config = amx_server::config_rt::ConfigRuntime::load(&ctx);
        let persist_actor = Persist::new(
            ctx.clone(),
            CoreHandle::new(core_tx.clone()),
            config.subscribe(),
        );
        runtime.spawn("persist", async move {
            let _persist = persist_actor.run(persist_rx, events).await;
        });

        runtime.spawn("core", async move {
            let _core = core.run(core_rx, |_: &Scheduled| {}).await;
        });
        let (report_tx, gateway_rx) = oneshot::channel();
        runtime.spawn("gateway", async move {
            let _ = report_tx.send(gateway.run().await);
        });

        let rig = Self {
            ctx,
            core: CoreHandle::new(core_tx),
            persist,
            ledger,
            control,
            jobs,
            runtime,
            gateway: gateway_rx,
            dir,
        };
        rig.seed_workspace().await;
        rig
    }

    /// Where the session's snapshot would be written.
    pub fn snapshot(&self) -> PathBuf {
        self.ctx.state_dir.join(amx_server::persist::SNAPSHOT_NAME)
    }

    /// Where the temp root is, for a test that wants a fixture beside it.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Create the session's first workspace and return its root pane.
    async fn seed_workspace(&self) {
        let (reply, answer) = oneshot::channel();
        self.core
            .send(CoreCommand::Workspace(WorkspaceCall::Create {
                params: workspace::CreateParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("the workspace was created");
    }

    /// Every pane the session has, in `session.state` order.
    pub async fn panes(&self) -> Vec<PaneId> {
        let (reply, answer) = oneshot::channel();
        self.core
            .send(CoreCommand::Session(SessionCall::State {
                params: session::StateParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        let state = answer.await.expect("core answered").expect("state answers");
        state.panes.iter().map(|pane| pane.pane).collect()
    }

    /// Split a second pane off `from`, running `cat` so it echoes.
    pub async fn split_echoing(&self, from: PaneId) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.core
            .send(CoreCommand::Pane(PaneCall::Split {
                params: pane::SplitParams {
                    pane: from,
                    direction: pane::SplitDirection::Vertical,
                    command: Some(vec!["/bin/sh".into(), "-c".into(), "exec cat".into()]),
                    cwd: Some(PathBuf::from("/")),
                },
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("the split succeeded")
            .pane
    }

    /// Ask `Core` to hand this session to `binary`, with the pre-flight it
    /// would have run on the connection task.
    pub async fn handoff(&self, binary: &Path, timeout_ms: Option<u64>) -> session::HandoffReply {
        let preflight = amx_server::handoff::export::pre_flight(binary)
            .await
            .map_err(|err| err.to_string());
        let (reply, answer) = oneshot::channel();
        self.core
            .send(CoreCommand::Session(SessionCall::Handoff {
                params: session::HandoffParams {
                    binary: binary.to_path_buf(),
                    timeout_ms,
                },
                preflight,
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("session.handoff answers rather than failing")
    }

    /// What `session.report` says about the last handoff.
    pub async fn reported_handoff(&self) -> Option<session::HandoffAttempt> {
        let (reply, answer) = oneshot::channel();
        self.core
            .send(CoreCommand::Session(SessionCall::Report {
                params: session::ReportParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("report answers")
            .handoff
    }

    /// Write bytes into a pane's terminal and wait for them to come back.
    ///
    /// The proof that a pane is *live* rather than merely present: a quiesced
    /// pane's handle refuses input outright, and a quiesced pane that accepted
    /// it would still show nothing, because nothing is reading the terminal.
    pub async fn echoes(&self, pane: PaneId, probe: &str) -> bool {
        let handle = {
            let (reply, answer) = oneshot::channel();
            self.core
                .send(CoreCommand::Stream(amx_server::actor::StreamCall::Wiring {
                    pane,
                    reply,
                }))
                .await
                .expect("core mailbox is open");
            match answer.await.expect("core answered") {
                Ok(wiring) => wiring.handle,
                Err(_) => return false,
            }
        };
        if handle
            .send(PaneCommand::Write(bytes::Bytes::from(format!("{probe}\n"))))
            .await
            .is_err()
        {
            return false;
        }
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let (reply, answer) = oneshot::channel();
            if handle.send(PaneCommand::TakeSnapshot(reply)).await.is_err() {
                return false;
            }
            let Ok(snapshot) = answer.await else {
                return false;
            };
            if screen(&snapshot).contains(probe) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(TICK).await;
        }
    }

    /// Cancel the session and join everything.
    ///
    /// Every handle this rig keeps for the tests to drive is a live sender, and
    /// `Persist`'s drain runs until its mailbox *closes* — so they all go before
    /// the join. `session/serve.rs` needs no such step because `Core` owns the
    /// only copy of each.
    pub async fn stop(self) -> Stopped {
        let Self {
            ctx,
            core,
            persist,
            ledger,
            control,
            jobs,
            runtime,
            gateway,
            dir,
        } = self;
        ctx.cancel.cancel();
        drop((core, persist, ledger, control, jobs));
        let shutdown = runtime.shutdown().await;
        let _ = gateway.await;
        Stopped {
            shutdown,
            _dir: dir,
        }
    }
}

/// A stopped session, holding its temp directory open.
///
/// Returned rather than dropped inside [`Rig::stop`] so a test can read what
/// the shutdown wrote — or did not write — before the tree goes away.
pub struct Stopped {
    pub shutdown: ShutdownReport,
    /// Held so the session's files outlive the shutdown that wrote them, or
    /// did not.
    _dir: TempDir,
}

/// One row of a snapshot as text, trailing blanks trimmed.
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
fn screen(snapshot: &Snapshot) -> String {
    snapshot
        .grid()
        .iter()
        .map(line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wait for `cond`, or fail saying what never happened.
pub async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(TICK).await;
    }
}

/// A staged binary that answers `_handoff-caps` with `document`.
///
/// The pre-flight is one exec of a path, so a shell script is a perfectly
/// honest successor for the half of the protocol that runs before a pane is
/// touched — which is the only half a wrong binary ever reaches.
///
/// The exec is retried past `ETXTBSY`, which is a hazard of writing a program
/// inside a process that also forks: a pane spawned by a *concurrent* test can
/// inherit this write descriptor for the few microseconds between its `fork`
/// and its `exec`, and the kernel refuses to execute a file anybody holds open
/// for writing. Nothing about amx is being worked around here — the same
/// failure has been seen in `crates/amx/tests/update.rs`, which plants a copy of
/// a real binary the same way.
pub fn fixture_binary(root: &Path, name: &str, document: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = root.join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\nif [ \"$1\" = _handoff-caps ]; then\n{document}\nfi\n"),
    )
    .expect("write the fixture");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fixture executable");

    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        match std::process::Command::new(&path)
            .arg("_handoff-caps")
            .output()
        {
            Ok(_) => return path,
            Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{} stayed busy for {PATIENCE:?}",
                    path.display(),
                );
                std::thread::sleep(TICK);
            }
            Err(err) => panic!("the fixture at {} will not run: {err}", path.display()),
        }
    }
}
