//! T14 acceptance: the modal input layer — prefix, navigate, kitty
//! passthrough, mouse forwarding — plus the two wire methods it grew
//! (`pane.focus`, `pane.resize`) end to end against a real server.
//!
//! Each test drives `App::handle_input` with raw bytes, exactly the shape a
//! stdin read hands it, and observes the machine only through its outward
//! effects: bytes addressed at a pane, control calls, mode, and the client's
//! local focus. The layout the client navigates over is adopted with locally
//! minted pane ids, the same stand-in `tests/attach.rs` documents (no wire
//! path delivers the server's layout as state yet); the wire round-trip test
//! at the bottom uses none of that and speaks to the real `Core` throughout.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::{App, Mode};
use amx_client::input::{InputEvent, PREFIX, RESIZE_STEP};
use amx_client::model::WorkspaceModel;
use amx_core::{Direction, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::{Call, pane as pane_proto};
use serde_json::json;

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-input-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// An owned copy of one sink event, so tests can assert on the whole batch.
#[derive(Clone, PartialEq, Debug)]
enum Ev {
    Fwd(PaneId, Vec<u8>),
    Call(Call),
    Detach,
}

fn drive(app: &mut TestApp, bytes: &[u8]) -> Vec<Ev> {
    let mut evs = Vec::new();
    app.handle_input(bytes, &mut |event| match event {
        InputEvent::Forward { pane, bytes } => evs.push(Ev::Fwd(pane, bytes.to_vec())),
        InputEvent::Call(call) => evs.push(Ev::Call(call)),
        InputEvent::Detach => evs.push(Ev::Detach),
    });
    evs
}

/// A real attached client over a two-pane mirrored layout: `left | right`.
struct Fixture {
    server: support::Server,
    _master: std::fs::File,
    app: TestApp,
    ws: WorkspaceId,
    left: PaneId,
    right: PaneId,
}

async fn fixture(tag: &str) -> Fixture {
    let server = support::Server::start(tag).await;
    let pty = support::open_pty();
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");
    let reply = app
        .session()
        .call("workspace.create", json!({}))
        .await
        .expect("workspace.create round trip");
    let ws: WorkspaceId =
        serde_json::from_value(reply["workspace"].clone()).expect("decode the workspace id");

    let left = PaneId::new_v4();
    let right = PaneId::new_v4();
    let mut layout = BspLayout::with_root(left);
    layout
        .split(left, Direction::Right, right, 0.5)
        .expect("split the mirrored layout");
    app.adopt_workspace(
        ws,
        WorkspaceModel {
            label: None,
            layout,
        },
    );
    // The attach already folded and focused the server's seeded workspace;
    // these tests navigate their own mirror instead.
    app.model().focus_workspace(ws);

    Fixture {
        server,
        _master: pty.master,
        app,
        ws,
        left,
        right,
    }
}

#[tokio::test]
async fn prefix_key_is_intercepted_and_not_forwarded() {
    let mut fx = fixture("prefix").await;
    let app = &mut fx.app;
    let left = fx.left;

    let evs = drive(app, b"ab\x01");
    assert_eq!(
        evs,
        vec![Ev::Fwd(left, b"ab".to_vec())],
        "bytes before the prefix forward; the prefix byte itself never does"
    );
    assert_eq!(app.mode(), Mode::Prefix);

    // An unknown one-shot key is swallowed with the mode.
    let evs = drive(app, b"q");
    assert!(evs.is_empty(), "unknown prefix key is swallowed: {evs:?}");
    assert_eq!(app.mode(), Mode::Terminal);

    // The deliberate escape hatch: prefix twice sends the literal byte.
    let evs = drive(app, &[PREFIX, PREFIX]);
    assert_eq!(evs, vec![Ev::Fwd(left, vec![PREFIX])]);
    assert_eq!(app.mode(), Mode::Terminal);

    fx.server.shutdown().await;
}

#[tokio::test]
async fn unrecognised_kitty_sequence_is_forwarded_byte_identical() {
    let mut fx = fixture("kitty").await;
    let app = &mut fx.app;
    let left = fx.left;

    // Kitty-encoded ctrl+a: the prefix in kitty clothing must not prefix —
    // only the literal byte does. The whole sequence passes through untouched.
    let seq = b"\x1b[97;5u";
    let evs = drive(app, seq);
    assert_eq!(evs, vec![Ev::Fwd(left, seq.to_vec())]);
    assert_eq!(app.mode(), Mode::Terminal);

    // Split across two reads it still arrives byte-identical, just chunked.
    let evs_a = drive(app, b"\x1b[105;");
    let evs_b = drive(app, b"9u");
    let mut forwarded = Vec::new();
    for ev in evs_a.iter().chain(&evs_b) {
        let Ev::Fwd(pane, bytes) = ev else {
            panic!("a key sequence must never become anything but bytes: {ev:?}");
        };
        assert_eq!(*pane, left);
        forwarded.extend_from_slice(bytes);
    }
    assert_eq!(forwarded, b"\x1b[105;9u");

    fx.server.shutdown().await;
}

#[tokio::test]
async fn navigate_mode_is_sticky_until_esc() {
    let mut fx = fixture("sticky").await;
    let app = &mut fx.app;

    drive(app, b"\x01w");
    assert_eq!(app.mode(), Mode::Navigate);

    drive(app, b"l");
    assert_eq!(
        app.mode(),
        Mode::Navigate,
        "one verb does not end the layer"
    );
    drive(app, b"h");
    assert_eq!(app.mode(), Mode::Navigate);

    drive(app, b"\x1b");
    assert_eq!(app.mode(), Mode::Terminal);

    // Back in terminal mode the same key is bytes again, not a verb.
    let evs = drive(app, b"x");
    assert_eq!(evs, vec![Ev::Fwd(fx.left, b"x".to_vec())]);

    fx.server.shutdown().await;
}

#[tokio::test]
#[allow(non_snake_case, reason = "the acceptance name in docs/06-m0-plan.md")]
async fn hjkl_moves_focus_and_HJKL_resizes() {
    let mut fx = fixture("hjkl").await;
    let ws = fx.ws;
    let app = &mut fx.app;

    assert_eq!(
        app.focused_pane(),
        Some(fx.left),
        "focus seeds to the first pane"
    );
    drive(app, b"\x01w");

    // `l`: focus moves locally and the movement is echoed to the server.
    let evs = drive(app, b"l");
    assert_eq!(app.focused_pane(), Some(fx.right));
    assert_eq!(
        evs,
        vec![Ev::Call(Call::PaneFocus(pane_proto::FocusParams {
            workspace: ws,
            direction: pane_proto::MoveDirection::Right,
        }))]
    );

    // At the right edge `l` finds no neighbour: focus stays, the echo still
    // goes out (the server's focus is canonical, not the mirror's guess).
    let evs = drive(app, b"l");
    assert_eq!(app.focused_pane(), Some(fx.right));
    assert_eq!(evs.len(), 1);

    // `L`: one resize call, no local focus change, and — the point — the
    // layout mirror is untouched: it is server truth, so the new rects
    // arrive as state, they are not guessed at locally.
    let area = app.model().content_area();
    let rects_before = app.model().pane_rects(area);
    let evs = drive(app, b"L");
    assert_eq!(
        evs,
        vec![Ev::Call(Call::PaneResize(pane_proto::ResizeParams {
            pane: fx.right,
            direction: pane_proto::MoveDirection::Right,
            delta: RESIZE_STEP,
        }))]
    );
    assert_eq!(app.focused_pane(), Some(fx.right));
    assert_eq!(
        app.model().pane_rects(area),
        rects_before,
        "HJKL must not mutate the client's layout mirror"
    );

    fx.server.shutdown().await;
}

#[tokio::test]
async fn sgr_mouse_event_is_forwarded_only_when_the_pane_enabled_reporting() {
    let mut fx = fixture("mouse").await;
    let app = &mut fx.app;
    let left = fx.left;
    let press = b"\x1b[<0;4;3M";

    let evs = drive(app, press);
    assert!(
        evs.is_empty(),
        "reporting off: the report is dropped, not leaked: {evs:?}"
    );

    app.input().set_mouse_reporting(left, true);
    let evs = drive(app, press);
    assert_eq!(evs, vec![Ev::Fwd(left, press.to_vec())]);

    // A report split across two reads is reassembled and still gated whole.
    let evs_a = drive(app, b"\x1b[<0;4");
    assert!(evs_a.is_empty());
    let evs_b = drive(app, b";3m");
    assert_eq!(evs_b, vec![Ev::Fwd(left, b"\x1b[<0;4;3m".to_vec())]);

    // ... and dropped whole once reporting is off again.
    app.input().set_mouse_reporting(left, false);
    drive(app, b"\x1b[<0;4");
    let evs = drive(app, b";3m");
    assert!(evs.is_empty());

    fx.server.shutdown().await;
}

#[tokio::test]
async fn chrome_never_interprets_a_mouse_event() {
    let mut fx = fixture("chrome-mouse").await;
    let app = &mut fx.app;

    drive(app, b"\x01w");
    assert_eq!(app.mode(), Mode::Navigate);
    // Even a pane that enabled reporting gets nothing while chrome owns the
    // keys: forwarding from inside a chrome mode would be interpretation.
    app.input().set_mouse_reporting(fx.left, true);

    let repaints = app.repaints;
    // A report whose payload bytes spell navigate verbs if misread: `2`
    // would jump, `m` would move the pane, `M` variants likewise.
    let evs = drive(app, b"\x1b[<0;2;2M\x1b[<0;2;2m");
    assert!(evs.is_empty(), "chrome saw a mouse event: {evs:?}");
    assert_eq!(
        app.mode(),
        Mode::Navigate,
        "Esc inside a report must not exit"
    );
    assert_eq!(
        app.focused_pane(),
        Some(fx.left),
        "digits must not jump focus"
    );
    assert_eq!(app.repaints, repaints);

    fx.server.shutdown().await;
}

#[tokio::test]
async fn split_keys_reach_the_server_as_one_method_call_each() {
    let mut fx = fixture("split-keys").await;
    let app = &mut fx.app;

    drive(app, b"\x01w");
    let evs = drive(app, b"x");
    assert_eq!(
        evs,
        vec![Ev::Call(Call::PaneSplit(pane_proto::SplitParams {
            pane: fx.left,
            direction: pane_proto::SplitDirection::Horizontal,
            command: None,
            cwd: None,
        }))],
        "`x` is exactly one pane.split, nothing else"
    );
    let evs = drive(app, b"v");
    let [Ev::Call(call)] = evs.as_slice() else {
        panic!("`v` must emit exactly one call: {evs:?}");
    };
    let Call::PaneSplit(params) = call else {
        panic!("`v` must be a pane.split: {call:?}");
    };
    assert_eq!(params.direction, pane_proto::SplitDirection::Vertical);

    // The emitted call goes over the real socket as that one method call:
    // the mirrored pane id is unknown to the server, so the real dispatch
    // answers INVALID_PARAMS — reaching a handler, not METHOD_NOT_FOUND.
    let outcome = app
        .session()
        .call(
            call.method().wire_name(),
            call.params().expect("params encode"),
        )
        .await;
    let Err(amx_client::net::NetError::Call(err)) = outcome else {
        panic!("expected the server to refuse the unknown pane: {outcome:?}");
    };
    assert_eq!(err.code, amx_proto::RpcError::INVALID_PARAMS);

    fx.server.shutdown().await;
}

#[tokio::test]
async fn pane_focus_and_pane_resize_round_trip_the_wire() {
    let server = support::Server::start("focus-wire").await;
    let pty = support::open_pty();
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach");
    let reply = app
        .session()
        .call("workspace.create", json!({}))
        .await
        .expect("workspace.create");
    let ws: WorkspaceId = serde_json::from_value(reply["workspace"].clone()).expect("workspace id");

    // The reply names the landing pane, which is how a client learns the
    // server-side focus it must not diverge from — here, the root pane.
    let focused = app
        .session()
        .call("pane.focus", json!({"workspace": ws, "direction": "left"}))
        .await
        .expect("pane.focus round trip");
    let root: PaneId =
        serde_json::from_value(focused["pane"].clone()).expect("a focused pane exists");

    // A real second pane, so both verbs have geometry to act on.
    let split = app
        .session()
        .call(
            "pane.split",
            json!({"pane": root, "direction": "vertical", "command": ["sleep", "60"]}),
        )
        .await
        .expect("pane.split round trip");
    assert!(split["pane"].is_string());

    // The split focused the new pane; moving left lands back on the root.
    let focused = app
        .session()
        .call("pane.focus", json!({"workspace": ws, "direction": "left"}))
        .await
        .expect("pane.focus round trip");
    assert_eq!(
        serde_json::from_value::<Option<PaneId>>(focused["pane"].clone()).expect("pane"),
        Some(root)
    );

    let resized = app
        .session()
        .call(
            "pane.resize",
            json!({"pane": root, "direction": "right", "delta": 0.25}),
        )
        .await
        .expect("pane.resize round trip");
    assert_eq!(resized["resized"], true);

    server.shutdown().await;
}
