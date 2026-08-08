//! Scaffolding for the resync suite: the frame-level moves every case makes.
//!
//! One connection speaking raw frames can be reading a reply, a delivery
//! notification and a grid message on three different channels at once, and
//! every assertion here is about *which* of those arrived. So the helpers are
//! all filters — classify a control frame, keep the grid messages on one
//! channel, wait out a silence — rather than a client API that would decide
//! for the test what it was looking at.

#![allow(dead_code, reason = "each test uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::time::Duration;

use amx_core::{Delivery, Event, GridGeneration, PaneId, Seq, WorkspaceId};
use amx_proto::Resume;
use amx_proto::control::session::StateReply;
use amx_proto::control::stream::BindReply;
use amx_proto::control::wait::{EVENT_METHOD, SubscribeReply};
use amx_proto::stream::grid::{Decoded, decode};
use serde_json::{Value, json};

use crate::support::{self, PATIENCE, result_of};

/// How long a test waits to be sure the server sent *nothing*.
///
/// Long enough that a keyframe the pump owed would have been built, queued and
/// written many times over — the grid pump's own drain retry is 4 ms.
pub const SILENCE: Duration = Duration::from_millis(600);

/// How long a pane may keep publishing after startup before a test gives up on
/// it going quiet.
pub const SETTLE: Duration = Duration::from_millis(400);

/// An event whose only job is to occupy a sequence number.
pub fn pane_event() -> Event {
    Event::PaneCreated {
        pane: PaneId::new_v4(),
        workspace: WorkspaceId::new_v4(),
    }
}

/// A resume block that presents a sequence and no generations.
pub fn resume_at(last_seq: Seq) -> Resume {
    Resume {
        last_seq,
        generations: Vec::new(),
    }
}

/// One control frame, decoded as either a reply to `id` or an event delivery.
pub enum Control {
    Reply(Value),
    Delivery(Delivery),
}

pub fn classify(payload: &[u8], id: u64) -> Control {
    let value: Value = serde_json::from_slice(payload).expect("a control frame is JSON");
    if value.get("method").and_then(Value::as_str) == Some(EVENT_METHOD) {
        let params = value
            .get("params")
            .expect("a delivery notification has params");
        return Control::Delivery(serde_json::from_value(params.clone()).expect("a delivery"));
    }
    let response: amx_proto::Response = serde_json::from_value(value).expect("a response");
    assert_eq!(
        response.id,
        amx_proto::RequestId::Number(id),
        "a reply to a call this test did not make"
    );
    match response.outcome {
        amx_proto::RpcOutcome::Result(value) => Control::Reply(value),
        amx_proto::RpcOutcome::Error(err) => panic!("call {id} failed: {}", err.message),
    }
}

/// Read the reply to `id`, tolerating deliveries that overtake it.
pub async fn read_reply(client: &mut support::Client, id: u64) -> Value {
    loop {
        let (header, payload) = client.next_frame().await;
        assert!(
            header.is_control(),
            "a reply arrives on the control channel"
        );
        if let Control::Reply(value) = classify(&payload, id) {
            return value;
        }
    }
}

/// Read the reply to a `events.subscribe` call plus the first `wanted`
/// deliveries, in whatever order the writer put them on the wire.
///
/// The pump is spawned inside the handler and the reply is queued after it
/// returns, so a delivery genuinely can beat the reply out. Ordering *between*
/// deliveries is the contract; ordering against the reply is not.
/// A subscription that never delivers is the failure this suite is most likely
/// to hit, so the collection is bounded: the caller then reports "expected
/// these, got those" instead of hanging until a read deadline that names
/// nothing.
pub async fn subscribe_reply(
    client: &mut support::Client,
    id: u64,
    wanted: usize,
) -> (SubscribeReply, Vec<Delivery>) {
    let mut reply = None;
    let mut deliveries = Vec::new();
    while reply.is_none() || deliveries.len() < wanted {
        let patience = if reply.is_some() { SILENCE } else { PATIENCE };
        let Some((header, payload)) = client.next_frame_within(patience).await else {
            break;
        };
        assert!(header.is_control());
        match classify(&payload, id) {
            Control::Reply(value) => {
                reply = Some(serde_json::from_value(value).expect("a subscribe reply"));
            }
            Control::Delivery(delivery) => deliveries.push(delivery),
        }
    }
    (reply.expect("the subscribe reply"), deliveries)
}

/// The next event delivery, or `None` if the connection stays quiet.
pub async fn next_delivery(client: &mut support::Client, patience: Duration) -> Option<Delivery> {
    let (header, payload) = client.next_frame_within(patience).await?;
    if !header.is_control() {
        return None;
    }
    match classify(&payload, u64::MAX) {
        Control::Delivery(delivery) => Some(delivery),
        Control::Reply(_) => None,
    }
}

/// The one pane a freshly seeded session has.
pub async fn sole_pane(client: &mut support::Client) -> PaneId {
    let response = client.request(900, "session.state", json!({})).await;
    let state: StateReply =
        serde_json::from_value(result_of(&response).clone()).expect("a state reply");
    let [pane] = state.panes.as_slice() else {
        panic!(
            "a seeded session has exactly one pane, not {}",
            state.panes.len()
        );
    };
    pane.pane
}

/// Declare a viewport, which is what drives the pane's pty size — and so its
/// grid generation (04 §3).
pub async fn resize_panes(client: &mut support::Client, pane: PaneId, rows: u16, cols: u16) {
    let response = client
        .request(
            901,
            "client.viewport",
            json!({ "rows": rows, "cols": cols, "panes": [pane] }),
        )
        .await;
    let _ = result_of(&response);
}

/// Bind a grid stream, optionally presenting a generation.
pub async fn bind_grid(
    client: &mut support::Client,
    id: u64,
    pane: PaneId,
    generation: Option<GridGeneration>,
) -> BindReply {
    let mut params = json!({ "kind": "pane_grid", "pane": pane });
    if let Some(generation) = generation {
        params["generation"] = json!(generation);
    }
    client.send_request(id, "stream.bind", params).await;
    let value = read_reply(client, id).await;
    serde_json::from_value(value).expect("a bind reply")
}

/// Drain `channel` until it goes quiet, and report the generation the last
/// message on it carried.
///
/// A pane running a real shell publishes while it starts up; a test that binds
/// a second stream mid-burst is measuring the pump's missed-publication path,
/// not the resume decision. So the first stream is drained to silence first,
/// and its last generation is the live one every later bind is judged against.
pub async fn settle_at(client: &mut support::Client, channel: u8) -> GridGeneration {
    let mut generation = None;
    loop {
        let Some((header, payload)) = client.next_frame_within(SETTLE).await else {
            return generation.expect("the first bind sent at least one grid message");
        };
        if header.channel != channel {
            continue;
        }
        match decode(&payload).expect("a decodable grid message") {
            Decoded::Reset { generation: at, .. } | Decoded::Delta { generation: at, .. } => {
                generation = Some(at);
            }
            _ => {}
        }
    }
}

/// Every grid message that arrives on `channel` within `patience`.
pub async fn frames_on(
    client: &mut support::Client,
    channel: u8,
    patience: Duration,
) -> Vec<Decoded> {
    let mut messages = Vec::new();
    let deadline = tokio::time::Instant::now() + patience;
    while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
        let Some((header, payload)) = client.next_frame_within(remaining).await else {
            break;
        };
        if header.channel != channel {
            continue;
        }
        messages.push(decode(&payload).expect("a decodable grid message"));
    }
    messages
}
