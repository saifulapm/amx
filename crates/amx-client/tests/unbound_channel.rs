//! DR-12: what a client does with a frame nobody here has a route for.
//!
//! The register left the question open — "the client's current handling of an
//! unknown channel appears to be a silent `Applied::Nothing`; decide
//! deliberately whether refusal or silence is intended" — and noted that the
//! race it came from (3 rounds in 30 under flood) had no named test. Both
//! halves are here.
//!
//! The decision, stated in `amx_client::stream`'s module header, is that the
//! two layers answer differently because only one of them can tell the two
//! cases apart:
//!
//! - the **reader** refuses a channel this connection never bound. That is the
//!   check that caught a desynchronised peer once
//!   (`docs/notes/frame-read-cancellation.md`), and it is fatal by design —
//!   [`NetError::UnboundChannel`] is not a transport failure, so no redial can
//!   swallow it;
//! - the **routing table** drops a frame on a channel it has no route for. By
//!   then the reader has already vouched that this connection bound it, so what
//!   is left is a binding this client let go of — a defined protocol condition,
//!   not a broken session.
//!
//! The flood half is the shape the failure actually had: it was never a
//! desynchronised server, it was the client's own read being cancelled mid
//! frame and resuming out of step, so the payload byte `b'o'` (111) was read as
//! a channel number. `flooding_reads_cancelled_every_time_never_report_an_unbound_channel`
//! is that, run to a count rather than to luck.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_client::cache::Scrollback;
use amx_client::model::ClientModel;
use amx_client::net::{NetError, Session, Torn};
use amx_client::stream::{Applied, Bindings, apply};
use amx_core::{PaneId, SessionId};
use amx_proto::frame::{CONTROL_CHANNEL, DEFAULT_STREAM_FRAME, FRAME_HEADER_LEN};
use amx_proto::stream::grid::{Cursor, CursorShape, GridMessage};
use amx_proto::{ClientInfo, FrameHeader, Hello, ServerInfo};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The channel these tests bind and stream on.
const CHANNEL: u8 = 7;
/// A channel nothing ever binds — the byte the original refusal named.
const STRANGER: u8 = 111;

fn cursor() -> Cursor {
    Cursor {
        row: 3,
        col: 4,
        visible: true,
        shape: CursorShape::Block,
        blink: false,
    }
}

/// A decodable grid message, so a dropped frame is dropped for its channel and
/// never for its bytes.
fn cursor_frame() -> Vec<u8> {
    let mut payload = Vec::new();
    GridMessage::Cursor(cursor()).encode(&mut payload);
    payload
}

/// The bytes tag 2 used to carry: a row range and a hash count.
///
/// DR-7 retired the scroll notice; nothing on the wire means this any more.
fn retired_scroll_notice() -> Vec<u8> {
    let mut payload = vec![2];
    payload.extend_from_slice(&40_u64.to_le_bytes());
    payload.extend_from_slice(&41_u64.to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload
}

fn header(payload: &[u8], channel: u8) -> FrameHeader {
    FrameHeader::new(
        u32::try_from(payload.len()).expect("a frame that fits"),
        channel,
    )
}

// ---------------------------------------------------------------- the table

#[test]
fn a_frame_on_a_channel_the_table_has_no_route_for_is_dropped() {
    let pane = PaneId::new_v4();
    let mut model = ClientModel::new(24, 80);
    let mut caches: HashMap<PaneId, Scrollback> = HashMap::new();
    let mut bindings = Bindings::new();
    bindings.bind_grid(pane, CHANNEL);

    let payload = cursor_frame();
    assert_eq!(
        apply(
            &mut model,
            &mut caches,
            &bindings,
            header(&payload, STRANGER),
            &payload,
        ),
        Applied::Nothing,
        "a channel this table has no route for is dropped, not refused"
    );
    assert!(
        model.pane(pane).is_none(),
        "and it lands nowhere: the bound pane's grid was never touched"
    );
    assert!(caches.is_empty(), "nor was any pane's scrollback cache");
}

#[test]
fn a_frame_for_a_pane_the_table_has_forgotten_is_dropped() {
    let pane = PaneId::new_v4();
    let mut model = ClientModel::new(24, 80);
    let mut caches: HashMap<PaneId, Scrollback> = HashMap::new();
    let mut bindings = Bindings::new();
    bindings.bind_grid(pane, CHANNEL);

    let payload = cursor_frame();
    assert_eq!(
        apply(
            &mut model,
            &mut caches,
            &bindings,
            header(&payload, CHANNEL),
            &payload,
        ),
        Applied::Grid(pane),
        "while the route is there the frame lands"
    );

    // `pane.close`: the routes go, the channel stays burned.
    bindings.forget_pane(pane);
    let before = model.pane(pane).map(|grid| grid.cursor());
    // A different cursor, so applying it would be visible.
    let mut moved = Vec::new();
    GridMessage::Cursor(Cursor {
        row: 11,
        col: 12,
        ..cursor()
    })
    .encode(&mut moved);
    assert_eq!(
        apply(
            &mut model,
            &mut caches,
            &bindings,
            header(&moved, CHANNEL),
            &moved,
        ),
        Applied::Nothing,
        "a frame the server queued before the pane died is dropped"
    );
    assert_eq!(
        model.pane(pane).map(|grid| grid.cursor()),
        before,
        "and changes nothing on the way past"
    );
}

#[test]
fn a_payload_this_build_cannot_read_is_dropped_on_a_bound_channel() {
    let pane = PaneId::new_v4();
    let mut model = ClientModel::new(24, 80);
    let mut caches: HashMap<PaneId, Scrollback> = HashMap::new();
    let mut bindings = Bindings::new();
    bindings.bind_grid(pane, CHANNEL);

    // DR-7's retired tag is the one payload of this shape that exists: a
    // message an older build would have decoded and committed to the pane's
    // scrollback. It is now bytes this build has no reading of, which is the
    // same answer it gives a newer peer's new message — skip the frame, keep
    // the stream.
    let payload = retired_scroll_notice();
    assert_eq!(
        apply(
            &mut model,
            &mut caches,
            &bindings,
            header(&payload, CHANNEL),
            &payload,
        ),
        Applied::Nothing,
        "an undecodable payload on a bound channel is dropped"
    );
    assert!(
        caches
            .get(&pane)
            .is_none_or(|cache| cache.head().get() == 0),
        "and commits nothing: the retired notice moved the committed head"
    );
}

// --------------------------------------------------------------- the reader

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-unbound-channel-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// Read one whole frame from the peer's end.
async fn read_frame(peer: &mut UnixStream) -> (FrameHeader, Vec<u8>) {
    let mut head = [0_u8; FRAME_HEADER_LEN];
    peer.read_exact(&mut head).await.expect("a frame header");
    let head = FrameHeader::decode(head).expect("the header decodes");
    let mut payload = vec![0_u8; head.payload_len()];
    peer.read_exact(&mut payload).await.expect("the payload");
    (head, payload)
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
async fn attached() -> (Session, UnixStream) {
    let (client, mut peer) = UnixStream::pair().expect("a socket pair");
    let attaching =
        tokio::spawn(async move { Session::attach(client, client_info(), true, None).await });

    let (head, payload) = read_frame(&mut peer).await;
    assert_eq!(
        head.channel, CONTROL_CHANNEL,
        "the hello is a control frame"
    );
    let hello: Hello = serde_json::from_slice(&payload).expect("decode the hello");
    let welcome = hello
        .accept(
            ServerInfo {
                name: "amx-unbound-channel-test".to_owned(),
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

#[tokio::test]
async fn a_frame_on_a_channel_this_connection_never_bound_is_refused() {
    let (mut session, mut peer) = attached().await;
    let payload = cursor_frame();
    write_frame(&mut peer, STRANGER, &payload).await;

    let mut buf = Vec::new();
    let error = session
        .read_frame_into(&mut buf)
        .await
        .expect_err("a channel this connection never bound is refused");
    assert!(
        matches!(error, NetError::UnboundChannel(channel) if channel == STRANGER),
        "the refusal names the channel: {error}"
    );
    assert!(
        !error.is_transport(),
        "and it is not the transport, so a redial cannot swallow it"
    );
    assert!(
        !error.is_abandoned_wait(),
        "nor a wait to re-ask: the peer, or this reader, has lost its place"
    );
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

/// The named test DR-12 says the register lacks, at the load it was seen under.
///
/// Every frame is raced by a ready future the way `App::run`'s `select!` races
/// the read against a keystroke, a `SIGWINCH` and the resize debounce, and the
/// payload is the byte the original failure reported as a channel number. A
/// reader that resynced anywhere in this would refuse `b'o'` = 111 and end the
/// session; one that resumes delivers every frame whole.
#[tokio::test]
async fn flooding_reads_cancelled_every_time_never_report_an_unbound_channel() {
    /// Enough frames that the 3-in-30 the register recorded would be certain.
    const FRAMES: usize = 60;
    /// How much of each frame arrives before its read is cancelled. Split
    /// across the header boundary on odd frames, so both resume points are
    /// exercised sixty times rather than once each.
    const HEAD_START: [usize; 2] = [3, FRAME_HEADER_LEN + 64];

    let (mut session, mut peer) = attached().await;
    session.bind_channel(CHANNEL, DEFAULT_STREAM_FRAME);

    // Made of the byte whose value is the channel the original failure was
    // reported on, and small enough that the peer's writes never wait for this
    // task to read — the flood is in the *cancellations*, not in the volume.
    let payload = vec![b'o'; 8 * 1024];
    let len = u32::try_from(payload.len()).expect("a frame that fits");
    let mut frame = FrameHeader::new(len, CHANNEL).encode().to_vec();
    frame.extend_from_slice(&payload);

    let mut buf = Vec::new();
    for n in 0..FRAMES {
        let head_start = HEAD_START[n % HEAD_START.len()];
        peer.write_all(&frame[..head_start])
            .await
            .expect("write the first slice");
        assert!(
            cancel_a_read(&mut session, &mut buf).await,
            "frame {n}: {head_start} of {} bytes cannot finish a read",
            frame.len()
        );

        peer.write_all(&frame[head_start..])
            .await
            .expect("write the rest");
        let head = session
            .read_frame_into(&mut buf)
            .await
            .expect("the cancelled frame resumes, and never resyncs");
        assert_eq!(head.channel, CHANNEL, "frame {n} kept its channel");
        assert_eq!(buf, payload, "frame {n} kept every byte of its payload");
    }

    // Nothing is left over: a reader that had resynced anywhere in that would
    // be part way through a frame nobody sent.
    assert!(
        matches!(session.torn(), Torn::Nothing),
        "the flood ended on a frame boundary: {:?}",
        session.torn()
    );
}
