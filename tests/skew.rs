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
//!
//! # The two things this table does not vary, said plainly (DR-20)
//!
//! A skew harness is worth exactly what it varies, and this one varies one
//! thing: the version *window a client offers*. Two dimensions it does not
//! vary, both of which read as covered if nobody writes them down:
//!
//! - **Protocol version: current against current.** Only version 1 exists, so
//!   [`ROWS`] is that version against itself plus a client-from-the-future.
//!   `the_table_is_current_against_current_until_a_second_version_exists` fails
//!   the day that stops being true, so the label cannot go stale quietly.
//! - **Build: one tree, one binary.** Every process in this file is the binary
//!   under test. The far side of the bridge row can be pointed at another one
//!   with [`FAR_BINARY`], which is how an independently *versioned* far side is
//!   run — but a binary another **machine** built for its own architecture is
//!   not something any test here can produce. m3-live-smoke §5 attached across
//!   two architectures with the far side cross-built *here*, which is the same
//!   tree by another name, and that residual is the M4 exit's smoke step
//!   ([11-m4-plan.md](../docs/11-m4-plan.md) §7), not this suite's.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_proto::control::Method;
use amx_proto::rpc::RpcError;
use amx_proto::version::{COMPAT_WINDOW, PROTO_MAX, PROTO_MIN};
use rig::wire::error_of;
use rig::{Env, Wire, result_of};
use serde_json::{Value, json};

#[path = "skew/rows.rs"]
mod rows;

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

/// An amx to run the whole far side of the bridge row from, instead of the
/// binary under test.
///
/// The bridge row's far side is a *machine*: `amx server` serving the session
/// and `amx _bridge` splicing stdio to it. Both come from this variable when it
/// is set, so pointing it at a differently-versioned build makes the row this
/// tree's client against that build's server — the one dimension of DR-20's
/// first residual that a single machine can supply.
///
/// Unset in CI and unset by default, because a second build is something a
/// person makes on purpose: bump `[workspace.package] version`, `cargo build -p
/// amx`, copy the binary aside, put the version back
/// ([m3-live-smoke.md](../docs/notes/m3-live-smoke.md) §7 step 1 is the same
/// recipe), then run this suite with `AMX_SKEW_FAR_BINARY=<that copy>`.
const FAR_BINARY: &str = "AMX_SKEW_FAR_BINARY";

/// The far side's own binary, or `None` when it is this build.
fn far_binary() -> Option<std::path::PathBuf> {
    std::env::var_os(FAR_BINARY)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

/// What the far side answers `--version` with — this build, unless
/// [`FAR_BINARY`] names another.
///
/// Read by running the binary rather than assumed, because it is the one thing
/// that proves the row ran against the binary it says it did: the version in
/// the `Welcome` came off the process serving the socket, and this came off the
/// file that process was spawned from.
fn far_version() -> String {
    let Some(far) = far_binary() else {
        return env!("CARGO_PKG_VERSION").to_owned();
    };
    let out = std::process::Command::new(&far)
        .arg("--version")
        .output()
        .expect("run the far binary's --version");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .last()
        .expect("a version in `amx --version`")
        .to_owned()
}

/// A command for the far side, carrying `env`'s roots whichever binary it runs.
fn far_command(env: &Env) -> std::process::Command {
    let base = env.command();
    let Some(far) = far_binary() else {
        return base;
    };
    assert!(
        far.is_file(),
        "${FAR_BINARY} names no file: {}",
        far.display()
    );
    let mut command = std::process::Command::new(&far);
    for (key, value) in base.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command
}

/// The far side's session server: [`far_command`] plus `server`.
///
/// The bridge row starts its own rather than taking [`Env::server`]'s, because
/// under [`FAR_BINARY`] the process serving the session has to be the far
/// side's too. A server left running would be inherited by the next test with
/// the same roots, so this ends with the struct that owns it.
struct FarServer {
    child: std::process::Child,
}

impl FarServer {
    fn start(env: &Env) -> Self {
        let child = far_command(env)
            .arg("server")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the far side's amx server");
        let socket = env.socket();
        rig::wait_until("the far side's server to answer its socket", || {
            amx_server::session::probe::probe(&socket).is_ok_and(|p| p.is_running())
        });
        Self { child }
    }
}

impl Drop for FarServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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
        // M4's one. Scoped at a workspace that does not exist, so the row has
        // to route and answer without an agent, a pane or a workspace behind
        // it — the point of a skew row is that the *table* routes.
        Method::AgentList => json!({ "workspace": bogus_workspace }),
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
///
/// **Same-build**, equally honestly, unless [`FAR_BINARY`] says otherwise: the
/// server and the splice below are the binary under test until that variable
/// names another one. The module header says which residual each of those two
/// labels belongs to.
#[tokio::test]
async fn every_skew_sample_row_answers_over_the_bridge_transport() {
    let env = Env::new("skew-bridge");
    let server = FarServer::start(&env);

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
        // Which build answered, checked rather than assumed: with no
        // `$AMX_SKEW_FAR_BINARY` this is the tautology that keeps the label
        // honest, and with one it is the whole claim — the session on the other
        // side of the splice is served by *that* binary.
        assert_eq!(
            welcome.server.version,
            far_version(),
            "{}: the far side is not the binary it was spawned from",
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
/// would have handed the same process. Which amx runs is [`far_command`]'s
/// answer, so a differently-built far side splices as well as serves.
fn bridge_child(env: &Env) -> (std::process::Child, tokio::net::UnixStream) {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::process::Stdio;

    let (mine, theirs) = StdUnixStream::pair().expect("socketpair");
    let stdin = OwnedFd::from(theirs.try_clone().expect("dup the bridge socket"));
    let stdout = OwnedFd::from(theirs);
    let child = far_command(env)
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

/// The label's tripwire: it says current-against-current, and this fails when
/// that stops being the truth (DR-20).
///
/// A label is worth nothing if the thing it describes can change underneath it,
/// and this one describes a table of two rows that both offer the same version.
/// The day a second protocol version lands, [`ROWS`] owes a
/// current-against-previous row and three doc comments owe a rewrite — and this
/// is what says so, at the moment it becomes true, rather than a reader
/// noticing later that the harness never ran the case it is named for.
#[test]
fn the_table_is_current_against_current_until_a_second_version_exists() {
    assert_eq!(
        PROTO_MIN, PROTO_MAX,
        "a second protocol version exists, so this table is no longer \
         current-against-current by construction: add the row that offers \
         ({PROTO_MIN}, {PROTO_MIN}) against this server to ROWS, and rewrite \
         the labels on this module, on the bridge row and on \
         tests/handoff_exit.rs that call the coverage current-vs-current",
    );
    assert!(
        ROWS.iter().all(|row| row.client.0 == PROTO_MIN),
        "every row offers the one version there is",
    );
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
