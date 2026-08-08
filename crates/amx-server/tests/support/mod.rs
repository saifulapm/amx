//! Shared scaffolding for the T10 socket tests: a session on a real socket and
//! a client that speaks frames rather than a client API, so a test can send
//! malformed ones.

#![allow(dead_code, reason = "each test binary uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

pub mod restore_rig;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use amx_core::{Bus, Ctx, Scheduled, SessionName};
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN};
use amx_proto::{
    ClientInfo, Feature, FrameHeader, Hello, Request, RequestId, Response, Resume, Welcome,
};
use amx_server::actor::CoreHandle;
use amx_server::actor::core::Core;
use amx_server::actor::gateway::{Gateway, GatewayProbe, GatewayReport};
use amx_server::runtime::{Runtime, ShutdownReport};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// How long a test waits for the server to do something.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// How long a poll loop waits between looks at its condition.
pub const TICK: Duration = Duration::from_millis(5);

/// A feature name no server build has, for testing the intersection.
pub const UNKNOWN_FEATURE: &str = "amx.test.feature-no-server-has";

/// A directory under `$TMPDIR`, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Kept short deliberately: this directory prefixes a unix socket path,
        // and darwin's $TMPDIR alone eats half the sun_path budget (~104
        // bytes). A four-char tag survives for debuggability.
        let brief: String = tag.chars().take(4).collect();
        let path = std::env::temp_dir().join(format!("a{}-{n}-{brief}", std::process::id()));
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

/// A `Ctx` whose paths all live under `root`, with a private bus and token.
pub fn ctx_under(root: &Path) -> Ctx {
    let runtime_dir = root.join("run/amx/test");
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        socket: runtime_dir.join("sock"),
        runtime_dir,
        state_dir: root.join("state"),
        config_path: root.join("config/amx/config.toml"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

/// A session: a `Core` actor and a `Gateway`, both under one `Runtime`.
pub struct Server {
    /// The context both actors share.
    pub ctx: Ctx,
    /// Live connection accounting.
    pub probe: GatewayProbe,
    runtime: Runtime,
    report: oneshot::Receiver<GatewayReport>,
    _dir: TempDir,
}

impl Server {
    /// Bind a session socket under a fresh temp directory and start serving.
    pub async fn start(tag: &str) -> Self {
        Self::start_with_replay(tag, 64).await
    }

    /// The same, with a bus whose replay ring holds `replay_capacity` events.
    ///
    /// The width of the replay ring is the width of the resume window, so a
    /// test about falling off the back of it needs to say which ring it means.
    pub async fn start_with_replay(tag: &str, replay_capacity: usize) -> Self {
        let dir = TempDir::new(tag);
        let mut ctx = ctx_under(dir.path());
        ctx.bus = Arc::new(Bus::new(replay_capacity));
        let mut runtime = Runtime::new(ctx.clone());

        let (core_tx, core_rx) = mpsc::channel(64);
        let core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
        runtime.spawn("core", async move {
            let _ = core.run(core_rx, |_: &Scheduled| {}).await;
        });

        let gateway =
            Gateway::bind(ctx.clone(), CoreHandle::new(core_tx)).expect("bind the session socket");
        let probe = gateway.probe().clone();
        let (report_tx, report) = oneshot::channel();
        runtime.spawn("gateway", async move {
            let _ = report_tx.send(gateway.run().await);
        });

        Self {
            ctx,
            probe,
            runtime,
            report,
            _dir: dir,
        }
    }

    /// The session socket.
    pub fn socket(&self) -> &Path {
        &self.ctx.socket
    }

    /// Connect a client to it.
    pub async fn connect(&self) -> Client {
        Client {
            stream: UnixStream::connect(self.socket()).await.expect("connect"),
        }
    }

    /// Connect and complete the handshake at this build's version window.
    pub async fn attach(&self) -> Client {
        let mut client = self.connect().await;
        client.hello(amx_proto::version::window()).await;
        client
    }

    /// Connect and complete the handshake as an attached client, so the
    /// session has a workspace with a live pane in it.
    pub async fn attach_rendering(&self) -> Client {
        let mut client = self.connect().await;
        client.hello_as_attach(amx_proto::version::window()).await;
        client
    }

    /// Cancel the session and join every task.
    pub async fn shutdown(self) -> (GatewayReport, ShutdownReport) {
        let shutdown = self.runtime.shutdown().await;
        let gateway = self.report.await.expect("the gateway reported");
        (gateway, shutdown)
    }
}

/// Connect to a session socket something else is serving.
///
/// [`Server`] assembles its own actors; a test that drives the real
/// [`serve`](amx_server::session::serve::serve) path has a socket and no
/// `Server`, and still wants the frame-level client.
pub async fn connect_to(socket: &Path) -> Client {
    Client {
        stream: UnixStream::connect(socket).await.expect("connect"),
    }
}

/// A raw client: it speaks frames, so a test can send malformed ones.
pub struct Client {
    stream: UnixStream,
}

impl Client {
    /// Send a control frame carrying `payload`.
    pub async fn send_control(&mut self, payload: &[u8]) {
        let header = FrameHeader::new(payload.len() as u32, CONTROL_CHANNEL);
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write");
        self.stream.write_all(payload).await.expect("write");
    }

    /// Send a bare header, with no payload behind it.
    pub async fn send_header(&mut self, header: FrameHeader) {
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write");
    }

    /// Send bytes with no framing at all.
    pub async fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("write");
    }

    /// Do the handshake, offering `proto` plus one feature no server has.
    pub async fn hello(&mut self, proto: (u16, u16)) -> Welcome {
        self.hello_with(proto, false).await
    }

    /// Do the handshake as an attached client: `attach: true`, which seeds an
    /// empty session with its first workspace before the welcome comes back.
    pub async fn hello_as_attach(&mut self, proto: (u16, u16)) -> Welcome {
        self.hello_with(proto, true).await
    }

    /// Do the handshake presenting a resume block: this is a reattach.
    pub async fn hello_resuming(&mut self, proto: (u16, u16), resume: Resume) -> Welcome {
        self.hello_full(proto, false, Some(resume)).await
    }

    /// Do the handshake as an attached client that is also reattaching.
    pub async fn hello_as_attach_resuming(&mut self, proto: (u16, u16), resume: Resume) -> Welcome {
        self.hello_full(proto, true, Some(resume)).await
    }

    async fn hello_with(&mut self, proto: (u16, u16), attach: bool) -> Welcome {
        self.hello_full(proto, attach, None).await
    }

    async fn hello_full(
        &mut self,
        proto: (u16, u16),
        attach: bool,
        resume: Option<Resume>,
    ) -> Welcome {
        let hello = Hello {
            proto,
            features: BTreeSet::from([Feature::GRID_STREAM, Feature::named(UNKNOWN_FEATURE)]),
            client: ClientInfo {
                name: "amx-test".to_owned(),
                version: "0.0.0".to_owned(),
                term: None,
            },
            attach,
            resume,
        };
        self.send_control(&serde_json::to_vec(&hello).expect("encode hello"))
            .await;
        let (header, payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "the welcome must be a control frame");
        serde_json::from_slice(&payload).expect("decode welcome")
    }

    /// Make one JSON-RPC call and read its reply.
    pub async fn request(&mut self, id: u64, method: &str, params: Value) -> Response {
        let request = Request::new(RequestId::Number(id), method, Some(params));
        self.send_control(&serde_json::to_vec(&request).expect("encode request"))
            .await;
        let (header, payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "a reply must be a control frame");
        serde_json::from_slice(&payload).expect("decode response")
    }

    /// Queue one JSON-RPC call without reading anything back.
    ///
    /// For calls whose reply can be overtaken by traffic the same call starts:
    /// `events.subscribe` spawns its pump before the dispatcher queues the
    /// reply, so a notification may reach the wire first and the caller has to
    /// read frames rather than *the* reply.
    pub async fn send_request(&mut self, id: u64, method: &str, params: Value) {
        let request = Request::new(RequestId::Number(id), method, Some(params));
        self.send_control(&serde_json::to_vec(&request).expect("encode request"))
            .await;
    }

    /// Read the next whole frame, whatever channel it is on.
    pub async fn next_frame(&mut self) -> (FrameHeader, Vec<u8>) {
        read_frame(&mut self.stream).await
    }

    /// Read the next whole frame, or `None` if nothing arrives within
    /// `patience`.
    ///
    /// The only way to assert a *silence*: "no keyframe" is a claim about what
    /// the server did not send, and there is no positive event for that.
    pub async fn next_frame_within(
        &mut self,
        patience: Duration,
    ) -> Option<(FrameHeader, Vec<u8>)> {
        tokio::time::timeout(patience, try_read_frame(&mut self.stream))
            .await
            .ok()
            .flatten()
    }

    /// Read until the server closes. `true` if it did so within [`PATIENCE`].
    pub async fn closed_by_server(&mut self) -> bool {
        let mut sink = [0_u8; 256];
        let closing = async {
            loop {
                match self.stream.read(&mut sink).await {
                    Ok(0) | Err(_) => return true,
                    Ok(_) => {}
                }
            }
        };
        tokio::time::timeout(PATIENCE, closing)
            .await
            .unwrap_or(false)
    }
}

/// Read one whole frame.
pub async fn read_frame<R: AsyncRead + Unpin>(source: &mut R) -> (FrameHeader, Vec<u8>) {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    tokio::time::timeout(PATIENCE, source.read_exact(&mut header))
        .await
        .expect("a frame header arrived")
        .expect("read the header");
    let header = FrameHeader::decode(header).expect("a decodable header");
    let mut payload = vec![0_u8; header.payload_len()];
    tokio::time::timeout(PATIENCE, source.read_exact(&mut payload))
        .await
        .expect("a frame payload arrived")
        .expect("read the payload");
    (header, payload)
}

/// Read one whole frame, or `None` if the peer stops mid-frame.
///
/// [`read_frame`]'s deadline is a test failure; this one's absence is an
/// answer, so it never panics on a short read.
pub async fn try_read_frame<R: AsyncRead + Unpin>(
    source: &mut R,
) -> Option<(FrameHeader, Vec<u8>)> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    source.read_exact(&mut header).await.ok()?;
    let header = FrameHeader::decode(header).expect("a decodable header");
    let mut payload = vec![0_u8; header.payload_len()];
    source.read_exact(&mut payload).await.ok()?;
    Some((header, payload))
}

/// Poll `cond` until it holds, failing the test if it never does.
pub async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(TICK).await;
    }
}

/// The result payload of a successful reply, or a panic naming the error.
pub fn result_of(response: &Response) -> &Value {
    match &response.outcome {
        amx_proto::RpcOutcome::Result(value) => value,
        amx_proto::RpcOutcome::Error(err) => panic!("call failed: {} ({})", err.message, err.code),
    }
}
