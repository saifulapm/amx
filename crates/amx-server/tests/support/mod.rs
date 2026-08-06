//! Shared scaffolding for the T10 socket tests: a session on a real socket and
//! a client that speaks frames rather than a client API, so a test can send
//! malformed ones.

#![allow(dead_code, reason = "each test binary uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use amx_core::{Bus, Ctx, Scheduled, SessionName};
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN};
use amx_proto::{ClientInfo, Feature, FrameHeader, Hello, Request, RequestId, Response, Welcome};
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
        let path = std::env::temp_dir().join(format!("amx-t10-{tag}-{}-{n}", std::process::id()));
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
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        let mut runtime = Runtime::new(ctx.clone());

        let (core_tx, core_rx) = mpsc::channel(64);
        let core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
        runtime.spawn(async move {
            let _ = core.run(core_rx, |_: &Scheduled| {}).await;
        });

        let gateway =
            Gateway::bind(ctx.clone(), CoreHandle::new(core_tx)).expect("bind the session socket");
        let probe = gateway.probe().clone();
        let (report_tx, report) = oneshot::channel();
        runtime.spawn(async move {
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

    /// Cancel the session and join every task.
    pub async fn shutdown(self) -> (GatewayReport, ShutdownReport) {
        let shutdown = self.runtime.shutdown().await;
        let gateway = self.report.await.expect("the gateway reported");
        (gateway, shutdown)
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

    async fn hello_with(&mut self, proto: (u16, u16), attach: bool) -> Welcome {
        let hello = Hello {
            proto,
            features: BTreeSet::from([Feature::GRID_STREAM, Feature::named(UNKNOWN_FEATURE)]),
            client: ClientInfo {
                name: "amx-test".to_owned(),
                version: "0.0.0".to_owned(),
                term: None,
            },
            attach,
            resume: None,
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
