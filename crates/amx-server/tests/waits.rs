//! V11 acceptance: the event wire and the three long polls.
//!
//! Two contracts are on trial here, and they are the same contract seen from
//! two sides (04 §2):
//!
//! > Subscribers that fall behind the replay buffer get an explicit
//! > `gap{from,to}` event — never a silent drop.
//!
//! > **Waits are state predicates, not event predicates.** … A transition
//! > falling inside a gap can therefore never hang a wait.
//!
//! So the suite sizes the replay buffer down to two events and floods past it
//! on purpose. That is not an artificial stress: a bounded buffer is what the
//! design chose over backpressure on publishers, and a gap is the *documented*
//! outcome, so a test that never produced one would be testing the easy half.
//!
//! Every timing assertion here is a bound on a condition, never a nap that is
//! then believed. The one place this suite sleeps deliberately is to open a
//! scheduling window before a flood; if that window expires short the test
//! proves less and still passes, which is the rule `tests/hygiene.rs` states.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

// A test crate root's module directory is `tests/`, so the harness needs its
// path spelled out to live beside its one suite rather than next to everybody's.
#[path = "waits/harness.rs"]
mod harness;
mod support;

use std::time::Duration;

use amx_core::agent::AgentState;
use amx_core::{Delivery, Event, PaneId};
use serde_json::{Value, json};

use harness::{Rig, Wire};
use support::{PATIENCE, TICK};

/// How long a wait that must land "the instant the status does" may take.
///
/// Two orders of magnitude under herdr's 300–500 ms detection scan, and no
/// multiple of anything in this build: there is no interval to be a multiple
/// of, which is the property being asserted.
const SUB_TICK: Duration = Duration::from_millis(250);

/// The window a test opens for the server to reach its first `await`.
const SETTLE: Duration = Duration::from_millis(200);

// ------------------------------------------------------- the event wire

/// The subscribe reply names the sequence the cursor sits at, and everything
/// after it arrives once, in order, on the control channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_reply_carries_the_bus_seq_and_deliveries_follow_in_order() {
    let rig = Rig::start("sub", 64).await;
    let mut wire = rig.wire(false).await;

    let at = wire.subscribe(None).await;
    assert_eq!(
        at,
        rig.head(),
        "the reply carries the head at subscribe time"
    );

    let pane = PaneId::new_v4();
    let published: Vec<u64> = (0..3)
        .map(|n| {
            rig.ctx.bus.publish(Event::PaneTitle {
                pane,
                title: format!("filler {n}"),
            })
        })
        .collect();
    assert_eq!(
        published,
        vec![at + 1, at + 2, at + 3],
        "the first delivery must be the one after the subscribe seq",
    );

    for (n, seq) in published.iter().enumerate() {
        match wire.delivery().await {
            Delivery::Event(envelope) => {
                assert_eq!(envelope.seq, *seq, "deliveries arrive in bus order");
                assert_eq!(
                    envelope.event,
                    Event::PaneTitle {
                        pane,
                        title: format!("filler {n}"),
                    }
                );
            }
            other => panic!("expected event {seq}, got {other:?}"),
        }
    }
    rig.stop().await;
}

/// A subscriber that stops reading is told what it missed, over the wire.
///
/// Current-thread on purpose: the flood is a synchronous loop, so nothing else
/// on this runtime can run while it publishes. The connection's pump therefore
/// cannot drain a single delivery during it, and a replay buffer of two events
/// against a flood of two thousand leaves exactly one honest answer.
#[tokio::test]
async fn a_slow_subscriber_receives_gap_never_a_silent_drop_over_the_wire() {
    let rig = Rig::start("gap", 2).await;
    let mut wire = rig.wire(false).await;
    let at = wire.subscribe(None).await;

    let pane = PaneId::new_v4();
    rig.flood(pane, 2_000);

    // Read until the gap: whatever the pump managed to queue before it stalled
    // is legitimate traffic, and the contract is that the loss *after* it is
    // announced rather than skipped.
    let mut seen = 0;
    let gap = loop {
        assert!(seen < 512, "read {seen} deliveries and never saw a gap");
        seen += 1;
        match wire.delivery().await {
            Delivery::Gap { from, to } => break (from, to),
            Delivery::Event(_) => {}
        }
    };
    assert!(gap.0 > at, "a gap starts after what the subscriber had");
    assert!(gap.1 >= gap.0, "a gap is never empty");

    // And the stream resumes: `to + 1` is the next delivery, never a reset.
    match wire.delivery().await {
        Delivery::Event(envelope) => assert_eq!(
            envelope.seq,
            gap.1 + 1,
            "deliveries continue at to + 1, so replay is always distinguishable",
        ),
        other => panic!("expected the first event after the gap, got {other:?}"),
    }
    rig.stop().await;
}

/// Resuming from a sequence the buffer has dropped opens with a gap, not an
/// error and not a silent jump — the consumer half of the resync contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribing_after_a_dropped_seq_opens_with_a_gap() {
    let rig = Rig::start("rsme", 2).await;
    let pane = PaneId::new_v4();
    rig.flood(pane, 32);

    let mut wire = rig.wire(false).await;
    let at = wire.subscribe(Some(1)).await;
    assert_eq!(at, 1, "the reply echoes where the caller asked to resume");

    match wire.delivery().await {
        Delivery::Gap { from, to } => {
            assert_eq!(from, 2, "the gap starts at the first sequence asked for");
            assert!(to >= 2, "and covers what the buffer no longer holds");
        }
        other => panic!("a resume past the replay buffer must open with a gap, got {other:?}"),
    }
    rig.stop().await;
}

// ------------------------------------------------------------ status waits

/// A wait for a status returns as soon as the status is written, with no
/// interval anywhere between the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_until_blocked_returns_the_instant_the_status_lands() {
    let rig = Rig::start("blk", 64).await;
    let mut wire = rig.wire(true).await;
    let pane = wire.seeded_pane().await;

    let id = wire
        .request(
            "wait",
            json!({ "until": "blocked", "target": pane.to_string() }),
        )
        .await;
    // The server needs a moment to reach its first await. Expiring short only
    // turns this into the already-satisfied case, which is another test's job
    // and never a failure here.
    tokio::time::sleep(SETTLE).await; // deliberate

    let flipped = tokio::time::Instant::now();
    rig.set_status(pane, Some(AgentState::Working), AgentState::Blocked);
    let reply = expect_ok(wire.reply(id).await);
    let took = flipped.elapsed();

    assert_eq!(reply["satisfied"], json!(true));
    assert_eq!(reply["agent"]["state"], json!("blocked"));
    assert!(
        took < SUB_TICK,
        "the wait took {took:?}, which is long enough to have been a poll",
    );
    rig.stop().await;
}

/// A condition that already holds is answered from state, with no event
/// involved at any point — because there is none to involve.
///
/// The transition is committed silently, so no event anywhere on the bus
/// describes it and no event ever will. A wait that consumed events to notice
/// its predicate would therefore not merely be slow here: it would never
/// return at all. Returning promptly is the whole assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_a_condition_already_true_returns_without_consuming_events() {
    let rig = Rig::start("true", 64).await;
    let mut wire = rig.wire(true).await;
    let pane = wire.seeded_pane().await;

    rig.set_status_silently(pane, AgentState::Blocked);

    let asked = tokio::time::Instant::now();
    let reply = wire
        .call(
            "wait",
            json!({ "until": "blocked", "target": pane.to_string() }),
        )
        .await;
    let took = asked.elapsed();

    assert_eq!(reply["satisfied"], json!(true));
    assert_eq!(reply["agent"]["state"], json!("blocked"));
    assert!(
        took < SUB_TICK,
        "the wait took {took:?}; nothing was ever published about this \
         transition, so anything but an immediate evaluation is a wait on \
         traffic that has nothing to do with it",
    );
    rig.stop().await;
}

/// The proof T03's discipline exists for, carried to the wire: the transition
/// happens inside the gap, and the wait still lands.
///
/// The flip is committed with no event of its own, so nothing the waiter is
/// interested in is ever published. Its only remaining wake-up is the gap the
/// flood leaves behind — and a gap re-evaluates state unconditionally, which is
/// the whole mechanism.
#[tokio::test]
async fn a_transition_inside_a_bus_gap_cannot_hang_a_wire_wait() {
    let rig = Rig::start("hang", 2).await;
    let mut wire = rig.wire(true).await;
    let pane = wire.seeded_pane().await;

    let id = wire
        .request(
            "wait",
            json!({ "until": "blocked", "target": pane.to_string() }),
        )
        .await;
    // Long enough for the wait to subscribe and evaluate false. Short only
    // weakens the test — it would become the already-true case — never reddens
    // it.
    tokio::time::sleep(SETTLE).await; // deliberate

    rig.set_status_silently(pane, AgentState::Blocked);
    rig.flood(pane, 2_000);

    let reply = expect_ok(
        tokio::time::timeout(PATIENCE, wire.reply(id))
            .await
            .expect("a transition inside a gap must not hang a wire wait"),
    );
    assert_eq!(reply["satisfied"], json!(true));
    assert_eq!(reply["agent"]["state"], json!("blocked"));
    rig.stop().await;
}

/// A timeout is an answer, not an error: the caller is told the condition did
/// not hold, and what the status was when the clock ran out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wait_that_times_out_answers_unsatisfied_rather_than_failing() {
    let rig = Rig::start("tmo", 64).await;
    let mut wire = rig.wire(true).await;
    let pane = wire.seeded_pane().await;

    let reply = wire
        .call(
            "wait",
            json!({ "until": "idle", "target": pane.to_string(), "timeout_ms": 50 }),
        )
        .await;
    assert_eq!(reply["satisfied"], json!(false));
    rig.stop().await;
}

/// A target that names no pane is refused now rather than left to expire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wait_on_an_unknown_pane_is_refused_not_left_to_time_out() {
    let rig = Rig::start("unk", 64).await;
    let mut wire = rig.wire(true).await;

    let err = wire
        .call_err(
            "wait",
            json!({ "until": "blocked", "target": PaneId::new_v4().to_string() }),
        )
        .await;
    assert_eq!(err.code, amx_proto::rpc::RpcError::INVALID_PARAMS);
    rig.stop().await;
}

// ------------------------------------------------------------- exit waits

/// `--until exited` completes on the process ending and reports its status.
///
/// It also proves the long poll runs beside the reader rather than inside it:
/// the very call that makes the child exit is sent on the same connection while
/// the wait is outstanding.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_until_exited_completes_on_pane_exit_with_status() {
    let rig = Rig::start("exit", 64).await;
    let mut wire = rig.wire(true).await;
    let root = wire.seeded_pane().await;
    let pane = split(&mut wire, root, "printf 'ready\\r\\n'; read x; exit 7").await;
    wait_for_screen(&mut wire, pane, "ready").await;

    let id = wire
        .request(
            "wait",
            json!({ "until": "exited", "target": pane.to_string() }),
        )
        .await;
    tokio::time::sleep(SETTLE).await; // deliberate
    wire.call(
        "pane.send_text",
        json!({ "target": pane.to_string(), "text": "\n" }),
    )
    .await;

    let reply = expect_ok(
        tokio::time::timeout(PATIENCE, wire.reply(id))
            .await
            .expect("the wait must complete when the child exits"),
    );
    assert_eq!(reply["satisfied"], json!(true));
    assert_eq!(
        reply["status"],
        json!(7),
        "the child's exit code is reported"
    );
    rig.stop().await;
}

// ------------------------------------------------------------ output waits

/// `pane.wait_output` matches the visible screen, per damage batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_output_matches_the_visible_screen_per_damage_batch() {
    let rig = Rig::start("out", 64).await;
    let mut wire = rig.wire(true).await;
    let root = wire.seeded_pane().await;
    let pane = split(
        &mut wire,
        root,
        "printf 'ready\\r\\n'; read x; printf 'build ok in 3s\\r\\n'; sleep 60",
    )
    .await;
    wait_for_screen(&mut wire, pane, "ready").await;

    let id = wire
        .request(
            "pane.wait_output",
            json!({ "target": pane.to_string(), "regex": "^build ok in \\d+s$" }),
        )
        .await;
    tokio::time::sleep(SETTLE).await; // deliberate
    wire.call(
        "pane.send_text",
        json!({ "target": pane.to_string(), "text": "\n" }),
    )
    .await;

    let reply = expect_ok(
        tokio::time::timeout(PATIENCE, wire.reply(id))
            .await
            .expect("the pattern must be found once the pane paints it"),
    );
    assert_eq!(reply["matched"], json!(true));
    assert_eq!(reply["line"], json!("build ok in 3s"));
    rig.stop().await;
}

/// One of `match` and `regex`, never both and never neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_output_takes_exactly_one_of_match_and_regex() {
    let rig = Rig::start("arg", 64).await;
    let mut wire = rig.wire(true).await;
    let pane = wire.seeded_pane().await;

    for params in [
        json!({ "target": pane.to_string() }),
        json!({ "target": pane.to_string(), "match": "a", "regex": "a" }),
        json!({ "target": pane.to_string(), "regex": "a(" }),
    ] {
        let err = wire.call_err("pane.wait_output", params.clone()).await;
        assert_eq!(
            err.code,
            amx_proto::rpc::RpcError::INVALID_PARAMS,
            "{params} should have been refused",
        );
    }
    rig.stop().await;
}

// ------------------------------------------------------------------ helpers

/// Unwrap a reply, naming the error if there was one.
fn expect_ok(response: amx_proto::Response) -> Value {
    match response.outcome {
        amx_proto::RpcOutcome::Result(value) => value,
        amx_proto::RpcOutcome::Error(err) => {
            panic!("the call failed: {} ({})", err.message, err.code)
        }
    }
}

/// Split off a pane running `script` under `/bin/sh`.
async fn split(wire: &mut Wire, root: PaneId, script: &str) -> PaneId {
    let reply = wire
        .call(
            "pane.split",
            json!({
                "pane": root.to_string(),
                "direction": "vertical",
                "command": ["/bin/sh", "-c", script],
            }),
        )
        .await;
    reply["pane"]
        .as_str()
        .expect("the split names its pane")
        .parse()
        .expect("a pane id")
}

/// Wait until the pane has painted `needle`, using `pane.read` rather than the
/// verb under test.
async fn wait_for_screen(wire: &mut Wire, pane: PaneId, needle: &str) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let reply = wire
            .call("pane.read", json!({ "target": pane.to_string() }))
            .await;
        let rows: Vec<String> = reply["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row.as_str().unwrap_or_default().to_owned())
            .collect();
        if rows.iter().any(|row| row.contains(needle)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pane never painted {needle:?}: {rows:?}"
        );
        tokio::time::sleep(TICK).await;
    }
}
