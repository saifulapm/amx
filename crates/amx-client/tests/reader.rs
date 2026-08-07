//! The session reader's cancel-safety contract.
//!
//! [`Session::read_frame_into`] is one arm of the client loop's `select!`
//! (`app::wired::App::run`, and `amx viewport`'s loop next to it), so any other
//! arm winning a race — a keystroke, a `SIGWINCH`, the resize debounce — drops
//! the read future wherever it had got to. A reader that keeps its progress on
//! that future's stack loses whatever it had already taken off the socket, and
//! the *next* read starts part way through a payload: it reads payload bytes as
//! a frame header and the session dies on the first thing that byte is not.
//!
//! Both tests here are that drop, made deterministic with `biased;` — the read
//! arm is polled first and takes what the peer sent, then a ready future wins
//! and the read is cancelled. The payload is `b'o'` (111) throughout, so a
//! reader that resyncs mid-frame produces exactly the refusal the adversarial
//! suite saw: `frame on unbound channel 111`.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_client::net::Session;
use amx_core::SessionId;
use amx_proto::frame::{CONTROL_CHANNEL, DEFAULT_STREAM_FRAME, FRAME_HEADER_LEN};
use amx_proto::{ClientInfo, FrameHeader, Hello, ServerInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The channel the tests bind and stream on.
const CHANNEL: u8 = 7;

/// A payload big enough that no socket hands it over in one read, and made of
/// the byte whose value is the channel the failure was reported on.
fn payload() -> Vec<u8> {
    vec![b'o'; 64 * 1024]
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-reader-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// Read one whole frame from the peer's end.
async fn read_frame(peer: &mut UnixStream) -> (FrameHeader, Vec<u8>) {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    peer.read_exact(&mut header).await.expect("a frame header");
    let header = FrameHeader::decode(header).expect("the header decodes");
    let mut payload = vec![0_u8; header.payload_len()];
    peer.read_exact(&mut payload).await.expect("the payload");
    (header, payload)
}

/// Write one whole frame from the peer's end.
async fn write_frame(peer: &mut UnixStream, channel: u8, payload: &[u8]) {
    let len = u32::try_from(payload.len()).expect("a frame that fits");
    peer.write_all(&FrameHeader::new(len, channel).encode())
        .await
        .expect("write the header");
    peer.write_all(payload).await.expect("write the payload");
}

/// A negotiated `Session` and the socket end a hand-written peer speaks on.
///
/// The peer is this test rather than a real server because these tests are
/// about *when* bytes arrive, which only a peer that writes half a frame and
/// stops can state.
async fn attached() -> (Session, UnixStream) {
    let (client, mut peer) = UnixStream::pair().expect("a socket pair");
    let attaching =
        tokio::spawn(async move { Session::attach(client, client_info(), true, None).await });

    let (header, payload) = read_frame(&mut peer).await;
    assert_eq!(
        header.channel, CONTROL_CHANNEL,
        "the hello is a control frame"
    );
    let hello: Hello = serde_json::from_slice(&payload).expect("decode the hello");
    let welcome = hello
        .accept(
            ServerInfo {
                name: "amx-reader-test".to_owned(),
                version: "0.0.0".to_owned(),
            },
            &hello.features,
            0,
            SessionId::new_v4(),
        )
        .expect("negotiate a welcome");
    let encoded = serde_json::to_vec(&welcome).expect("encode the welcome");
    write_frame(&mut peer, CONTROL_CHANNEL, &encoded).await;

    let (session, _welcome) = attaching
        .await
        .expect("the attach task")
        .expect("the handshake");
    (session, peer)
}

/// Poll one read, let another arm win, and report whether the read was
/// cancelled part way through.
async fn cancel_a_read(session: &mut Session, buf: &mut Vec<u8>) -> bool {
    tokio::select! {
        biased;
        _ = session.read_frame_into(buf) => false,
        () = std::future::ready(()) => true,
    }
}

#[tokio::test]
async fn a_read_cancelled_inside_the_payload_resumes_the_frame() {
    let (mut session, mut peer) = attached().await;
    session.bind_channel(CHANNEL, DEFAULT_STREAM_FRAME);
    let payload = payload();

    // The header and a first slice of the payload; the rest stays with the
    // peer, so the read cannot finish however long it is polled.
    let len = u32::try_from(payload.len()).expect("a frame that fits");
    peer.write_all(&FrameHeader::new(len, CHANNEL).encode())
        .await
        .expect("write the header");
    peer.write_all(&payload[..1024])
        .await
        .expect("write the first slice");

    let mut buf = Vec::new();
    assert!(
        cancel_a_read(&mut session, &mut buf).await,
        "the peer sent 1024 of {} payload bytes, so the read cannot complete",
        payload.len()
    );

    // The rest of the frame arrives, and nothing else does.
    peer.write_all(&payload[1024..])
        .await
        .expect("write the rest");

    let header = session
        .read_frame_into(&mut buf)
        .await
        .expect("the cancelled frame is resumed, not resynced");
    assert_eq!(header.channel, CHANNEL, "the frame keeps its channel");
    assert_eq!(buf, payload, "the frame keeps every byte of its payload");
}

#[tokio::test]
async fn a_read_cancelled_inside_the_header_resumes_the_frame() {
    let (mut session, mut peer) = attached().await;
    session.bind_channel(CHANNEL, DEFAULT_STREAM_FRAME);
    let payload = payload();

    // Cancelled with the header itself half-read: the other resume point.
    let len = u32::try_from(payload.len()).expect("a frame that fits");
    let header = FrameHeader::new(len, CHANNEL).encode();
    peer.write_all(&header[..3])
        .await
        .expect("write part of the header");

    let mut buf = Vec::new();
    assert!(
        cancel_a_read(&mut session, &mut buf).await,
        "the peer sent 3 of {FRAME_HEADER_LEN} header bytes, so the read cannot complete"
    );

    peer.write_all(&header[3..])
        .await
        .expect("write the rest of the header");
    peer.write_all(&payload).await.expect("write the payload");

    let header = session
        .read_frame_into(&mut buf)
        .await
        .expect("the cancelled frame is resumed, not resynced");
    assert_eq!(header.channel, CHANNEL, "the frame keeps its channel");
    assert_eq!(buf, payload, "the frame keeps every byte of its payload");
}
