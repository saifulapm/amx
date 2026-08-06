//! A raw protocol client over the session socket.
//!
//! It speaks frames rather than a client API — the same choice the T10
//! harness made — because the suites here need to do things no well-behaved
//! client would: offer a version window from the future, declare a payload
//! and send half of it, call a method that does not exist. The higher-level
//! helpers at the bottom drive the real verb surface with it.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use amx_core::{PaneId, WorkspaceId};
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN};
use amx_proto::{ClientInfo, Feature, FrameHeader, Hello, Request, RequestId, Response, Welcome};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// How long a read waits for the server before the test fails.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// A connected raw client.
#[derive(Debug)]
pub struct Wire {
    stream: UnixStream,
    next_id: u64,
}

impl Wire {
    /// Connect to the session socket.
    pub async fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).await.expect("connect");
        Self { stream, next_id: 1 }
    }

    /// Do the handshake, offering `proto` and this build's feature set.
    pub async fn hello(&mut self, proto: (u16, u16)) -> Welcome {
        let hello = Hello {
            proto,
            features: BTreeSet::from([Feature::GRID_STREAM, Feature::HISTORY_RANGES]),
            client: ClientInfo {
                name: "amx-rig".to_owned(),
                version: "0.0.0".to_owned(),
                term: None,
            },
            resume: None,
        };
        self.hello_value(&serde_json::to_value(&hello).expect("encode hello"))
            .await
    }

    /// Do the handshake from an arbitrary JSON value, for skew probes that
    /// carry unknown features or fields no current `Hello` can express.
    pub async fn hello_value(&mut self, hello: &Value) -> Welcome {
        self.send_control(&serde_json::to_vec(hello).expect("encode hello"))
            .await;
        let (header, payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "the welcome must be a control frame");
        serde_json::from_slice(&payload).expect("decode welcome")
    }

    /// Make one JSON-RPC call and read its reply.
    pub async fn request(&mut self, method: &str, params: Value) -> Response {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(id, method, params).await;
        self.read_response().await
    }

    /// Send a call without waiting for its reply.
    pub async fn send_request(&mut self, id: u64, method: &str, params: Value) {
        let request = Request::new(RequestId::Number(id), method, Some(params));
        self.send_control(&serde_json::to_vec(&request).expect("encode request"))
            .await;
    }

    /// Read one control frame and decode it as a response.
    pub async fn read_response(&mut self) -> Response {
        let (header, payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "a reply must be a control frame");
        serde_json::from_slice(&payload).expect("decode response")
    }

    /// Send a control frame carrying `payload`.
    pub async fn send_control(&mut self, payload: &[u8]) {
        let header = FrameHeader::new(payload.len() as u32, CONTROL_CHANNEL);
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write the header");
        self.stream
            .write_all(payload)
            .await
            .expect("write the payload");
    }

    /// Send a bare header, with `follow` bytes of a payload it declares more of.
    pub async fn send_partial_frame(&mut self, declared: u32, follow: &[u8]) {
        assert!((follow.len() as u32) < declared, "a partial frame is short");
        let header = FrameHeader::new(declared, CONTROL_CHANNEL);
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write the header");
        self.stream
            .write_all(follow)
            .await
            .expect("write the partial payload");
    }

    /// Send bytes with no framing at all — half a header, if that's the test.
    pub async fn send_bytes(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("write raw bytes");
    }

    /// Drop the connection without ceremony.
    pub fn abandon(self) {
        drop(self);
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

/// Read one whole frame, failing the test if none arrives in time.
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

/// The result payload of a successful reply, or a panic naming the error.
pub fn result_of(response: &Response) -> &Value {
    match &response.outcome {
        amx_proto::RpcOutcome::Result(value) => value,
        amx_proto::RpcOutcome::Error(err) => panic!("call failed: {} ({})", err.message, err.code),
    }
}

/// The error of a failed reply, or a panic quoting the success.
pub fn error_of(response: &Response) -> &amx_proto::RpcError {
    match &response.outcome {
        amx_proto::RpcOutcome::Error(err) => err,
        amx_proto::RpcOutcome::Result(value) => panic!("call unexpectedly succeeded: {value}"),
    }
}

/// Create a focused workspace and learn its root pane's id.
///
/// The only wire route to a pane id today: `workspace.create` replies with the
/// workspace alone, and `workspace.switch` replies with the focused pane —
/// which, for a fresh workspace, is its root.
pub async fn workspace_with_root(wire: &mut Wire) -> (WorkspaceId, PaneId) {
    let created = wire
        .request("workspace.create", json!({ "focus": true }))
        .await;
    let workspace: WorkspaceId =
        serde_json::from_value(result_of(&created)["workspace"].clone()).expect("a workspace id");
    let switched = wire
        .request("workspace.switch", json!({ "workspace": workspace }))
        .await;
    let root: PaneId = serde_json::from_value(result_of(&switched)["focused_pane"].clone())
        .expect("a fresh workspace has a focused root pane");
    (workspace, root)
}

/// Split `pane`, running `command` in the new pane, and return the new pane.
pub async fn split_running(wire: &mut Wire, pane: PaneId, command: &[&str]) -> PaneId {
    let reply = wire
        .request(
            "pane.split",
            json!({
                "pane": pane,
                "direction": "vertical",
                "command": command,
                "cwd": "/",
            }),
        )
        .await;
    serde_json::from_value(result_of(&reply)["pane"].clone()).expect("the new pane's id")
}
