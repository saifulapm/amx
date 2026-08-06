//! T10: the connection writer's strict channel priority (04 §4).
//!
//! "The connection writer has strict channel priority (control > grid deltas >
//! history/bulk), and history transfers are chunked — a big scrollback fetch
//! can never head-of-line-block a keystroke's response."

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_proto::frame::MAX_CONTROL_FRAME;
use amx_proto::stream::Priority;
use amx_server::conn::writer::{self, OutFrame, OutboundError};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

mod support;

use support::read_frame;

/// The channel a history transfer is bound to in these tests.
const HISTORY_CHANNEL: u8 = 7;
/// One history chunk.
const CHUNK: usize = 64 * 1024;
/// How many chunks the transfer is.
const CHUNKS: usize = 64;
/// The control frame that has to overtake them.
const REPLY: &[u8] = br#"{"jsonrpc":"2.0","id":1}"#;

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
    out.send(OutFrame::control(REPLY.to_vec()).expect("a control frame"))
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
    assert_eq!(payload, REPLY);

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
fn each_priority_class_has_its_own_queue() {
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
        matches!(err, OutboundError::TooLarge { cap, .. } if cap == MAX_CONTROL_FRAME),
        "{err}"
    );
}

#[test]
fn a_stream_frame_may_not_claim_the_control_channel() {
    let err = OutFrame::stream(0, Priority::Grid, 16, vec![b'x'])
        .expect_err("channel 0 is control, always");
    assert!(
        matches!(err, OutboundError::ControlChannelReserved),
        "{err}"
    );
}
