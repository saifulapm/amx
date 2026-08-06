//! T10: the session socket and the connection tasks behind it (04 §1).
//!
//! One socket per session at mode 0600, stale-socket disambiguation by connect
//! probe, and a `JoinSet` that accounts for every connection task — including
//! the ones a hostile or vanishing peer ends.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

use amx_proto::FrameHeader;
use amx_proto::frame::CONTROL_CHANNEL;
use amx_server::actor::CoreHandle;
use amx_server::actor::gateway::{Gateway, GatewayError, RUNTIME_DIR_MODE, SOCKET_MODE};
use tokio::sync::mpsc;

mod support;

use support::{Server, TempDir, ctx_under, wait_until};

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

#[tokio::test]
async fn bind_never_clobbers_a_path_it_cannot_prove_stale() {
    // A dangling symlink probes `Absent` (connect follows it to nothing) yet
    // still refuses the bind with `AddrInUse` — the same shape as losing the
    // bind race to a server mid-`bind(2)`. The gateway must not have removed
    // what it could not prove stale, and must re-probe before reporting, so a
    // racer that finished listening in the window is answered
    // `AlreadyRunning` rather than a bare bind failure.
    let dir = TempDir::new("dangling");
    let ctx = ctx_under(dir.path());
    std::fs::create_dir_all(&ctx.runtime_dir).expect("create the runtime dir");
    std::os::unix::fs::symlink(dir.path().join("nothing-here"), &ctx.socket)
        .expect("plant the occupant");

    let (tx, _rx) = mpsc::channel(1);
    let err = Gateway::bind(ctx.clone(), CoreHandle::new(tx))
        .expect_err("the occupied path must refuse the bind");
    assert!(matches!(err, GatewayError::Bind { .. }), "{err}");
    assert!(
        std::fs::symlink_metadata(&ctx.socket).is_ok(),
        "the occupant was not ours to remove"
    );
}

// ------------------------------------------------------------ hostile peers

#[tokio::test]
async fn oversized_frame_closes_the_connection_without_allocating() {
    let server = Server::start("oversized").await;
    let mut client = server.attach().await;

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
        let mut client = server.attach().await;
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

#[tokio::test]
async fn every_connection_task_is_joined_at_shutdown() {
    let server = Server::start("joinall").await;
    let mut clients = Vec::new();
    for _ in 0..4 {
        clients.push(server.attach().await);
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
