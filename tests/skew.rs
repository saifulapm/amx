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
    }
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
