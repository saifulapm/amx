//! A process tree the test writes down, plus the pane harness two of the
//! acceptance tests drive a real `Core` through.
//!
//! The fake tree exists because the interesting half of tier 3 is *judgement* —
//! which of a job's processes answers, and what an argv is allowed to mean —
//! and judgement is not something a real `/proc` can be asked to demonstrate on
//! command. The real reader is exercised separately, against a real pty, in the
//! one test that is about the reader.

#![allow(dead_code, reason = "each acceptance drives a subset of the harness")]

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use amx_core::platform::{PlatformError, ProcessId, ProcessTree};
use amx_core::{Bus, Ctx, PaneId, Scheduled, SessionName};
use amx_proto::control::{pane, session, workspace};
use amx_server::actor::core::Core;
use amx_server::actor::{CoreCommand, CoreHandle, PaneCall, SessionCall, WorkspaceCall};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How long an acceptance waits for something that must happen.
pub const PATIENCE: Duration = Duration::from_secs(15);

/// How long a condition loop waits between looks.
pub const TICK: Duration = Duration::from_millis(10);

/// One process, as a test declares it.
pub struct Process {
    argv: Vec<OsString>,
    exe: PathBuf,
    children: Vec<ProcessId>,
}

/// A process tree with no operating system behind it.
#[derive(Default)]
pub struct FakeTree {
    processes: HashMap<u32, Process>,
}

impl FakeTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a process: its pid, its argv, and the pids it parents.
    ///
    /// `exe` is derived from `argv[0]` unless [`with_exe`](Self::with_exe)
    /// overrides it, which mirrors the ordinary case — a program whose
    /// `argv[0]` is the path it was started from.
    #[must_use]
    pub fn with(mut self, pid: u32, argv: &[&str], children: &[u32]) -> Self {
        let exe = PathBuf::from(argv.first().copied().unwrap_or_default());
        self.processes.insert(
            pid,
            Process {
                argv: argv.iter().map(OsString::from).collect(),
                exe,
                children: children.iter().copied().map(ProcessId).collect(),
            },
        );
        self
    }

    /// Declare a process whose argv is unreadable — a zombie, or one that
    /// exited under the walk — leaving only the executable the kernel mapped.
    #[must_use]
    pub fn with_exe(mut self, pid: u32, exe: &str, children: &[u32]) -> Self {
        self.processes.insert(
            pid,
            Process {
                argv: Vec::new(),
                exe: PathBuf::from(exe),
                children: children.iter().copied().map(ProcessId).collect(),
            },
        );
        self
    }
}

impl ProcessTree for FakeTree {
    fn cwd(&self, _process: ProcessId) -> Result<PathBuf, PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn children(&self, process: ProcessId) -> Result<Vec<ProcessId>, PlatformError> {
        self.processes
            .get(&process.0)
            .map(|held| held.children.clone())
            .ok_or(PlatformError::NotFound)
    }

    fn argv(&self, process: ProcessId) -> Result<Vec<OsString>, PlatformError> {
        self.processes
            .get(&process.0)
            .map(|held| held.argv.clone())
            .ok_or(PlatformError::NotFound)
    }

    fn exe(&self, process: ProcessId) -> Result<PathBuf, PlatformError> {
        self.processes
            .get(&process.0)
            .map(|held| held.exe.clone())
            .ok_or(PlatformError::NotFound)
    }

    fn is_alive(&self, process: ProcessId) -> bool {
        self.processes.contains_key(&process.0)
    }
}

/// A directory under `$TMPDIR`, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    ///
    /// Short on purpose: the path prefixes a unix socket in `Ctx`, and darwin's
    /// `$TMPDIR` alone eats half the `sun_path` budget.
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let brief: String = tag.chars().take(4).collect();
        let path = std::env::temp_dir().join(format!("i{}-{n}-{brief}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A live `Core` on its real actor loop, over paths under a temp dir.
///
/// The spawn path is the subject of two acceptances, and it is reachable only
/// through the running loop: `Core::absorb` mints pane state without starting a
/// process, so a test that folded commands synchronously would assert about a
/// pane that never had a child.
pub struct Panes {
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    task: JoinHandle<Core>,
}

impl Panes {
    /// Start one.
    pub fn start(root: &Path) -> Self {
        let runtime_dir = root.join("run");
        std::fs::create_dir_all(&runtime_dir).expect("create the runtime dir");
        let ctx = Ctx {
            session: SessionName::new("identity").expect("valid session name"),
            socket: runtime_dir.join("sock"),
            runtime_dir,
            state_dir: root.join("state"),
            config_path: root.join("config/amx/config.toml"),
            bus: Arc::new(Bus::new(64)),
            cancel: CancellationToken::new(),
        };
        let (tx, rx) = mpsc::channel(32);
        let core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));
        let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));
        Self { ctx, tx, task }
    }

    /// The session context the panes were spawned against.
    pub const fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Create a workspace and return its root pane.
    pub async fn root_pane(&self) -> PaneId {
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
            .send(CoreCommand::Session(SessionCall::State {
                params: session::StateParams::default(),
                reply,
            }))
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

    /// Split `from`, running `command` in the new pane.
    pub async fn split(&self, from: PaneId, command: Vec<String>) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Pane(PaneCall::Split {
                params: pane::SplitParams {
                    pane: from,
                    direction: pane::SplitDirection::Vertical,
                    command: Some(command),
                    // Explicit, so the split never asks another pane anything.
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

    /// Cancel the session and take the `Core` back, so its state can be read.
    ///
    /// Cancellation and not a dropped sender: `Core` hands a clone of its own
    /// mailbox to every pane it spawns, so the mailbox only closes once the
    /// panes are gone, and the panes only go once the loop breaks.
    ///
    /// `Core::run` joins every pane task on the way out, so the children the
    /// tests started are gone by the time this returns.
    pub async fn finish(self) -> Core {
        self.ctx.cancel.cancel();
        drop(self.tx);
        tokio::time::timeout(PATIENCE, self.task)
            .await
            .expect("the core loop returned before the deadline")
            .expect("the core task did not panic")
    }
}

/// Read `path` once it exists and is non-empty, or fail at the deadline.
///
/// A condition, not a nap: the child writes to a temporary name and renames it
/// into place, so the file appearing *is* the file being complete.
pub async fn read_when_written(path: &Path) -> String {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} was never written",
            path.display()
        );
        tokio::time::sleep(TICK).await;
    }
}
