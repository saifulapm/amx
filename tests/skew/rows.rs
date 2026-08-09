//! The per-milestone row ledgers: every tabled row answers, and none of them
//! answers a seam.
//!
//! A child module of [`super`], sharing its table, its [`sample_params`] and
//! its fixtures — a second harness with a second notion of what a bogus pane id
//! is would be a second definition of the thing under test. Split out by X02
//! when M4's row pushed the parent past the module budget.
//!
//! What [`super`] proves is that *negotiation* holds across a version window
//! and across the bridge transport. What this proves is narrower and moves
//! every milestone: which rows are wired, and which one is not yet. As of X10
//! the answer is all of them.

use amx_proto::control::Method;
use amx_proto::rpc::RpcError;
use amx_proto::version::{PROTO_MAX, PROTO_MIN};
use rig::wire::error_of;
use rig::{Env, Wire, result_of};
use serde_json::json;

use super::sample_params;

/// The code a dispatch seam answers with while its wiring is being built.
///
/// `-32099`, the far end of JSON-RPC 2.0's implementation-defined server error
/// range. Deliberately a literal rather than an import: the assertions below
/// have to keep meaning what they mean across the milestones where nothing in
/// the tree defines it at all.
///
/// M1, M2 and M3 all spelled it `-32000`, which was free while amx had no code
/// of its own in the range. It is not free now — `WAIT_ABANDONED` is `-32000`
/// and a client that recognises it *redials and asks again*, so an unwired row
/// answering that number would put a caller in a loop. X02 moved the seam to
/// the bottom of the range: the permanent codes fill it from the top, the
/// temporary one from the bottom, and they cannot meet before it is deleted.
///
/// Every test here reads it in the same direction now: no row answers it. M2's
/// twelve and M3's one were closed before this milestone started, and
/// [`method_golden_and_skew_arm_cover_agent_list`] joined them when X10 wired
/// `agent.list`. The constant stays because the *ban* is what the ledgers are
/// worth — a row that starts answering it has been un-implemented — and because
/// the next milestone's first seam will be spelled the same way.
const SEAM_CODE: i32 = -32099;

/// The twelve rows M2 added, by wire name.
///
/// Named rather than derived so the harness states what it is covering: a
/// thirteenth row would compile (the exhaustive match above is what catches
/// that) but would not be claimed here, and the assertion below counts.
const M2_ROWS: &[&str] = &[
    "agent.report",
    "agent.start",
    "agent.prompt",
    "agent.explain",
    "agent.next",
    "wait",
    "events.subscribe",
    "pane.send_text",
    "pane.send_keys",
    "pane.run",
    "pane.read",
    "pane.wait_output",
];

#[tokio::test]
async fn skew_calls_every_m2_row_and_none_is_method_not_found() {
    let env = Env::new("skew-m2");
    let server = env.server();
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello((PROTO_MIN, PROTO_MAX)).await;

    for name in M2_ROWS {
        let method = Method::from_wire_name(name)
            .unwrap_or_else(|| panic!("{name} is not in this build's method table"));
        let reply = wire
            .request(method.wire_name(), sample_params(method))
            .await;
        // Two codes are forbidden, and the second one only since V17.
        //
        // `METHOD_NOT_FOUND` would tell a client this build does not have the
        // method its own table lists, and would let a row land in the table
        // with no handler at all — the failure this harness exists to catch.
        //
        // The seam code was the *permitted* answer while M2 was being built: a
        // tabled row whose wiring had not landed answered through the `seam`
        // helper rather than disowning itself. V17 closed the last two seams,
        // so no M2 row may produce it again — and asserting that here is the
        // wire-side half of the ledger `tests/hygiene.rs` keeps at the source
        // level. A row that starts answering it has been un-implemented.
        if let amx_proto::RpcOutcome::Error(err) = &reply.outcome {
            assert_ne!(
                err.code,
                RpcError::METHOD_NOT_FOUND,
                "the server disowned its own method {name}",
            );
            assert_ne!(
                err.code, SEAM_CODE,
                "{name} answers the seam code; M2's ledger is empty and every \
                 row of docs/08-m2-plan.md §4 owes real behavior",
            );
        }
    }

    // And the connection survived all twelve.
    let alive = wire.request("ping", json!({})).await;
    assert!(result_of(&alive)["seq"].is_u64());

    assert_eq!(
        M2_ROWS.len(),
        12,
        "docs/08-m2-plan.md §4 tables twelve rows; this list must be all of them",
    );

    drop(server);
}

/// M3's one row, over the wire, on a connection that survives it.
///
/// The other half of the goldens law of `docs/09-m3-plan.md` §4: a method
/// golden freezes the *shape*, and this freezes that the shape is reachable —
/// the table routes `session.handoff`, the server owns it, and asking for one
/// leaves the session exactly as it was.
///
/// W03 wrote this against the seam code, which was the *permitted* answer for a
/// tabled row without wiring: `METHOD_NOT_FOUND` would tell a client to stop
/// offering the method that `amx update apply` exists to find. **W06 wired the
/// row**, so the answer is now behavior — a staged binary that does not exist
/// is refused, by name, with the session untouched (D-M3-6 point 2), and the
/// seam code has become forbidden here the way it already was for M2's twelve.
#[tokio::test]
async fn method_golden_and_skew_arm_cover_session_handoff() {
    let env = Env::new("skew-handoff");
    let mut server = env.server();
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello((PROTO_MIN, PROTO_MAX)).await;

    let method = Method::from_wire_name("session.handoff")
        .expect("session.handoff is in this build's method table");
    let reply = wire
        .request(method.wire_name(), sample_params(method))
        .await;
    if let amx_proto::RpcOutcome::Error(err) = &reply.outcome {
        assert_ne!(
            err.code,
            RpcError::METHOD_NOT_FOUND,
            "the server disowned its own method: {err:?}",
        );
        panic!(
            "a staged binary this session may not be handed to is a reply, not \
             a failed call: {err:?}"
        );
    }
    let refused = result_of(&reply);
    assert_eq!(
        refused["accepted"],
        json!(false),
        "the sample names a binary that does not exist: {refused}",
    );
    let reason = refused["reason"]
        .as_str()
        .expect("a refusal carries its reason");
    assert!(
        reason.contains("amx-from-a-version-that-was-never-built"),
        "a refusal names the binary it refused: {reason}",
    );
    assert!(refused["seq"].is_u64());

    // And the session it was asked to leave is still the session it was.
    let alive = wire.request("ping", json!({})).await;
    assert!(result_of(&alive)["seq"].is_u64());
    assert!(server.alive(), "a refused handoff started nothing");

    drop(server);
}

/// M4's one row, over the wire, on a connection that survives it.
///
/// The other half of the goldens law of `docs/11-m4-plan.md` §3: a method
/// golden freezes the *shape*, and this freezes that the shape is reachable —
/// the table routes `agent.list`, the server owns it, and the whole path from
/// decode to `Core`'s mailbox to a reply channel runs.
///
/// W03's shape, one milestone on. This was written against the seam code, the
/// permitted answer for a tabled row whose wiring had not landed —
/// `METHOD_NOT_FOUND` would tell a client to stop offering the method three of
/// D15's surfaces are built on. **X10 wired the row**, so the answer is now
/// behavior, and the seam code has become forbidden here the way it already was
/// for M2's twelve and M3's one. M4's ledger is empty.
///
/// Both directions, because the sample is deliberately scoped at a workspace no
/// session has: the *scoped* call is refused by name, and an unscoped one on
/// the same connection answers with a list.
#[tokio::test]
async fn method_golden_and_skew_arm_cover_agent_list() {
    let env = Env::new("skew-agents");
    let mut server = env.server();
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello((PROTO_MIN, PROTO_MAX)).await;

    let method =
        Method::from_wire_name("agent.list").expect("agent.list is in this build's method table");
    let reply = wire
        .request(method.wire_name(), sample_params(method))
        .await;
    let err = error_of(&reply);
    assert_ne!(
        err.code,
        RpcError::METHOD_NOT_FOUND,
        "the server disowned its own method: {err:?}",
    );
    assert_ne!(
        err.code, SEAM_CODE,
        "agent.list answers the seam code; M4's ledger is empty and the row \
         owes real behavior: {err:?}",
    );
    assert_eq!(
        err.code,
        RpcError::INVALID_PARAMS,
        "a filter naming a workspace this session does not have is refused, so \
         a caller can tell a stale id from a project with no agents: {err:?}",
    );

    // Unscoped, the same row answers: an empty session has no agents and no
    // queue, and says so with the two fields that are never empty — the seq it
    // was captured at and the server's own wall clock, which is what every
    // surface renders an age against (D-M4-4).
    let unscoped = wire.request(method.wire_name(), json!({})).await;
    let listed = result_of(&unscoped);
    assert!(listed["seq"].is_u64(), "the reply names its seq: {listed}");
    assert!(
        listed["now"]
            .as_u64()
            .is_some_and(|now| now > 1_700_000_000_000),
        "and the server's own now, in epoch milliseconds: {listed}",
    );

    // And the connection, and the session, are exactly what they were.
    let alive = wire.request("ping", json!({})).await;
    assert!(result_of(&alive)["seq"].is_u64());
    assert!(server.alive());

    drop(server);
}
