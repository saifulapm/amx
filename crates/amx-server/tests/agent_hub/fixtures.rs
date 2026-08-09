//! The rig the hub suite drives: a stand-in `Core`, panes with no processes,
//! and the builder that wires a real `AgentHub` to both.
//!
//! Everything the hub talks to is a seam, which is what makes the suite honest
//! without a running session. `Core` is an ordinary mailbox receiver, so "the
//! hub asked a sibling for something" is not an assertion but the only way the
//! request could arrive at all. The bus is real. The `StatusView` is real. The
//! fusion machine, the registry and the manifests are the shipped ones.
//!
//! The panes are the interesting fake. A [`SnapshotFeed`] can only come from a
//! `PaneHost`, and a `PaneHost` needs a [`PtySession`] — so [`FakePty`] is one
//! over a socket pair that is never written to: the pty thread parks in its
//! `poll()` forever and no child process is involved. The screen is painted
//! with `PaneCommand::Seed`, which is the path restored scrollback takes into a
//! fresh terminal, so the grid the hub reads is a real libghostty-vt grid — and
//! `PaneDamage` is published by the test rather than by the pane, which is what
//! makes "how many evaluations did that burst cost" a number rather than a
//! race.

use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use amx_core::agent::{AgentKind, HookToken, RefSource};
use amx_core::platform::{PlatformError, ProcessId, PtySession, WinSize};
use amx_core::{Bus, Ctx, Event, GridGeneration, PaneId, SessionName, WorkspaceId};
use amx_proto::control::agent as proto;
use amx_server::actor::agent_hub::inherit::InheritedPane;
use amx_server::actor::agent_hub::{AgentHub, AgentProbe, AgentReport};
use amx_server::actor::{
    AGENT_MAILBOX, AgentCall, AgentCommand, AgentHandle, CoreCommand, CoreHandle, PaneCommand,
    PaneHost, PaneHostConfig, SnapshotFeed, SpawnedIdentity, StatusView,
};
use amx_server::agent::registry::Registry;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::support::TempDir;

/// The grid every pane in this suite runs at.
///
/// Wide and tall enough for the recorded Claude Code screens under
/// `tests/fixtures/manifest/`, which were captured at 100×30: a narrower grid
/// would wrap them and a shorter one would scroll their top away, and either
/// would be testing the rig rather than the rules.
pub const SIZE: WinSize = WinSize {
    rows: 40,
    cols: 110,
};

/// A registry override naming one `claude` that is believed immediately.
///
/// The same stanza the binary ships, with `startup_grace_ms = 0`: the grace is
/// there so a booting TUI's splash cannot be read as state, and a suite that
/// paints a settled screen on purpose has nothing to wait for. This is D-M2-2's
/// "the override merge is also the test seam" — the suite plants a stanza
/// rather than patching the binary.
pub const IMPATIENT_CLAUDE: &str = r#"
[[agent]]
id          = "claude"
aliases     = ["claude-code"]
label       = "Claude Code"
executables = ["claude"]
coverage    = "edges"
start       = ["claude"]
resume      = ["claude", "--resume", "{ref}"]
ref_kind    = "id"
manifest    = "claude.toml"
startup_grace_ms = 0
hook_events = ["SessionStart", "UserPromptSubmit", "Stop", "PermissionRequest"]
"#;

// ------------------------------------------------------------- the fake pty

/// A pty session with nothing on the far end.
///
/// Both ends of the socket pair are held here and neither is ever written to,
/// so the pty thread's `poll()` reports the descriptor unreadable for as long
/// as the pane lives — no child, no output, no exit.
pub struct FakePty {
    ours: UnixStream,
    _theirs: UnixStream,
}

impl FakePty {
    fn pair() -> Self {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        Self {
            ours,
            _theirs: theirs,
        }
    }
}

impl AsFd for FakePty {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.ours.as_fd()
    }
}

impl PtySession for FakePty {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize, PlatformError> {
        Err(PlatformError::WouldBlock)
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, PlatformError> {
        Ok(buf.len())
    }

    fn resize(&mut self, _size: WinSize) -> Result<(), PlatformError> {
        Ok(())
    }

    fn child(&self) -> ProcessId {
        ProcessId(std::process::id())
    }

    fn foreground_group(&self) -> Result<ProcessId, PlatformError> {
        Ok(ProcessId(std::process::id()))
    }

    fn try_wait(&mut self) -> Result<Option<Option<i32>>, PlatformError> {
        Ok(None)
    }

    fn kill(&mut self) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// A live pane whose grid the test paints directly.
pub struct FakePane {
    /// Which pane this is.
    pub pane: PaneId,
    host: PaneHost,
}

impl FakePane {
    /// Start a pane publishing on `bus`.
    pub fn start(bus: &Arc<Bus>, pane: PaneId) -> Self {
        let config = PaneHostConfig::new(pane, Arc::clone(bus), SIZE);
        let host = PaneHost::spawn(config, FakePty::pair()).expect("start the pane host");
        Self { pane, host }
    }

    /// The feed `Core` would hand the hub at `PaneStarted`.
    pub fn feed(&self) -> SnapshotFeed {
        self.host.frames()
    }

    /// Paint `text` onto the grid and publish the frame it produced.
    ///
    /// `Seed` is the parser-thread path restored scrollback takes, so the bytes
    /// reach the terminal without going anywhere near the (absent) child, and
    /// the snapshot command publishes before it answers — so a `paint` that has
    /// returned is a frame the hub can already read.
    pub async fn paint(&self, text: &str) {
        let mut bytes = b"\x1b[2J\x1b[H".to_vec();
        for line in text.lines() {
            bytes.extend_from_slice(line.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
        self.host
            .handle()
            .send(PaneCommand::Seed(bytes))
            .await
            .expect("the pane is running");
        let (reply, answer) = oneshot::channel();
        self.host
            .handle()
            .send(PaneCommand::TakeSnapshot(reply))
            .await
            .expect("the pane is running");
        answer.await.expect("the parser published a frame");
    }

    /// Stop the pane and join its threads.
    pub async fn stop(self) {
        self.host.kill();
        let _ = self.host.shutdown().await;
    }
}

// ------------------------------------------------------------ the fake Core

/// What the stand-in `Core` was told.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    /// Every status mirror, in order.
    pub statuses: Vec<(PaneId, Option<amx_core::agent::AgentSnapshot>)>,
    /// Every attention queue mirrored, in order.
    pub queues: Vec<Vec<PaneId>>,
    /// Every pane the hub asked `Core` to focus.
    pub focused: Vec<PaneId>,
    /// Anything else that arrived — every one of these is a *request* to a
    /// sibling, which is what the shutdown discipline forbids.
    pub others: usize,
}

/// A window onto the stand-in `Core`.
#[derive(Clone, Debug)]
pub struct Spy {
    seen: Arc<Mutex<Seen>>,
}

impl Spy {
    /// Everything the stand-in `Core` has been told so far.
    pub fn seen(&self) -> Seen {
        self.seen.lock().expect("the spy is not poisoned").clone()
    }

    /// The last status mirrored for `pane`, if any.
    pub fn status_of(&self, pane: PaneId) -> Option<amx_core::agent::AgentSnapshot> {
        self.seen()
            .statuses
            .iter()
            .rev()
            .find(|(mirrored, _)| *mirrored == pane)
            .and_then(|(_, status)| status.clone())
    }
}

// ------------------------------------------------------------------ the rig

/// A running `AgentHub` with everything it talks to under test control.
pub struct Rig {
    /// The session context — its bus is what a test publishes on, its
    /// cancellation token what a test uses to shut the hub down.
    pub ctx: Ctx,
    /// The hub's mailbox.
    pub agent: AgentHandle,
    /// The fast read model, exactly as a wait predicate would hold it.
    pub view: StatusView,
    /// The hub's live counters.
    pub probe: AgentProbe,
    /// What the stand-in `Core` has been told.
    pub spy: Spy,
    actor: JoinHandle<AgentReport>,
}

/// How long a condition has to come true before the suite calls it wedged.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a poll loop waits between looks at its condition.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(5);

/// Wait for `condition`, failing with `what` if it never comes true.
///
/// A condition plus a deadline, never a nap: the deadline's expiry is the
/// failure path, which is the shape `tests/hygiene.rs` enforces across the rig.
pub async fn wait_for(mut condition: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(TICK).await;
    }
}

/// Builds a [`Rig`].
pub struct Builder {
    root: PathBuf,
    registry: Option<Registry>,
    inherited: Option<(Vec<InheritedPane>, Vec<PaneId>)>,
}

impl Rig {
    /// A rig whose paths live under `root`.
    pub fn under(root: &TempDir) -> Builder {
        Builder {
            root: root.path().to_path_buf(),
            registry: None,
            inherited: None,
        }
    }

    /// Publish one event on the session bus.
    pub fn publish(&self, event: Event) {
        self.ctx.bus.publish(event);
    }

    /// Publish one damage batch for `pane`.
    pub fn damage(&self, pane: PaneId) {
        self.publish(Event::PaneDamage {
            pane,
            generation: GridGeneration::FIRST,
        });
    }

    /// Hand the hub a pane, the way `Core` does at spawn.
    pub async fn started(&self, pane: &FakePane, token: &HookToken, requested: Option<&str>) {
        self.agent
            .send(AgentCommand::PaneStarted {
                pane: pane.pane,
                frames: pane.feed(),
                spawn: SpawnedIdentity {
                    argv: vec!["/bin/sh".to_owned()],
                    token: token.clone(),
                    requested: requested.map(kind),
                },
            })
            .await
            .expect("the hub is running");
    }

    /// Send one hook report and wait for its acknowledgement.
    pub async fn report(&self, params: proto::ReportParams) -> proto::ReportReply {
        let (reply, answer) = oneshot::channel();
        let (pane, token) = (params.pane, params.token.clone());
        self.agent
            .send(AgentCommand::HookReport {
                pane,
                token,
                report: Box::new(params),
                reply,
            })
            .await
            .expect("the hub is running");
        answer
            .await
            .expect("the hub answers")
            .expect("a report is never an error")
    }

    /// Ask why a pane is in the state it is in.
    pub async fn explain(&self, pane: PaneId) -> proto::ExplainReply {
        let (reply, answer) = oneshot::channel();
        self.agent
            .send(AgentCommand::Explain { pane, reply })
            .await
            .expect("the hub is running");
        answer
            .await
            .expect("the hub answers")
            .expect("the pane is tracked")
    }

    /// Ask for the head of the attention queue.
    pub async fn next_attention(&self) -> proto::NextReply {
        self.next_attention_in(None).await
    }

    /// Ask for the head of `workspace`'s share of the attention queue, or of
    /// the whole queue when it is `None`.
    pub async fn next_attention_in(&self, workspace: Option<WorkspaceId>) -> proto::NextReply {
        let (reply, answer) = oneshot::channel();
        self.agent
            .send(AgentCommand::NextAttention { workspace, reply })
            .await
            .expect("the hub is running");
        answer
            .await
            .expect("the hub answers")
            .expect("an empty queue is not an error")
    }

    /// Let every other task run to a standstill.
    pub async fn settle(&self) {
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
    }

    /// Cancel the session, close the mailbox, and join the hub.
    pub async fn stop(self) -> AgentReport {
        self.ctx.cancel.cancel();
        drop(self.agent);
        self.actor.await.expect("the hub must not panic")
    }
}

impl Builder {
    /// Run the hub against `text` merged over the builtin registry.
    pub fn registry(mut self, text: &str) -> Self {
        let (merged, diagnostics) = Registry::builtin().with_override(text);
        assert!(
            diagnostics.is_empty(),
            "the suite's own override must be clean: {diagnostics:?}",
        );
        self.registry = Some(merged);
        self
    }

    /// Seed the hub the way a live upgrade does, before it runs.
    ///
    /// The import path's own call, in the same order: `AgentHub::inherit`
    /// before `run`, then a `PaneStarted` per pane once the hub is listening
    /// (`session/import.rs` step 9, then `Core::announce_inherited`). It
    /// publishes nothing, so it is the one path where labels arrive by hand.
    pub fn inherit(mut self, panes: Vec<InheritedPane>, attention: Vec<PaneId>) -> Self {
        self.inherited = Some((panes, attention));
        self
    }

    /// Spawn the hub and the stand-in `Core` behind it.
    pub fn start(self) -> Rig {
        let ctx = Ctx {
            session: SessionName::new("test").expect("valid session name"),
            runtime_dir: self.root.join("run"),
            socket: self.root.join("run/sock"),
            state_dir: self.root.join("state"),
            config_path: self.root.join("config/amx/config.toml"),
            bus: Arc::new(Bus::new(64)),
            cancel: CancellationToken::new(),
        };

        let (core_tx, core_rx) = mpsc::channel(64);
        let spy = spawn_core(core_rx);

        let events = ctx.bus.subscribe();
        let view = StatusView::new();
        let mut hub = AgentHub::new(ctx.clone(), CoreHandle::new(core_tx), view.clone());
        if let Some(registry) = self.registry {
            hub = hub.with_registry(registry);
        }
        if let Some((panes, attention)) = self.inherited {
            hub = hub.inherit(panes, attention);
        }
        let probe = hub.probe();

        let (agent_tx, agent_rx) = mpsc::channel(AGENT_MAILBOX);
        let actor = tokio::spawn(hub.run(agent_rx, events));

        Rig {
            ctx,
            agent: AgentHandle::new(agent_tx),
            view,
            probe,
            spy,
            actor,
        }
    }
}

/// Run a stand-in `Core` over `mailbox`, recording what the hub tells it.
fn spawn_core(mut mailbox: mpsc::Receiver<CoreCommand>) -> Spy {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(command) = mailbox.recv().await {
            let mut seen = log.lock().expect("the spy is not poisoned");
            match command {
                CoreCommand::Agent(AgentCall::Status {
                    pane,
                    status,
                    attention,
                }) => {
                    seen.statuses.push((pane, status.map(|status| *status)));
                    seen.queues.push(attention);
                }
                CoreCommand::Agent(AgentCall::Focus { pane }) => seen.focused.push(pane),
                _ => seen.others += 1,
            }
        }
    });
    Spy { seen }
}

// ------------------------------------------------------------------ payloads

/// An agent name that parses.
pub fn kind(name: &str) -> AgentKind {
    AgentKind::new(name).expect("a valid agent name")
}

/// The `n`th pane of the fixture session, stable across runs.
pub fn pane_id(n: usize) -> PaneId {
    format!("00000000-0000-0000-0000-{:012x}", n + 1)
        .parse()
        .expect("a valid pane id")
}

/// The workspace the fixture events name.
pub fn workspace_id() -> WorkspaceId {
    workspace_id_n(1)
}

/// The `n`th workspace of the fixture session, stable across runs.
pub fn workspace_id_n(n: usize) -> WorkspaceId {
    format!("00000000-0000-0000-0000-0000000000b{n:x}")
        .parse()
        .expect("a valid workspace id")
}

/// One hook report, shaped as `amx _hook` sends it.
///
/// The payload fields are V01's recordings: a `session_id` that is a v4-shaped
/// UUID, a `source` naming the agent whose hook path this is, and a `seq` from
/// the emitter's monotonic nanos.
pub fn report(
    pane: PaneId,
    token: &HookToken,
    agent: &str,
    event: proto::HookEvent,
) -> proto::ReportParams {
    proto::ReportParams {
        pane,
        token: token.clone(),
        agent: kind(agent),
        source: RefSource::for_agent(&kind(agent)),
        event,
        seq: 1,
        scope: proto::ReportScope::Parent,
        session_id: Some("6f1d2a70-0000-4000-8000-00000000c0de".to_owned()),
        transcript_path: None,
        start_source: None,
        tool: None,
        notification: None,
    }
}

/// A hook token nothing else in the suite mints.
pub fn token(tag: &str) -> HookToken {
    HookToken::new(format!("token-{tag}"))
}

/// Read one of the recorded agent screens under `tests/fixtures/manifest/`.
pub fn screen(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/manifest")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}
