//! The frame reader survives what `select!` does to it.
//!
//! The wired client's run loop races its frame read against stdin, `SIGWINCH`
//! and the resize debounce; whenever another branch wins, the in-flight read
//! future is dropped. A reader that keeps half-consumed bytes in that future
//! desyncs the whole stream — the next header parse starts mid-frame, and the
//! client dies on "control frame is not JSON" the moment traffic and input
//! coincide. This suite drives `Session` over a real socketpair against a
//! scripted peer and drops a read future mid-frame on purpose.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::future::Future as _;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use amx_client::net::Session;
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN};
use amx_proto::{ClientInfo, FrameHeader, Hello, ServerInfo};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;

/// One control frame, wire-encoded.
fn control_frame(payload: &[u8]) -> Vec<u8> {
    let header = FrameHeader::new(
        u32::try_from(payload.len()).expect("test payloads are small"),
        CONTROL_CHANNEL,
    );
    let mut wire = header.encode().to_vec();
    wire.extend_from_slice(payload);
    wire
}

/// Complete the `Hello`/`Welcome` exchange as the peer of `attach`.
async fn welcome_the_client(peer: &mut UnixStream) {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    peer.read_exact(&mut header).await.expect("hello header");
    let header = FrameHeader::decode(header).expect("hello header decodes");
    let mut payload = vec![0_u8; header.payload_len()];
    peer.read_exact(&mut payload).await.expect("hello payload");
    let hello: Hello = serde_json::from_slice(&payload).expect("hello is JSON");

    let welcome = hello
        .accept(
            ServerInfo {
                name: "scripted-peer".to_owned(),
                version: "0.0.0".to_owned(),
            },
            &hello.features.clone(),
            0,
            amx_core::SessionId::new_v4(),
        )
        .expect("the windows overlap");
    let frame = control_frame(&serde_json::to_vec(&welcome).expect("encode welcome"));
    peer.write_all(&frame).await.expect("write the welcome");
}

#[tokio::test]
async fn a_read_dropped_mid_frame_resumes_at_the_same_byte() {
    let (client, mut peer) = UnixStream::pair().expect("socketpair");
    let client_info = ClientInfo {
        name: "frames-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    };
    let (attached, ()) = tokio::join!(
        Session::attach(client, client_info, false, None),
        welcome_the_client(&mut peer),
    );
    let (mut session, _welcome) = attached.expect("attach against the scripted peer");

    // Three bytes of a frame's five-byte header arrive, a read consumes them,
    // and then that read is dropped — exactly what the run loop's `select!`
    // does when stdin or a signal wins the race.
    let first = control_frame(b"one");
    peer.write_all(&first[..3]).await.expect("dribble a header");
    let mut buf = Vec::new();
    {
        let mut read = pin!(session.read_frame_into(&mut buf));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(
            matches!(read.as_mut().poll(&mut cx), Poll::Pending),
            "three header bytes are not a frame"
        );
    }

    // The rest of that frame and a whole second one: both must come out
    // intact, which they cannot if the dropped read swallowed the dribble.
    peer.write_all(&first[3..]).await.expect("finish the frame");
    peer.write_all(&control_frame(b"two"))
        .await
        .expect("a second frame");

    let header = session.read_frame_into(&mut buf).await.expect("frame one");
    assert!(header.is_control());
    assert_eq!(buf, b"one");
    let header = session.read_frame_into(&mut buf).await.expect("frame two");
    assert!(header.is_control());
    assert_eq!(buf, b"two");
}
