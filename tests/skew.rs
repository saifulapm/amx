//! The N/N−1 skew harness (04 §4, D3).
//!
//! One conformance run per table row, each row a client version window driven
//! against the real server binary over the real socket. While only protocol
//! version 1 exists the table is current-against-current plus a
//! client-from-the-future row — and adding a version is adding a row, nothing
//! else.
//!
//! "Fails on an unhandled variant" is the run's teeth, in both directions a
//! peer can be newer: a method this server never heard of must come back
//! `METHOD_NOT_FOUND` on a connection that stays up, an unknown notification
//! must be ignored, unknown `Hello` fields and features must be dropped — and
//! every method this build *does* table must answer, whatever the reply. Any
//! of those turning into a disconnect fails the row, because a dropped
//! connection is exactly how skew intolerance would present.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_proto::control::Method;
use amx_proto::rpc::RpcError;
use amx_proto::version::{COMPAT_WINDOW, PROTO_MAX, PROTO_MIN};
use rig::wire::error_of;
use rig::{Env, Wire, result_of};
use serde_json::{Value, json};

/// One row of the skew table: a peer window against the current server.
struct Row {
    /// What the row proves.
    name: &'static str,
    /// The version window the client offers.
    client: (u16, u16),
    /// The version the server must pick.
    expect: u16,
}

/// The table. A second protocol version lands here as a second
/// current-against-previous row; nothing else in this file changes.
const ROWS: &[Row] = &[
    Row {
        name: "current against current",
        client: (PROTO_MIN, PROTO_MAX),
        expect: PROTO_MAX,
    },
    Row {
        name: "a client one version ahead",
        client: (PROTO_MIN, PROTO_MAX + 1),
        expect: PROTO_MAX,
    },
];

/// Example params for every method in the table.
///
/// Exhaustive over [`Method`] on purpose: a new method refuses to compile
/// until this harness knows how to call it, which is the "adding a version is
/// adding a row" property applied to the method table.
fn sample_params(method: Method) -> Value {
    let bogus_pane = "00000000-0000-0000-0000-00000000dead";
    let bogus_workspace = "00000000-0000-0000-0000-00000000beef";
    match method {
        Method::Ping => json!({}),
        Method::WorkspaceCreate => json!({ "focus": false }),
        Method::WorkspaceRename => json!({ "workspace": bogus_workspace, "label": "renamed" }),
        Method::WorkspaceKill => json!({ "workspace": bogus_workspace }),
        Method::WorkspaceSwitch => json!({ "workspace": bogus_workspace }),
        Method::PaneSplit => json!({ "pane": bogus_pane, "direction": "vertical" }),
        Method::PaneZoom => json!({ "pane": bogus_pane }),
        Method::PaneSwap => json!({ "pane": bogus_pane, "with": bogus_pane }),
        Method::PaneMove => json!({ "pane": bogus_pane, "to": bogus_workspace }),
        Method::PaneClose => json!({ "pane": bogus_pane }),
        Method::PaneFocus => json!({ "workspace": bogus_workspace, "direction": "left" }),
        Method::PaneResize => {
            json!({ "pane": bogus_pane, "direction": "right", "delta": 0.0625 })
        }
        Method::PaneRename => json!({ "pane": bogus_pane, "label": "renamed" }),
        Method::SessionState => json!({}),
        Method::SessionReport => json!({}),
        Method::StreamBind => json!({ "kind": "pane_grid", "pane": bogus_pane }),
        Method::PaneHistory => {
            json!({ "pane": bogus_pane, "first": 0, "last": 0, "request": 1 })
        }
        Method::ClientViewport => json!({ "rows": 24, "cols": 80, "panes": [] }),
        // M2's twelve. Every target names a pane that does not exist, so each
        // call reaches its handler and comes back with a defined failure — the
        // point of the row is that the *table* routes, not that the pane does.
        Method::AgentReport => json!({
            "pane": bogus_pane,
            "token": "not-the-token-this-pane-was-spawned-with",
            "agent": "claude",
            "source": "amx:claude",
            "event": "UserPromptSubmit",
            "seq": 1_754_524_800_123_456_789u64,
        }),
        Method::AgentStart => json!({ "name": "skew", "kind": "claude" }),
        Method::AgentPrompt => json!({ "target": bogus_pane, "text": "hello" }),
        Method::AgentExplain => json!({ "target": bogus_pane }),
        Method::AgentNext => json!({}),
        // Every long-poll gets a timeout, because a skew row that waited
        // indefinitely for a status no pane will ever reach would hang the
        // harness rather than fail it.
        Method::Wait => {
            json!({ "until": "idle", "target": bogus_pane, "timeout_ms": 1 })
        }
        Method::EventsSubscribe => json!({}),
        Method::PaneSendText => json!({ "target": bogus_pane, "text": "hi" }),
        Method::PaneSendKeys => json!({ "target": bogus_pane, "keys": ["ctrl+c"] }),
        Method::PaneRun => json!({ "target": bogus_pane, "text": "true" }),
        Method::PaneRead => json!({ "target": bogus_pane }),
        Method::PaneWaitOutput => {
            json!({ "target": bogus_pane, "match": "never", "timeout_ms": 1 })
        }
        // M3's one. The binary does not exist, which is the point: the row has
        // to route and answer, and a handoff that actually started would take
        // the harness's own server with it.
        Method::SessionHandoff => json!({
            "binary": "/nonexistent/amx-from-a-version-that-was-never-built",
            "timeout_ms": 1,
        }),
    }
}

/// The twelve rows M2 added, by wire name.
///
/// Named rather than derived so the harness states what it is covering: a
/// thirteenth row would compile (the exhaustive match above is what catches
/// that) but would not be claimed here, and the assertion below counts.
/// The code a dispatch seam answers with while its wiring is being built.
///
/// `-32000`, inside JSON-RPC 2.0's implementation-defined server error range.
/// Deliberately a literal rather than an import: the assertions below have to
/// keep meaning what they mean across the milestones where nothing in the tree
/// defines it at all. V17 deleted M2's helper and W03 wrote M3's, in
/// `dispatch/session.rs`, for `session.handoff` alone.
///
/// Two tests read it, in opposite directions:
/// [`skew_calls_every_m2_row_and_none_is_method_not_found`] asserts no M2 row
/// answers it — that ledger is closed and stays closed — and
/// [`method_golden_and_skew_arm_cover_session_handoff`] asserts M3's one row
/// *does*, until W06 lands.
const RETIRED_SEAM: i32 = -32000;

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
        // `NOT_IMPLEMENTED` (-32000) was the *permitted* answer while M2 was
        // being built: a tabled row whose wiring had not landed answered
        // through the `seam` helper rather than disowning itself. V17 closed
        // the last two seams and deleted the helper, so nothing in the tree can
        // produce this code any more — and asserting that here is the wire-side
        // half of the ledger `tests/hygiene.rs` keeps at the source level. A row
        // that starts answering it again has been un-implemented.
        if let amx_proto::RpcOutcome::Error(err) = &reply.outcome {
            assert_ne!(
                err.code,
                RpcError::METHOD_NOT_FOUND,
                "the server disowned its own method {name}",
            );
            assert_ne!(
                err.code, RETIRED_SEAM,
                "{name} answers the retired seam code; M2's ledger is empty and \
                 every row of docs/08-m2-plan.md §4 owes real behavior",
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

#[tokio::test]
async fn skew_harness_runs_current_against_current_and_fails_on_an_unhandled_variant() {
    let env = Env::new("skew");
    let server = env.server();

    for row in ROWS {
        let mut wire = Wire::connect(&env.socket()).await;

        // Negotiation picks the highest common version, not equality.
        let welcome = wire.hello(row.client).await;
        assert_eq!(
            welcome.proto, row.expect,
            "{}: negotiated the wrong version",
            row.name
        );

        // Every method this build tables answers — with a result or a defined
        // error — and never by dropping the connection.
        for &method in Method::ALL {
            let reply = wire
                .request(method.wire_name(), sample_params(method))
                .await;
            if let amx_proto::RpcOutcome::Error(err) = &reply.outcome {
                assert_ne!(
                    err.code,
                    RpcError::METHOD_NOT_FOUND,
                    "{}: the server disowned its own method {}",
                    row.name,
                    method.wire_name()
                );
            }
        }

        // A method from the future: refused softly, on a connection that
        // stays up. This is the unhandled-variant probe — a server that
        // disconnected here would fail the read below.
        let future = wire.request("pane.teleport", json!({})).await;
        assert_eq!(
            error_of(&future).code,
            RpcError::METHOD_NOT_FOUND,
            "{}: an unknown method must be METHOD_NOT_FOUND",
            row.name
        );

        // A notification from the future: dropped, not fatal.
        wire.send_control(
            &serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "method": "amx.future/burst",
                "params": { "novel": true },
            }))
            .expect("encode the notification"),
        )
        .await;

        // The connection survived all of it.
        let alive = wire.request("ping", json!({})).await;
        assert!(
            result_of(&alive)["seq"].is_u64(),
            "{}: the connection should still answer",
            row.name
        );
    }

    drop(server);
}

#[tokio::test]
async fn unknown_hello_fields_and_features_are_dropped_not_fatal() {
    let env = Env::new("skew-hello");
    let server = env.server();

    let mut wire = Wire::connect(&env.socket()).await;
    let welcome = wire
        .hello_value(&json!({
            "proto": [PROTO_MIN, PROTO_MAX],
            "features": ["grid.stream", "amx.test.feature-from-the-future"],
            "client": {
                "name": "amx-rig",
                "version": "0.0.0",
                "field_from_the_future": { "nested": [1, 2, 3] },
            },
            "top_level_field_from_the_future": "ignored by contract",
        }))
        .await;

    assert_eq!(welcome.proto, PROTO_MAX);
    let features: Vec<&str> = welcome.features.iter().map(|f| f.as_str()).collect();
    assert!(
        features.contains(&"grid.stream"),
        "the known feature survives the intersection: {features:?}"
    );
    assert!(
        !features.iter().any(|f| f.contains("future")),
        "an unknown feature must not be echoed: {features:?}"
    );

    drop(server);
}

#[tokio::test]
async fn a_peer_with_no_common_version_is_refused_without_a_welcome() {
    let env = Env::new("skew-disjoint");
    let mut server = env.server();

    let mut wire = Wire::connect(&env.socket()).await;
    let hello = json!({
        "proto": [PROTO_MAX + 1, PROTO_MAX + 2],
        "client": { "name": "amx-rig", "version": "0.0.0" },
    });
    wire.send_control(&serde_json::to_vec(&hello).expect("encode hello"))
        .await;
    assert!(
        wire.closed_by_server().await,
        "a disjoint window is the one negotiation that cannot degrade; the \
         server must close rather than welcome"
    );

    // And refusing one peer costs nobody else anything.
    let mut next = Wire::connect(&env.socket()).await;
    let welcome = next.hello((PROTO_MIN, PROTO_MAX)).await;
    assert_eq!(welcome.proto, PROTO_MAX);
    assert!(server.alive(), "the server itself is untouched");

    drop(server);
}

/// The second transport the skew table owes a run over (D-M3-9, §4's law).
///
/// A remote session is the same protocol over `ssh host exec amx _bridge`
/// stdio, so "the skew window is honored remotely" is a claim about *this*
/// table answering over *that* transport. W03 planted the row as a tripwire
/// against the stub's refusal; W11 wrote the splice, which turned the tripwire
/// red, and this is the finished row it demanded.
///
/// Only the socketpair stands where ssh would. That is the whole substitution:
/// ssh moves stdio between two machines, a socketpair moves it between two
/// processes, and every byte either side of it is amx's. What runs over it is
/// the same `for &method in Method::ALL` loop the socket rows run, against the
/// same [`sample_params`] table, so a new method joins this transport's
/// coverage by being added to the table once.
///
/// **Current-vs-current**, honestly labeled: only protocol version 1 exists, so
/// this proves the bridge negotiates and answers, not that it has been tested
/// across versions. It inherits that limit from the M0 harness above, and a
/// second version lands here as a second row in [`ROWS`] and nothing else.
#[tokio::test]
async fn every_skew_sample_row_answers_over_the_bridge_transport() {
    let env = Env::new("skew-bridge");
    let server = env.server();

    for row in ROWS {
        // Spawned exactly as ssh would run it — `amx _bridge`, stdio and
        // nothing else — so what this exercises is the real argv the far side
        // of an `ssh host exec amx _bridge` receives.
        let (mut child, local) = bridge_child(&env);
        let mut wire = Wire::over(local);

        let welcome = wire.hello(row.client).await;
        assert_eq!(
            welcome.proto, row.expect,
            "{}: the bridge negotiated the wrong version",
            row.name
        );

        for &method in Method::ALL {
            let reply = wire
                .request(method.wire_name(), sample_params(method))
                .await;
            if let amx_proto::RpcOutcome::Error(err) = &reply.outcome {
                assert_ne!(
                    err.code,
                    RpcError::METHOD_NOT_FOUND,
                    "{}: the server disowned its own method {} over the bridge",
                    row.name,
                    method.wire_name()
                );
            }
        }

        // A method from the future, refused softly on a connection that stays
        // up: a splice that dropped bytes or desynced the framing would show
        // here as a dead connection rather than as an error code.
        let future = wire.request("pane.teleport", json!({})).await;
        assert_eq!(
            error_of(&future).code,
            RpcError::METHOD_NOT_FOUND,
            "{}: an unknown method must be METHOD_NOT_FOUND over the bridge",
            row.name
        );

        let alive = wire.request("ping", json!({})).await;
        assert!(
            result_of(&alive)["seq"].is_u64(),
            "{}: the bridged connection should still answer",
            row.name
        );

        // The splice ends with its client, and it ends cleanly.
        drop(wire);
        rig::wait_until("the bridge child to exit", || {
            child.try_wait().expect("try_wait").is_some()
        });
        let status = child.wait().expect("reap the bridge");
        assert!(
            status.success(),
            "{}: the splice exited {status:?}",
            row.name
        );
    }

    // And the session every row was pointed at is the session it was.
    let mut wire = Wire::connect(&env.socket()).await;
    let welcome = wire.hello((PROTO_MIN, PROTO_MAX)).await;
    assert_eq!(welcome.proto, PROTO_MAX);

    drop(server);
}

/// Spawn `amx _bridge` with a socketpair as its stdin and stdout.
///
/// The one place ssh is replaced, and it is replaced by the two descriptors ssh
/// would have handed the same process.
fn bridge_child(env: &Env) -> (std::process::Child, tokio::net::UnixStream) {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::process::Stdio;

    let (mine, theirs) = StdUnixStream::pair().expect("socketpair");
    let stdin = OwnedFd::from(theirs.try_clone().expect("dup the bridge socket"));
    let stdout = OwnedFd::from(theirs);
    let child = env
        .command()
        .arg("_bridge")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn amx _bridge");
    mine.set_nonblocking(true).expect("non-blocking");
    let local = tokio::net::UnixStream::from_std(mine).expect("adopt the bridge socket");
    (child, local)
}

#[test]
fn the_compatibility_window_promise_holds() {
    // 04 §4: "server supports current and previous protocol version, minimum".
    // While only v1 exists the window is degenerate; this pins the promise so
    // shrinking it is a loud change.
    const {
        assert!(COMPAT_WINDOW >= 2, "the N/N-1 window must stay at least 2");
        assert!(PROTO_MIN <= PROTO_MAX);
    }
    assert_eq!(
        amx_proto::version::window(),
        (PROTO_MIN, PROTO_MAX),
        "the offered window is the whole supported range"
    );
}
