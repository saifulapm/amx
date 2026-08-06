//! T10: the socket surface — accept, negotiate, decode, dispatch, and a writer
//! with strict channel priority (04 §1, §4).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use amx_core::{Bus, Ctx, Scheduled, SessionName};
use amx_proto::control::session;
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, MAX_CONTROL_FRAME};
use amx_proto::stream::Priority;
use amx_proto::version::{COMPAT_WINDOW, PROTO_MAX, PROTO_MIN, window};
use amx_proto::{ClientInfo, Feature, FrameHeader, Hello, Request, RequestId, Response, Welcome};
use amx_server::actor::CoreHandle;
use amx_server::actor::core::Core;
use amx_server::actor::gateway::{
    Gateway, GatewayError, GatewayProbe, GatewayReport, RUNTIME_DIR_MODE, SOCKET_MODE,
};
use amx_server::conn::writer::{self, OutFrame};
use amx_server::dispatch::NOT_IMPLEMENTED;
use amx_server::runtime::{Runtime, ShutdownReport};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// How long a test waits for the server to do something.
const PATIENCE: Duration = Duration::from_secs(5);

/// The payload length the oversized frame declares: 8 MiB, eight times the cap.
const OVERSIZED: u32 = 8 * 1024 * 1024;

/// Single allocations at or above this size are counted.
///
/// Any buffer sized from [`OVERSIZED`] would be at least 8 MiB and so would be
/// counted; nothing else this test binary does allocates a single block of 4
/// MiB, so the count is attributable.
const WATCHED_ALLOC: usize = 4 * 1024 * 1024;

// ------------------------------------------------------- allocation counting

/// Counts single allocations of [`WATCHED_ALLOC`] bytes or more.
struct WatchingAlloc;

/// How many watched allocations have happened.
static BIG_ALLOCS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the system allocator with the layout it was
// given and returns its pointer unchanged; the counter is the only addition.
unsafe impl GlobalAlloc for WatchingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= WATCHED_ALLOC {
            BIG_ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= WATCHED_ALLOC {
            BIG_ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= WATCHED_ALLOC {
            BIG_ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: WatchingAlloc = WatchingAlloc;

// --------------------------------------------------------------- scaffolding

/// A directory under `$TMPDIR`, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("amx-t10-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    /// Where it is.
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A session: a `Core` actor and a `Gateway`, both under one `Runtime`.
struct Server {
    ctx: Ctx,
    probe: GatewayProbe,
    runtime: Runtime,
    report: oneshot::Receiver<GatewayReport>,
    _dir: TempDir,
}

impl Server {
    /// Bind a session socket under a fresh temp directory and start serving.
    async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        let mut runtime = Runtime::new(ctx.clone());

        let (core_tx, core_rx) = mpsc::channel(64);
        runtime.spawn(Core::new(ctx.clone()).run(core_rx, |_: &Scheduled| {}));

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
    fn socket(&self) -> &Path {
        &self.ctx.socket
    }

    /// Connect a client to it.
    async fn connect(&self) -> Client {
        Client {
            stream: UnixStream::connect(self.socket()).await.expect("connect"),
        }
    }

    /// Cancel the session and join every task.
    async fn shutdown(self) -> (GatewayReport, ShutdownReport) {
        let shutdown = self.runtime.shutdown().await;
        let gateway = self.report.await.expect("the gateway reported");
        (gateway, shutdown)
    }
}

/// A `Ctx` whose paths all live under `root`, with a private bus and token.
fn ctx_under(root: &Path) -> Ctx {
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

/// A raw client: it speaks frames, so a test can send malformed ones.
struct Client {
    stream: UnixStream,
}

impl Client {
    /// Send a control frame carrying `payload`.
    async fn send_control(&mut self, payload: &[u8]) {
        let header = FrameHeader::new(payload.len() as u32, CONTROL_CHANNEL);
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write");
        self.stream.write_all(payload).await.expect("write");
    }

    /// Send a bare header, with no payload behind it.
    async fn send_header(&mut self, header: FrameHeader) {
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write");
    }

    /// Send bytes with no framing at all.
    async fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("write");
    }

    /// Do the handshake, offering `proto` plus one feature no server has.
    async fn hello(&mut self, proto: (u16, u16)) -> Welcome {
        let hello = Hello {
            proto,
            features: BTreeSet::from([
                Feature::GRID_STREAM,
                Feature::named("amx.test.feature-no-server-has"),
            ]),
            client: ClientInfo {
                name: "amx-test".to_owned(),
                version: "0.0.0".to_owned(),
                term: None,
            },
            resume: None,
        };
        self.send_control(&serde_json::to_vec(&hello).expect("encode hello"))
            .await;
        let (header, payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "the welcome must be a control frame");
        serde_json::from_slice(&payload).expect("decode welcome")
    }

    /// Make one JSON-RPC call and read its reply.
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Response {
        let request = Request::new(RequestId::Number(id), method, Some(params));
        self.send_control(&serde_json::to_vec(&request).expect("encode request"))
            .await;
        let (header, payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "a reply must be a control frame");
        serde_json::from_slice(&payload).expect("decode response")
    }

    /// Read until the server closes. `true` if it did so within [`PATIENCE`].
    async fn closed_by_server(&mut self) -> bool {
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
async fn read_frame<R: AsyncRead + Unpin>(source: &mut R) -> (FrameHeader, Vec<u8>) {
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
async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// The result payload of a successful reply, or a panic naming the error.
fn result_of(response: &Response) -> &Value {
    match &response.outcome {
        amx_proto::RpcOutcome::Result(value) => value,
        amx_proto::RpcOutcome::Error(err) => panic!("call failed: {} ({})", err.message, err.code),
    }
}

// -------------------------------------------------------------- the socket

#[tokio::test]
async fn socket_is_created_mode_0600() {
    let server = Server::start("mode").await;

    let socket = std::fs::metadata(server.socket()).expect("the socket exists");
    assert_eq!(
        socket.permissions().mode() & 0o777,
        SOCKET_MODE,
        "the session socket must be owner-only"
    );

    let dir = std::fs::metadata(&server.ctx.runtime_dir).expect("the runtime dir exists");
    assert_eq!(
        dir.permissions().mode() & 0o777,
        RUNTIME_DIR_MODE,
        "the socket's directory must be owner-only too, so the window between \
         bind and chmod is not reachable by another user"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn bind_refuses_a_socket_a_live_server_answers_on() {
    let server = Server::start("occupied").await;

    let (spare_tx, _spare_rx) = mpsc::channel(1);
    let err = Gateway::bind(server.ctx.clone(), CoreHandle::new(spare_tx))
        .expect_err("a second gateway must not steal a live session's socket");
    assert!(matches!(err, GatewayError::AlreadyRunning { .. }), "{err}");

    server.shutdown().await;
}

#[tokio::test]
async fn bind_replaces_a_socket_no_server_answers_on() {
    let dir = TempDir::new("stale");
    let ctx = ctx_under(dir.path());
    std::fs::create_dir_all(&ctx.runtime_dir).expect("create the runtime dir");
    // A socket file whose server is gone: binding and dropping the listener
    // leaves exactly the file a killed server would have left behind.
    drop(std::os::unix::net::UnixListener::bind(&ctx.socket).expect("leave a stale socket"));
    assert!(ctx.socket.exists());

    let (tx, _rx) = mpsc::channel(1);
    let gateway = Gateway::bind(ctx.clone(), CoreHandle::new(tx))
        .expect("a stale socket must be replaced, not refused");
    assert_eq!(gateway.socket(), ctx.socket);
}

// ---------------------------------------------------------- the handshake

#[tokio::test]
async fn hello_welcome_round_trip_over_the_real_socket() {
    let server = Server::start("hello").await;
    let mut client = server.connect().await;

    let welcome = client.hello(window()).await;
    assert_eq!(welcome.proto, PROTO_MAX);
    assert!(
        welcome.features.contains(&Feature::GRID_STREAM),
        "a feature both sides have must survive the intersection"
    );
    assert!(
        !welcome
            .features
            .contains(&Feature::named("amx.test.feature-no-server-has")),
        "a feature only the client has must be dropped, not rejected"
    );

    // The identity in the welcome is the identity every later reply carries:
    // the welcome is built from the same `ping` the client can call itself.
    let reply = client.request(1, "ping", json!({})).await;
    let ping: session::PingReply =
        serde_json::from_value(result_of(&reply).clone()).expect("a ping reply");
    assert_eq!(ping.session, welcome.session);
    assert!(ping.seq >= welcome.seq, "the bus sequence never goes back");

    server.shutdown().await;
}

#[tokio::test]
async fn server_accepts_previous_protocol_version() {
    let server = Server::start("skew").await;

    // The oldest version inside the compatibility window. While only one
    // version exists this is current-against-current, as the plan states.
    let oldest = PROTO_MAX.saturating_sub(COMPAT_WINDOW - 1).max(PROTO_MIN);
    let mut old = server.connect().await;
    let welcome = old.hello((oldest, PROTO_MAX)).await;
    assert_eq!(
        welcome.proto, PROTO_MAX,
        "the highest version both sides speak wins"
    );

    // A client newer than this build offers a window running past PROTO_MAX;
    // it must be answered with PROTO_MAX rather than refused.
    let mut new = server.connect().await;
    let welcome = new.hello((PROTO_MIN, PROTO_MAX + 1)).await;
    assert_eq!(welcome.proto, PROTO_MAX);

    server.shutdown().await;
}

#[tokio::test]
async fn a_first_frame_that_is_not_a_hello_closes_the_connection() {
    let server = Server::start("nohello").await;
    let mut client = server.connect().await;

    client
        .send_control(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
        .await;
    assert!(
        client.closed_by_server().await,
        "nothing is dispatchable before a version is agreed"
    );

    wait_until("the connection task is gone", || server.probe.live() == 0).await;
    server.shutdown().await;
}

// ------------------------------------------------------------- dispatch

#[tokio::test]
async fn a_control_call_reaches_the_core_and_comes_back() {
    let server = Server::start("dispatch").await;
    let mut client = server.connect().await;
    client.hello(window()).await;

    let reply = client
        .request(7, "workspace.create", json!({ "label": "work" }))
        .await;
    let created: amx_proto::control::workspace::CreateReply =
        serde_json::from_value(result_of(&reply).clone()).expect("a create reply");
    assert_eq!(created.short.get(), 1);
    assert_eq!(reply.id, RequestId::Number(7));

    server.shutdown().await;
}

#[tokio::test]
async fn an_unknown_method_is_a_reply_not_a_disconnect() {
    let server = Server::start("skewmethod").await;
    let mut client = server.connect().await;
    client.hello(window()).await;

    let reply = client.request(1, "pane.teleport", json!({})).await;
    let amx_proto::RpcOutcome::Error(err) = &reply.outcome else {
        panic!("an unknown method must fail");
    };
    assert_eq!(err.code, amx_proto::RpcError::METHOD_NOT_FOUND);

    // The connection is still usable, which is the whole point of answering
    // rather than dropping: a peer built against another revision of the
    // method table keeps its session.
    let reply = client.request(2, "ping", json!({})).await;
    assert!(matches!(reply.outcome, amx_proto::RpcOutcome::Result(_)));

    server.shutdown().await;
}

#[tokio::test]
async fn a_method_with_no_handler_yet_reports_a_seam_not_a_missing_method() {
    let server = Server::start("seam").await;
    let mut client = server.connect().await;
    client.hello(window()).await;

    let reply = client
        .request(1, "pane.close", json!({ "pane": uuid_text() }))
        .await;
    let amx_proto::RpcOutcome::Error(err) = &reply.outcome else {
        panic!("pane.close has no handler yet");
    };
    assert_eq!(
        err.code, NOT_IMPLEMENTED,
        "a method in the shared table must not be reported as unknown — that \
         would tell the client to stop offering it"
    );

    server.shutdown().await;
}

/// A syntactically valid pane id, for calls whose params must parse.
fn uuid_text() -> String {
    amx_core::PaneId::new_v4().to_string()
}

// ----------------------------------------------------------- writer priority

/// The channel a history transfer is bound to in these tests.
const HISTORY_CHANNEL: u8 = 7;
/// One history chunk.
const CHUNK: usize = 64 * 1024;
/// How many chunks the transfer is.
const CHUNKS: usize = 64;

#[tokio::test]
async fn control_reply_overtakes_a_queued_history_transfer() {
    let (mut client, server_side) = UnixStream::pair().expect("a socket pair");
    let (out, queue) = writer::channel();
    let cancel = CancellationToken::new();

    // A chunked scrollback fetch: 4 MiB, entirely queued.
    for i in 0..CHUNKS {
        let mut chunk = vec![0_u8; CHUNK];
        chunk[0] = i as u8;
        out.send(
            OutFrame::stream(HISTORY_CHANNEL, Priority::Bulk, CHUNK as u32, chunk)
                .expect("a bulk frame"),
        )
        .expect("queue a chunk");
    }
    assert_eq!(out.depth(Priority::Bulk), CHUNKS);

    // Then the keystroke's reply, queued dead last.
    out.send(OutFrame::control(br#"{"jsonrpc":"2.0","id":1}"#.to_vec()).expect("a control frame"))
        .expect("queue the reply");

    // Nothing above awaited, so the writer has not yet seen any of it: what
    // follows is decided purely by priority, not by who got there first.
    let writing = tokio::spawn(writer::run(server_side, queue, cancel.clone()));

    let (header, payload) = read_frame(&mut client).await;
    assert!(
        header.is_control(),
        "the control reply must overtake every queued history chunk, not wait \
         behind 4 MiB of them"
    );
    assert_eq!(payload, br#"{"jsonrpc":"2.0","id":1}"#);

    // And the transfer still arrives, in order, behind it.
    for i in 0..CHUNKS {
        let (header, payload) = read_frame(&mut client).await;
        assert_eq!(header.channel, HISTORY_CHANNEL);
        assert_eq!(payload[0], i as u8, "chunk {i} arrived out of order");
    }

    cancel.cancel();
    let _ = writing.await;
}

#[test]
fn the_writer_queue_is_strictly_ordered_control_then_grid_then_bulk() {
    let (out, _queue) = writer::channel();
    out.send(OutFrame::stream(3, Priority::Bulk, 16, vec![b'b']).unwrap())
        .unwrap();
    out.send(OutFrame::stream(4, Priority::Grid, 16, vec![b'g']).unwrap())
        .unwrap();
    out.send(OutFrame::control(vec![b'c']).unwrap()).unwrap();

    assert_eq!(out.depth(Priority::Control), 1);
    assert_eq!(out.depth(Priority::Grid), 1);
    assert_eq!(out.depth(Priority::Bulk), 1);
}

#[test]
fn an_outgoing_control_reply_over_the_cap_is_refused_at_construction() {
    let err = OutFrame::control(vec![0_u8; MAX_CONTROL_FRAME + 1])
        .expect_err("the cap binds outgoing frames too");
    assert!(
        matches!(err, writer::OutboundError::TooLarge { cap, .. } if cap == MAX_CONTROL_FRAME),
        "{err}"
    );
}

// ------------------------------------------------------------ hostile peers

#[tokio::test]
async fn oversized_frame_closes_the_connection_without_allocating() {
    let server = Server::start("oversized").await;
    let mut client = server.connect().await;
    client.hello(window()).await;

    let before = BIG_ALLOCS.load(Ordering::Relaxed);
    // A header alone, declaring eight times the control cap. No payload is
    // sent, and none may be waited for: the length is refused from the header.
    client
        .send_header(FrameHeader::new(OVERSIZED, CONTROL_CHANNEL))
        .await;

    assert!(
        client.closed_by_server().await,
        "an oversized declaration must end the connection"
    );
    wait_until("the connection task is gone", || server.probe.live() == 0).await;

    assert_eq!(
        BIG_ALLOCS.load(Ordering::Relaxed),
        before,
        "no buffer was sized from the declared length: rejecting it costs a \
         rejection, not {OVERSIZED} bytes"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn client_disconnect_mid_frame_does_not_leak_a_task() {
    let server = Server::start("torn").await;
    {
        let mut client = server.connect().await;
        client.hello(window()).await;
        // Announce a 4 KiB control frame, send four bytes of it, and vanish.
        client
            .send_header(FrameHeader::new(4096, CONTROL_CHANNEL))
            .await;
        client.send_raw(br#"{"id"#).await;
    }

    wait_until("the connection task is gone", || server.probe.live() == 0).await;
    assert_eq!(server.probe.accepted(), 1);

    let (gateway, shutdown) = server.shutdown().await;
    assert_eq!(gateway.accepted, 1);
    assert_eq!(
        gateway.joined, 1,
        "the torn connection's task must be joined, not merely finished"
    );
    assert_eq!(gateway.panicked, 0);
    assert!(gateway.clean(), "{gateway:?}");
    assert_eq!(
        shutdown.joined, 2,
        "the core and the gateway are the runtime's only tasks, and both are joined"
    );
    assert!(shutdown.clean());
}

#[tokio::test]
async fn every_connection_task_is_joined_at_shutdown() {
    let server = Server::start("joinall").await;
    let mut clients = Vec::new();
    for _ in 0..4 {
        let mut client = server.connect().await;
        client.hello(window()).await;
        clients.push(client);
    }
    wait_until("every client is accounted for", || server.probe.live() == 4).await;

    // Shut down with all four still connected: cancellation, not disconnect,
    // is what has to stop them.
    let (gateway, shutdown) = server.shutdown().await;
    assert_eq!(gateway.accepted, 4);
    assert_eq!(gateway.joined, 4);
    assert!(gateway.clean(), "{gateway:?}");
    assert!(shutdown.clean());
}
