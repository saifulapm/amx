//! W08: what a `Hello` carrying `Resume` gets (`docs/09-m3-plan.md` D-M3-7).
//!
//! Every type this suite exercises was frozen a milestone before anything read
//! it — `Resume` in the hello, `subscribe_after` on the bus,
//! `KeyframeReason::Generation` in the damage layer, the additive `generation`
//! on `stream.bind` — so the thing under test is never a type. It is the
//! connection plumbing, which is why every case drives a real server over a
//! real socket: a unit test of the types would have passed before this wave
//! started.
//!
//! Two facts a reattach presents, and one question each answers:
//!
//! - `last_seq` — where does the event subscription open? At
//!   `subscribe_after(last_seq)`, so the missed events are replayed; and past
//!   the ring, at a `gap`, which is loss made visible rather than a silent
//!   skip.
//! - `generations` — what does a re-bound grid stream open with? Nothing, when
//!   the pane is still at the generation the client holds; a `Generation`
//!   keyframe when it is not.
//!
//! And the case that pins the blast radius: a connection that says nothing
//! about resume gets exactly what it got before any of this existed.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::event::bus::DEFAULT_REPLAY_CAPACITY;
use amx_core::{Delivery, GridGeneration, PaneId, Seq};
use amx_proto::Resume;
use amx_proto::control::wait::SubscribeReply;
use amx_proto::stream::StreamId;
use amx_proto::stream::grid::Decoded;
use amx_server::damage::{GridStream, GridStreamConfig, KeyframeReason};
use serde_json::json;

// A test crate root's module directory is `tests/`, so the harness needs its
// path spelled out to live beside its one suite rather than next to everybody's.
#[path = "resync/harness.rs"]
mod harness;
mod support;

use harness::{
    SILENCE, bind_grid, frames_on, next_delivery, pane_event, read_reply, resize_panes, resume_at,
    settle_at, sole_pane, subscribe_reply,
};
use support::{PATIENCE, Server, wait_until};

// ------------------------------------------------------------ the event half

#[tokio::test]
async fn a_resume_within_the_ring_replays_exactly_the_missed_events() {
    let server = Server::start("rrepl").await;

    // Three events this client claims to have consumed, then four it was away
    // for. The ring holds 64, so nothing here can fall out of it.
    for _ in 0..3 {
        server.ctx.bus.publish(pane_event());
    }
    let last_seq = server.ctx.bus.head();
    for _ in 0..4 {
        server.ctx.bus.publish(pane_event());
    }
    let before_attach = server.ctx.bus.head();

    let mut client = server.connect().await;
    client
        .hello_resuming(amx_proto::version::window(), resume_at(last_seq))
        .await;
    // Attaching publishes `ClientAttached`, which the resumed subscription is
    // just as entitled to as the four it missed: it happened after `last_seq`.
    let attached = before_attach + 1;

    client.send_request(1, "events.subscribe", json!({})).await;
    let (reply, deliveries) = subscribe_reply(&mut client, 1, (attached - last_seq) as usize).await;

    assert_eq!(
        reply.seq, last_seq,
        "a resumed subscription is taken at the sequence it resumed from, not \
         at the head"
    );
    let seqs: Vec<Seq> = deliveries
        .iter()
        .map(|delivery| match delivery {
            Delivery::Event(envelope) => envelope.seq,
            Delivery::Gap { from, to } => panic!("a gap {from}..={to} inside the ring"),
        })
        .collect();
    let expected: Vec<Seq> = (last_seq + 1..=attached).collect();
    assert_eq!(
        seqs, expected,
        "exactly the missed events, in order: no duplicates, no skips"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_resume_beyond_the_ring_opens_with_a_gap_never_a_silent_skip() {
    // The real ring, not a test-sized one: the resume window a client actually
    // gets is `DEFAULT_REPLAY_CAPACITY` wide.
    let server = Server::start_with_replay("rgap", DEFAULT_REPLAY_CAPACITY).await;

    let published = DEFAULT_REPLAY_CAPACITY as u64 + 100;
    for _ in 0..published {
        server.ctx.bus.publish(pane_event());
    }

    // Seq 1 fell off the back long ago.
    let mut client = server.connect().await;
    client
        .hello_resuming(amx_proto::version::window(), resume_at(1))
        .await;
    // The attach publishes `ClientAttached`, which evicts one more event; the
    // ring's oldest entry is only settled once that has landed.
    wait_until("the attach is published", || {
        server.ctx.bus.head() == published + 1
    })
    .await;
    let head = server.ctx.bus.head();
    let oldest_held = head - DEFAULT_REPLAY_CAPACITY as u64 + 1;

    client.send_request(1, "events.subscribe", json!({})).await;
    let (reply, deliveries) = subscribe_reply(&mut client, 1, 2).await;

    assert_eq!(
        reply.seq, 1,
        "the subscription is still taken where it asked"
    );
    assert!(
        deliveries.len() >= 2,
        "a resume past the ring must still deliver: the gap and then the \
         oldest event still held, and got {deliveries:?}"
    );
    assert_eq!(
        deliveries[0],
        Delivery::Gap {
            from: 2,
            to: oldest_held - 1,
        },
        "the events between the resume point and the oldest one still held are \
         reported as lost, not skipped"
    );
    match &deliveries[1] {
        Delivery::Event(envelope) => assert_eq!(
            envelope.seq, oldest_held,
            "delivery resumes at the oldest event the ring still holds"
        ),
        Delivery::Gap { from, to } => panic!("a second gap {from}..={to}"),
    }

    server.shutdown().await;
}

// ------------------------------------------------------------- the grid half

#[tokio::test]
async fn a_bind_with_the_current_generation_sends_no_keyframe() {
    let server = Server::start("rcur").await;
    let mut client = server.attach_rendering().await;
    let pane = sole_pane(&mut client).await;
    resize_panes(&mut client, pane, 40, 100).await;

    // A first, ordinary bind: it opens with a keyframe, and the generation it
    // carries is the one the pane is live at.
    let first = bind_grid(&mut client, 10, pane, None).await;
    let generation = settle_at(&mut client, first.channel).await;

    // The second bind presents exactly that generation. The client is not
    // stale, so it is owed no full grid.
    let second = bind_grid(&mut client, 11, pane, Some(generation)).await;
    let opened = frames_on(&mut client, second.channel, SILENCE).await;

    assert!(
        !opened.is_empty(),
        "the resumed stream must be live, not merely quiet: it owes the cursor \
         even when it owes no cells"
    );
    for message in &opened {
        assert!(
            !matches!(message, Decoded::Reset { .. }),
            "a bind at the pane's current generation must open delta-only, and \
             this one sent a keyframe: {message:?}"
        );
    }

    server.shutdown().await;
}

#[tokio::test]
async fn a_bind_with_a_stale_generation_opens_with_keyframe_reason_generation() {
    let server = Server::start("rstale").await;
    let mut client = server.attach_rendering().await;
    let pane = sole_pane(&mut client).await;
    // The resize is what makes a stale generation reachable at all: a pane
    // nothing ever resized sits at `FIRST`, and nothing is older than that.
    resize_panes(&mut client, pane, 40, 100).await;

    let first = bind_grid(&mut client, 10, pane, None).await;
    let generation = settle_at(&mut client, first.channel).await;
    assert!(
        generation > GridGeneration::FIRST,
        "the viewport declaration must have resized the pane, or this test is \
         not testing staleness"
    );
    let stale = GridGeneration::from_raw(generation.get() - 1);

    let second = bind_grid(&mut client, 11, pane, Some(stale)).await;
    let opened = frames_on(&mut client, second.channel, SILENCE).await;

    let Some(Decoded::Reset {
        generation: sent, ..
    }) = opened.first()
    else {
        panic!("a stale bind must open with a keyframe, and got {opened:?}");
    };
    assert_eq!(
        *sent, generation,
        "the keyframe carries the generation the client is being moved to"
    );

    // The wire cannot carry *why* a keyframe was sent — `GridMessage::Reset`
    // has no reason field and gains none for this — so the reason is asserted
    // where it is decided, against the same live generation the binds above
    // were judged against.
    assert_eq!(
        opening_keyframe(pane, generation, Some(stale)),
        Some(KeyframeReason::Generation),
        "a stale generation is what makes the keyframe owed"
    );
    assert_eq!(
        opening_keyframe(pane, generation, Some(generation)),
        None,
        "the current generation owes none"
    );
    assert_eq!(
        opening_keyframe(pane, generation, None),
        Some(KeyframeReason::First),
        "a bind that presents no generation is a first attach, unchanged"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_generation_presented_in_the_hello_is_spent_by_the_first_rebind() {
    let server = Server::start("rhello").await;

    // One attached client, so the session has a pane, and so the pane stays
    // alive while a second connection asks about it.
    let mut attached = server.attach_rendering().await;
    let pane = sole_pane(&mut attached).await;
    resize_panes(&mut attached, pane, 40, 100).await;
    let bound = bind_grid(&mut attached, 10, pane, None).await;
    let generation = settle_at(&mut attached, bound.channel).await;

    // A reattach that presents the generation in its hello and never mentions
    // it again. This is the `generations` half of `Resume`, which nothing read
    // before this wave.
    let mut resumed = server.connect().await;
    resumed
        .hello_resuming(
            amx_proto::version::window(),
            Resume {
                last_seq: server.ctx.bus.head(),
                generations: vec![(pane, generation)],
            },
        )
        .await;

    let first = bind_grid(&mut resumed, 20, pane, None).await;
    let opened = frames_on(&mut resumed, first.channel, SILENCE).await;
    assert!(
        !opened.is_empty() && !opened.iter().any(|m| matches!(m, Decoded::Reset { .. })),
        "the hello's generation is the default for the first re-bind, so it \
         opens delta-only: {opened:?}"
    );

    // And it is spent. A second bind naming no generation is a client asking
    // for the grid afresh, not repeating a claim it already cashed in.
    let second = bind_grid(&mut resumed, 21, pane, None).await;
    let again = frames_on(&mut resumed, second.channel, SILENCE).await;
    assert!(
        matches!(again.first(), Some(Decoded::Reset { .. })),
        "a second bind with no generation must open with a keyframe: {again:?}"
    );

    server.shutdown().await;
}

/// What a stream binding `pane` opens owing, for a client presenting `held`
/// against a pane live at `live`.
fn opening_keyframe(
    pane: PaneId,
    live: GridGeneration,
    held: Option<GridGeneration>,
) -> Option<KeyframeReason> {
    let config = GridStreamConfig::new(pane, StreamId::new(9)).resuming(live, held);
    GridStream::new(config)
        .expect("a bindable stream id")
        .owed_keyframe()
}

// ---------------------------------------------------------- the unchanged case

#[tokio::test]
async fn a_verb_connection_without_resume_behaves_exactly_as_before() {
    let server = Server::start("rplain").await;

    // Events published before the connection exists. A resuming client would
    // be replayed them; a verb connection must not be.
    for _ in 0..5 {
        server.ctx.bus.publish(pane_event());
    }

    let mut client = server.attach_rendering().await;
    let pane = sole_pane(&mut client).await;
    let head_before = server.ctx.bus.head();

    client.send_request(1, "events.subscribe", json!({})).await;
    let reply: SubscribeReply = {
        let response = read_reply(&mut client, 1).await;
        serde_json::from_value(response).expect("a subscribe reply")
    };
    assert!(
        reply.seq >= head_before,
        "a subscription with nothing to resume from opens at the head ({} < {head_before})",
        reply.seq
    );

    // Whatever arrives is new traffic; nothing from before the subscription.
    let mut fresh = 0_usize;
    while let Some(delivery) = next_delivery(&mut client, SILENCE).await {
        match delivery {
            Delivery::Event(envelope) => {
                assert!(
                    envelope.seq > reply.seq,
                    "seq {} was published before the subscription and must not \
                     be replayed to a connection that never asked to resume",
                    envelope.seq
                );
                fresh += 1;
            }
            Delivery::Gap { from, to } => panic!("a gap {from}..={to} on a fresh subscription"),
        }
    }
    let _ = fresh;

    // And a bind that presents no generation opens with a keyframe, which is
    // what every bind has done since M0.
    let bound = bind_grid(&mut client, 2, pane, None).await;
    let opened = frames_on(&mut client, bound.channel, PATIENCE).await;
    assert!(
        matches!(opened.first(), Some(Decoded::Reset { .. })),
        "a bind with no generation must still open with a keyframe, and got \
         {opened:?}"
    );

    server.shutdown().await;
}
