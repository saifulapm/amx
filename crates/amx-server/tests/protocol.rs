//! T10: what the socket speaks — Hello/Welcome negotiation over a real socket,
//! the N/N−1 compatibility window, and control dispatch through to the `Core`.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_proto::control::{session, workspace};
use amx_proto::version::{COMPAT_WINDOW, PROTO_MAX, PROTO_MIN, window};
use amx_proto::{Feature, RequestId, RpcError, RpcOutcome};
use serde_json::json;

mod support;

use support::{Server, UNKNOWN_FEATURE, result_of};

#[tokio::test]
async fn hello_welcome_round_trip_over_the_real_socket() {
    let server = Server::start("hello").await;
    let mut client = server.connect().await;

    let welcome = client.hello(window()).await;
    assert_eq!(welcome.proto, PROTO_MAX);
    assert!(
        welcome.features.contains(&Feature::GRID_STREAM),
        "a feature both sides have must survive the intersection"
    );
    assert!(
        !welcome.features.contains(&Feature::named(UNKNOWN_FEATURE)),
        "a feature only the client has must be dropped, not rejected"
    );

    // The identity in the welcome is the identity every later reply carries:
    // the welcome is built from the same `ping` the client can call itself.
    let reply = client.request(1, "ping", json!({})).await;
    let ping: session::PingReply =
        serde_json::from_value(result_of(&reply).clone()).expect("a ping reply");
    assert_eq!(ping.session, welcome.session);
    assert!(ping.seq >= welcome.seq, "the bus sequence never goes back");

    server.shutdown().await;
}

#[tokio::test]
async fn server_accepts_previous_protocol_version() {
    let server = Server::start("skew").await;

    // The oldest version inside the compatibility window. While only one
    // version exists this is current-against-current, as the plan states.
    let oldest = PROTO_MAX.saturating_sub(COMPAT_WINDOW - 1).max(PROTO_MIN);
    let mut old = server.connect().await;
    let welcome = old.hello((oldest, PROTO_MAX)).await;
    assert_eq!(
        welcome.proto, PROTO_MAX,
        "the highest version both sides speak wins"
    );

    // A client newer than this build offers a window running past PROTO_MAX;
    // it must be answered with PROTO_MAX rather than refused.
    let mut new = server.connect().await;
    let welcome = new.hello((PROTO_MIN, PROTO_MAX + 1)).await;
    assert_eq!(welcome.proto, PROTO_MAX);

    server.shutdown().await;
}

#[tokio::test]
async fn a_control_call_reaches_the_core_and_comes_back() {
    let server = Server::start("dispatch").await;
    let mut client = server.attach().await;

    let reply = client
        .request(7, "workspace.create", json!({ "label": "work" }))
        .await;
    let created: workspace::CreateReply =
        serde_json::from_value(result_of(&reply).clone()).expect("a create reply");
    assert_eq!(created.short.get(), 1);
    assert_eq!(reply.id, RequestId::Number(7));

    server.shutdown().await;
}

#[tokio::test]
async fn an_unknown_method_is_a_reply_not_a_disconnect() {
    let server = Server::start("skewmethod").await;
    let mut client = server.attach().await;

    let reply = client.request(1, "pane.teleport", json!({})).await;
    let RpcOutcome::Error(err) = &reply.outcome else {
        panic!("an unknown method must fail");
    };
    assert_eq!(err.code, RpcError::METHOD_NOT_FOUND);

    // The connection is still usable, which is the whole point of answering
    // rather than dropping: a peer built against another revision of the
    // method table keeps its session.
    let reply = client.request(2, "ping", json!({})).await;
    assert!(matches!(reply.outcome, RpcOutcome::Result(_)));

    server.shutdown().await;
}

#[tokio::test]
async fn a_call_naming_a_pane_that_does_not_exist_is_a_reply_not_a_disconnect() {
    let server = Server::start("noexist").await;
    let mut client = server.attach().await;

    let pane = amx_core::PaneId::new_v4().to_string();
    let reply = client
        .request(1, "pane.close", json!({ "pane": pane }))
        .await;
    let RpcOutcome::Error(err) = &reply.outcome else {
        panic!("closing a pane nobody has is not success");
    };
    assert_eq!(err.code, RpcError::INVALID_PARAMS);

    // The connection is still alive: a well-formed call the `Core` rejected on
    // its merits, not a reason to drop the peer.
    let reply = client.request(2, "ping", json!({})).await;
    assert!(matches!(reply.outcome, RpcOutcome::Result(_)));

    server.shutdown().await;
}

#[tokio::test]
async fn session_report_answers_not_implemented_not_404() {
    // The row is in the table both peers share, so reporting it as unknown
    // would tell a client to stop offering it — the distinction
    // `dispatch::NOT_IMPLEMENTED` exists for. This covered `pane.rename` too
    // until U07 implemented it (`tests/rename.rs` is what covers it now);
    // U06 replaces this answer in the same way, and then the whole test goes.
    let server = Server::start("seams").await;
    let mut client = server.attach().await;

    let method = "session.report";
    let reply = client.request(1, method, json!({})).await;
    let RpcOutcome::Error(err) = &reply.outcome else {
        panic!("{method} is a seam in this build, not an implemented method");
    };
    assert_eq!(
        err.code,
        amx_server::dispatch::NOT_IMPLEMENTED,
        "{method} answered {}, not the seam code",
        err.code
    );
    assert_ne!(
        err.code,
        RpcError::METHOD_NOT_FOUND,
        "{method} is in the table; disowning it would tell the client to stop offering it"
    );
    assert!(err.message.contains(method), "{}", err.message);

    // The connection survives it, like every other refusal here.
    let reply = client.request(2, "ping", json!({})).await;
    assert!(matches!(reply.outcome, RpcOutcome::Result(_)));

    server.shutdown().await;
}

#[tokio::test]
async fn bad_parameters_are_a_reply_not_a_disconnect() {
    let server = Server::start("badparams").await;
    let mut client = server.attach().await;

    let reply = client
        .request(1, "workspace.rename", json!({ "workspace": "not-a-uuid" }))
        .await;
    let RpcOutcome::Error(err) = &reply.outcome else {
        panic!("parameters that do not fit must fail");
    };
    assert_eq!(err.code, RpcError::INVALID_PARAMS);

    let reply = client.request(2, "ping", json!({})).await;
    assert!(matches!(reply.outcome, RpcOutcome::Result(_)));

    server.shutdown().await;
}
