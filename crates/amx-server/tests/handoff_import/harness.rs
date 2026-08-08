//! The rig W07's suite drives: an exporter played by hand, a successor run in
//! process, and the wire helpers that ask it questions.
//!
//! Split from the tests themselves for the module budget, and along the seam
//! that keeps both halves readable: everything here is *the other side of a
//! handoff* — a real pty with a real child, frozen the way the export path will
//! freeze it; W05's state machine scripted where W06's orchestrator will drive
//! it; and a client that speaks frames.

#![allow(dead_code, reason = "the suite uses a subset of the rig")]

use std::ffi::OsString;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::support::{self, Client, TempDir, connect_to, ctx_under};
use amx_core::agent::{AgentKind, AgentSnapshot, AgentState, HookToken, StatusCause};
use amx_core::platform::{ProcessId, Pty, PtyCommand, WinSize};
use amx_core::{Bus, Ctx, Delivery, Event, Layout, PaneId, Seq, SessionId, ShortNumber};
use amx_proto::rpc::Notification;
use amx_server::actor::{PaneCommand, PaneExport, PaneHost, PaneHostConfig};
use amx_server::handoff::manifest::{AgentEntry, Manifest, PaneManifest, READ_WINDOW, VERSION};
use amx_server::handoff::protocol::{HandoffError, HandoffListener, Timeouts, Token, socket_path};
use amx_server::persist::{PaneSnapshot, Snapshot, WorkspaceSnapshot};
use amx_server::platform::UnixPty;
use amx_server::session::import::{ImportFailed, ImportOptions, import};
use amx_server::session::serve::StopOn;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// How long a test waits for something that should already have happened.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// How long a poll loop waits between looks.
pub const TICK: Duration = Duration::from_millis(10);

/// A frame interval short enough that a test does not spend its patience
/// waiting for output to coalesce.
pub const FRAME: Duration = Duration::from_millis(2);

/// The grid every frozen pane is captured at.
pub const SIZE: WinSize = WinSize { rows: 24, cols: 80 };

/// A child that does nothing until its terminal is taken away.
pub const SLEEPER: &str = "sleep 300";

/// Stage bounds short enough that a peer that will never answer is a test
/// failure and not a coffee break. §3's own numbers are pinned by W05.
pub fn brisk() -> Timeouts {
    Timeouts {
        stage: Duration::from_millis(300),
        owned_ack: Duration::from_millis(80),
        socket_free: Duration::from_secs(5),
    }
}

// ---------------------------------------------------------------- the rig

/// A private directory, the successor's context, and the handoff socket.
pub struct Rig {
    pub ctx: Ctx,
    pub handoff: PathBuf,
    pub token: Token,
    _dir: TempDir,
}

impl Rig {
    pub fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        let handoff = socket_path(dir.path(), std::process::id());
        Self {
            ctx,
            handoff,
            token: Token::mint(),
            _dir: dir,
        }
    }

    /// Take the session socket the way the exporter is holding it.
    ///
    /// It *answers*, which is the point: §3 keeps the exporter's gateway alive
    /// through `restored` so that an `amx` typed mid-handoff finds a live
    /// server rather than daemonizing a rival, and what "finds a live server"
    /// means is the connect probe getting a frame back. A listener that merely
    /// existed would make every probe wait out its one-second patience, which
    /// is a slower answer than any real server gives.
    pub fn hold_socket(&self) -> Held {
        std::fs::create_dir_all(&self.ctx.runtime_dir).expect("the runtime directory");
        let listener = UnixListener::bind(&self.ctx.socket).expect("hold the session socket");
        listener.set_nonblocking(true).expect("a pollable listener");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let answering = std::thread::spawn(move || {
            use std::io::Write as _;

            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut frame =
                            amx_proto::FrameHeader::new(2, amx_proto::frame::CONTROL_CHANNEL)
                                .encode()
                                .to_vec();
                        frame.extend_from_slice(b"{}");
                        let _ = stream.write_all(&frame);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(TICK);
                    }
                    Err(_) => break,
                }
            }
        });
        Held {
            stop,
            answering,
            path: self.ctx.socket.clone(),
        }
    }

    /// Start the importer, exactly as `amx server --handoff-import` does.
    pub fn start_import(&self, timeouts: Timeouts) -> Successor {
        let ctx = self.ctx.clone();
        let options = ImportOptions {
            socket: self.handoff.clone(),
            token: self.token.clone(),
            timeouts,
            stop: StopOn::Cancellation,
        };
        Successor {
            ctx: self.ctx.clone(),
            task: tokio::spawn(async move { import(ctx, options).await }),
        }
    }
}

/// The session socket, held and answered by the server on its way out.
pub struct Held {
    stop: Arc<std::sync::atomic::AtomicBool>,
    answering: std::thread::JoinHandle<()>,
    path: PathBuf,
}

impl Held {
    /// §3 step 11: give the path up, then stop answering.
    ///
    /// The unlink goes *first*, and that ordering is the harness's own, not
    /// §3's. A real gateway unlinks the socket it bound and can therefore do it
    /// last; this holder would be racing the successor for the name — between
    /// dropping the listener and removing the file, the successor's bind can
    /// clear the stale entry and create its own, and a removal that landed
    /// afterwards would delete a socket somebody is serving on. Unlinking
    /// first leaves the doomed listener answering a name nothing resolves to,
    /// which is harmless.
    pub fn retire(self) {
        let _ = std::fs::remove_file(&self.path);
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.answering.join();
    }
}

/// The importer under test, running as the process's whole reason to exist.
pub struct Successor {
    pub ctx: Ctx,
    pub task: JoinHandle<Result<amx_server::session::serve::ServeReport, ImportFailed>>,
}

impl Successor {
    /// Wait until the successor answers on the session socket *as a server*.
    ///
    /// Neither a connect nor a frame is enough, and the test learned both the
    /// hard way: until it retires, the exporter holds the same path and answers
    /// on it, which is exactly what §3 wants it to do. Only a decodable welcome
    /// distinguishes the two processes — see [`answers`].
    pub async fn ready(&self) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            if answers(&self.ctx.socket).await {
                return;
            }
            // An import that has already ended is not going to bind: say that
            // instead of waiting out the patience and reporting a timeout for
            // a failure that has a name.
            assert!(
                !self.task.is_finished(),
                "the import ended before it answered on {}",
                self.ctx.socket.display()
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "the successor never bound {}",
                self.ctx.socket.display()
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Connect a client once the successor is answering.
    pub async fn client(&self) -> Client {
        self.ready().await;
        assert!(
            !self.task.is_finished(),
            "the import ended before a client could connect"
        );
        connect_to(&self.ctx.socket).await
    }

    /// Stop it the way a `SIGTERM` would, and take its report.
    pub async fn stop(self) {
        self.ctx.cancel.cancel();
        let report = tokio::time::timeout(PATIENCE, self.task)
            .await
            .expect("the successor stopped")
            .expect("the import task did not panic")
            .expect("the import served");
        assert!(report.clean(), "the successor stopped cleanly: {report:?}");
    }

    /// Take the failure it ended with, having served nothing.
    pub async fn failure(self) -> ImportFailed {
        tokio::time::timeout(PATIENCE, self.task)
            .await
            .expect("the import ended")
            .expect("the import task did not panic")
            .expect_err("a strict abort never serves")
    }
}

// ------------------------------------------------------- the exporter's half

/// One pane, frozen exactly as the export path will freeze it.
///
/// The pane host here is the *exporter's*: a real pty, a real child, a real
/// terminal, quiesced and then captured on its own parser thread. What crosses
/// is [`Frozen::master`]; what stays behind is dropped before the commit, the
/// way the exporter's own actors are.
pub struct Frozen {
    host: Option<PaneHost>,
    entry: PaneManifest,
    pub master: OwnedFd,
    /// A second duplicate, so the test can type at the child's terminal from
    /// outside either server.
    pub control: OwnedFd,
    child: ProcessId,
}

impl Frozen {
    /// Spawn `script` under `/bin/sh` on a real pty, then freeze it.
    pub async fn new(script: &str) -> Self {
        let command = PtyCommand {
            program: OsString::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from(script)],
            env: vec![
                (OsString::from("TERM"), OsString::from("xterm-256color")),
                (OsString::from("LC_ALL"), OsString::from("C")),
            ],
            cwd: None,
            size: SIZE,
        };
        let session = UnixPty.spawn(&command).expect("spawn a pty");
        let bus = Arc::new(Bus::new(64));
        let mut config = PaneHostConfig::new(PaneId::new_v4(), Arc::clone(&bus), SIZE);
        config.frame_interval = FRAME;
        let host = PaneHost::spawn(config, session).expect("start the exporter's pane");
        tokio::task::block_in_place(|| host.quiesce()).expect("the pane quiesces");
        let (reply, answer) = oneshot::channel();
        host.handle()
            .send(PaneCommand::ExportHandoff { reply })
            .await
            .expect("the pane is running");
        let PaneExport { manifest, master } = tokio::time::timeout(PATIENCE, answer)
            .await
            .expect("an export before the deadline")
            .expect("the parser answered")
            .expect("a quiesced pane exports");
        let control = rustix::io::fcntl_dupfd_cloexec(master.as_fd(), 0).expect("a second handle");
        let child = ProcessId(manifest.child);
        Self {
            host: Some(host),
            entry: manifest,
            master,
            control,
            child,
        }
    }

    pub fn pane(&self) -> PaneId {
        self.entry.pane
    }

    /// Say which token this pane's child carries in its environment.
    ///
    /// `Core` fills this in as it assembles the manifest; here the exporter is
    /// a script, so the test says it.
    pub fn carry_token(&mut self, token: HookToken) {
        self.entry.token = Some(token);
    }

    /// Type at the child's terminal, from outside both servers.
    pub fn type_in(&self, bytes: &[u8]) {
        rustix::io::write(self.control.as_fd(), bytes).expect("write to the terminal");
    }

    /// Let the exporter's own actors go, as its shutdown does after the
    /// commit: the descriptions stay open because the successor holds them.
    pub async fn retire(&mut self) {
        if let Some(host) = self.host.take() {
            host.shutdown().await.expect("the exporter's pane stopped");
        }
    }

    /// Hang the child up, so no test leaves a `sleep` behind.
    pub fn kill_child(&self) {
        if let Some(pid) = i32::try_from(self.child.0)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

/// How far the scripted exporter goes before it stops talking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Plan {
    /// All of §3: commit, and read the advisory ack.
    Commit,
    /// Wedged, not dead: bound, ready, and then nothing — the row of the crash
    /// table the importer's strict abort exists for.
    WedgeBeforeCommit,
}

/// Everything the scripted exporter needs to play its half.
pub struct Script {
    pub listener: HandoffListener,
    pub token: Token,
    pub timeouts: Timeouts,
    pub manifest: Value,
    pub masters: Vec<OwnedFd>,
    /// The session socket, when this test wants the transition exercised.
    pub held: Option<Held>,
    pub plan: Plan,
}

/// Run the exporter's half of §3 on the blocking pool.
///
/// Blocking because both state machines are (W05: ancillary data has no async
/// surface), and on its own thread because the importer is running the other
/// half of the same conversation.
pub fn spawn_exporter(script: Script) -> JoinHandle<Result<(), HandoffError>> {
    let Script {
        listener,
        token,
        timeouts,
        manifest,
        masters,
        held,
        plan,
    } = script;
    tokio::task::spawn_blocking(move || {
        let exporter = listener.accept(token, timeouts)?;
        let exporter = exporter.authenticate()?;
        let exporter = exporter.send_manifest(manifest)?;
        let exporter = exporter.await_validated()?;
        let borrowed: Vec<BorrowedFd<'_>> = masters.iter().map(AsFd::as_fd).collect();
        let exporter = exporter.send_masters(&borrowed)?;
        let exporter = exporter.await_restored()?;
        // §3 step 11, in the order that matters: the socket is only given up
        // once the successor has said it holds everything.
        if let Some(held) = held {
            held.retire();
        }
        let exporter = exporter.await_ready()?;
        if plan == Plan::WedgeBeforeCommit {
            // Not a peer that died — a peer that stopped. The socket stays
            // open under this thread's return, which is what makes the
            // importer's failure a *timeout* rather than an end of file.
            std::thread::sleep(timeouts.stage * 4); // deliberate
            return Ok(());
        }
        let exporter = exporter.commit()?;
        exporter.await_owned();
        Ok(())
    })
}

// ------------------------------------------------------------- the manifest

/// The session-level manifest W06 will build, built by hand.
pub fn manifest(
    session: SessionId,
    seq: Seq,
    panes: &[&Frozen],
    agents: Vec<AgentEntry>,
) -> Manifest {
    let workspace = amx_core::WorkspaceId::new_v4();
    let mut layout = Layout::with_root(panes[0].pane());
    for (n, extra) in panes.iter().enumerate().skip(1) {
        layout
            .split(
                panes[0].pane(),
                amx_core::layout::Direction::Right,
                extra.pane(),
                0.5,
            )
            .unwrap_or_else(|err| panic!("split {n}: {err}"));
    }
    let state = Snapshot {
        version: amx_server::persist::VERSION,
        focused_workspace: Some(workspace),
        workspaces: vec![WorkspaceSnapshot {
            id: workspace,
            short: ShortNumber::new(1),
            label: Some("carried".to_owned()),
            layout,
            focus: Some(panes[0].pane()),
            worktree: None,
        }],
        panes: panes
            .iter()
            .enumerate()
            .map(|(n, frozen)| PaneSnapshot {
                id: frozen.pane(),
                short: ShortNumber::new(u32::try_from(n).unwrap_or(0) + 1),
                label: Some(format!("pane-{n}")),
                cwd: None,
                // A pane the exporter tracked an agent in was *spawned* as one:
                // `agent start` runs the stanza's argv, and the recorded argv
                // is what tier 3 would identify the pane from all over again on
                // the successor. Recording it is what makes the flap the
                // successor must not produce reachable at all.
                argv: Some(vec![
                    if agents.iter().any(|entry| entry.pane == frozen.pane()) {
                        "claude".to_owned()
                    } else {
                        "/bin/sh".to_owned()
                    },
                ]),
                agent: None,
            })
            .collect(),
    };
    Manifest {
        version: VERSION,
        read_window: READ_WINDOW.to_vec(),
        exporter: "0.1.0".to_owned(),
        proto: amx_proto::version::window(),
        session,
        seq,
        state: Box::new(state),
        panes: panes.iter().map(|frozen| frozen.entry.clone()).collect(),
        agents,
    }
}

/// A carried agent status, as the hub's mirror holds one.
pub fn agent(pane: PaneId, state: AgentState, attention: Option<u32>) -> AgentEntry {
    AgentEntry {
        pane,
        agent: AgentSnapshot {
            kind: Some(AgentKind::new("claude").expect("a valid agent kind")),
            state,
            cause: StatusCause::Hook,
            transition_seq: 7,
            attention,
            session_ref: None,
        },
    }
}

/// The manifest as the wire carries it.
pub fn document(manifest: &Manifest) -> Value {
    serde_json::to_value(manifest).expect("the manifest serializes")
}

// ------------------------------------------------------------ wire helpers

/// `session.state`, as the successor answers it.
pub async fn state_of(client: &mut Client) -> Value {
    let response = client.request(1, "session.state", json!({})).await;
    support::result_of(&response).clone()
}

/// The visible grid of `pane`, as `pane.read` serves it.
pub async fn read_pane(client: &mut Client, pane: PaneId, id: u64) -> String {
    let response = client
        .request(id, "pane.read", json!({ "target": pane.to_string() }))
        .await;
    let rows = support::result_of(&response)["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    rows.join("\n")
}

/// Poll `pane.read` until its text satisfies `want`, or give up.
pub async fn wait_for_screen(
    client: &mut Client,
    pane: PaneId,
    want: impl Fn(&str) -> bool,
) -> String {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut id = 100;
    loop {
        let screen = read_pane(client, pane, id).await;
        if want(&screen) {
            return screen;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pane never showed what was expected; it showed {screen:?}"
        );
        id += 1;
        tokio::time::sleep(TICK).await;
    }
}

/// Whether a real server on `socket` answers a hello with a welcome.
///
/// A frame is not enough, and the test learned that the hard way too: the
/// exporter's socket answers the connect probe — that is its whole job until
/// `restored` — so "somebody replied" is true of the process being replaced.
/// Only a decodable [`Welcome`](amx_proto::Welcome) says the successor is the
/// one on the other end.
pub async fn answers(socket: &std::path::Path) -> bool {
    let Ok(mut stream) = tokio::net::UnixStream::connect(socket).await else {
        return false;
    };
    if write_control(&mut stream, &hello_bytes()).await.is_err() {
        return false;
    }
    // Read the frame by hand rather than through the harness: the harness's
    // reader *asserts* that one arrives, and here a connection that resets
    // under a peer on its way out is one of the answers being polled for.
    let Some(payload) = read_control(&mut stream, Duration::from_millis(200)).await else {
        return false;
    };
    serde_json::from_slice::<amx_proto::Welcome>(&payload).is_ok()
}

/// One control frame's payload, or `None` if the peer never sent a whole one.
pub async fn read_control(
    stream: &mut tokio::net::UnixStream,
    within: Duration,
) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let read = async {
        let mut header = [0_u8; amx_proto::frame::FRAME_HEADER_LEN];
        stream.read_exact(&mut header).await.ok()?;
        let header = amx_proto::FrameHeader::decode(header).ok()?;
        let mut payload = vec![0_u8; header.payload_len()];
        stream.read_exact(&mut payload).await.ok()?;
        Some(payload)
    };
    tokio::time::timeout(within, read).await.ok().flatten()
}

/// The opening frame's payload: a hello that mutates nothing by arriving.
pub fn hello_bytes() -> Vec<u8> {
    use std::collections::BTreeSet;

    use amx_proto::{ClientInfo, Hello};

    let hello = Hello {
        proto: amx_proto::version::window(),
        features: BTreeSet::new(),
        client: ClientInfo {
            name: "amx-test-watcher".to_owned(),
            version: "0.0.0".to_owned(),
            term: None,
        },
        attach: false,
        resume: None,
    };
    serde_json::to_vec(&hello).expect("encode hello")
}

/// A second connection that does nothing but listen to the bus.
///
/// The successor's bus is the successor's — an import builds it at the
/// inherited head, inside the assembly — so the only honest way to watch it is
/// the way a client does. It is a *second* connection because a subscribed one
/// interleaves notifications with replies, and a test that reads a reply where
/// an event arrived would be testing its own frame reader.
pub struct Watcher {
    stream: tokio::net::UnixStream,
}

impl Watcher {
    /// Connect, say hello, subscribe from `after_seq`, and answer with the bus
    /// head at that moment.
    ///
    /// Subscribing from a sequence the swap is *behind* is what lets a test
    /// see events the successor published while it was still assembling: they
    /// are in the replay ring, and asking for them is what a reconnecting
    /// client does too.
    pub async fn open(socket: &std::path::Path, after_seq: Option<Seq>) -> (Self, Seq) {
        use amx_proto::{Request, RequestId, Response};

        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .expect("connect to the successor");
        write_control(&mut stream, &hello_bytes())
            .await
            .expect("write the hello");
        let _welcome = support::read_frame(&mut stream).await;
        let request = Request::new(
            RequestId::Number(1),
            "events.subscribe",
            Some(json!({ "after_seq": after_seq })),
        );
        write_control(
            &mut stream,
            &serde_json::to_vec(&request).expect("encode subscribe"),
        )
        .await
        .expect("write the subscribe");
        let (_, payload) = support::read_frame(&mut stream).await;
        let response: Response = serde_json::from_slice(&payload).expect("a subscribe reply");
        let seq = serde_json::from_value::<amx_proto::control::wait::SubscribeReply>(
            support::result_of(&response).clone(),
        )
        .expect("a subscribe reply")
        .seq;
        (Self { stream }, seq)
    }

    /// The next delivery, whatever it is.
    pub async fn next(&mut self, within: Duration) -> Option<Delivery> {
        loop {
            let frame = tokio::time::timeout(within, support::read_frame(&mut self.stream))
                .await
                .ok()?;
            let Ok(notification) = serde_json::from_slice::<Notification>(&frame.1) else {
                continue;
            };
            let Some(params) = notification.params else {
                continue;
            };
            if let Ok(delivery) = serde_json::from_value::<Delivery>(params) {
                return Some(delivery);
            }
        }
    }

    /// Read deliveries until one carries an event `want` accepts.
    pub async fn wait_for(&mut self, mut want: impl FnMut(&Event) -> bool) -> amx_core::Envelope {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let delivery = self
                .next(deadline.saturating_duration_since(tokio::time::Instant::now()))
                .await
                .expect("an event before the deadline");
            match delivery {
                Delivery::Event(envelope) if want(&envelope.event) => return envelope,
                Delivery::Event(_) => {}
                Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
            }
        }
    }

    /// Everything the successor has published, until it goes quiet for
    /// `settle`.
    pub async fn drain(&mut self, settle: Duration) -> Vec<Delivery> {
        let mut seen = Vec::new();
        while let Some(delivery) = self.next(settle).await {
            seen.push(delivery);
        }
        seen
    }
}

/// Write one control frame.
pub async fn write_control(
    stream: &mut tokio::net::UnixStream,
    payload: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let header = amx_proto::FrameHeader::new(
        u32::try_from(payload.len()).expect("a small frame"),
        amx_proto::frame::CONTROL_CHANNEL,
    );
    stream.write_all(&header.encode()).await?;
    stream.write_all(payload).await
}
