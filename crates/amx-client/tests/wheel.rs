//! D14's wheel exception, end to end through the real input machine: a wheel
//! turn in a pane that asked for no mouse reports scrolls the client's own
//! scrollback cache, and a wheel-down at the live edge gives the pane back.
//!
//! The fence is asserted here as hard as the behaviour is. Every test that
//! feeds a report feeds one with coordinates that would be nonsense if anything
//! read them, and the last test feeds four reports whose coordinates disagree
//! wildly and asserts they do the same thing — because "no column or row is
//! parsed on this path" is a property, and a property is worth a test rather
//! than a comment.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::{App, Mode};
use amx_client::input::InputEvent;
use amx_core::{PaneId, RowId, RowRange};
use amx_proto::ClientInfo;

type TestApp = App<std::fs::File, Vec<u8>>;

/// How many rows one wheel click moves the view. The engine's own constant is
/// private; this is the number the tests are written against, and a change to
/// one without the other is what these assertions catch.
const NOTCH: u64 = 3;

/// The newest row the fixture's cache holds.
const NEWEST: u64 = 99;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-wheel-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

struct Fixture {
    server: support::Server,
    _master: std::fs::File,
    app: TestApp,
    pane: PaneId,
}

/// An attached client over the seeded single-pane workspace, with rows
/// `0..=NEWEST` cached the way the history stream would have left them.
async fn fixture(tag: &str) -> Fixture {
    let server = support::Server::start(tag).await;
    let pty = support::open_pty();
    let mut app: TestApp = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");
    // The rects: a wheel turn needs the focused pane's own box, both to size
    // the copy viewport and to relay a report to a pane that asked for one.
    app.repaint();
    let pane = app
        .focused_pane()
        .expect("the seeded workspace has a focused pane");
    let cache = app.cache_mut(pane);
    let whole = RowRange::new(RowId::from_raw(0), RowId::from_raw(NEWEST));
    cache.commit(whole);
    let rows: Vec<String> = (0..=NEWEST).map(|id| format!("row {id}")).collect();
    cache.fill(whole, rows.iter().map(String::as_str));
    Fixture {
        server,
        _master: pty.master,
        app,
        pane,
    }
}

/// Drive raw bytes through the machine, collecting anything addressed at a
/// pane.
fn drive(app: &mut TestApp, bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    app.handle_input(bytes, &mut |event| {
        if let InputEvent::Forward { bytes, .. } = event {
            out.push(bytes.to_vec());
        }
    });
    out
}

/// Where the copy-mode view is parked, when it is open.
fn top(app: &TestApp) -> Option<u64> {
    app.copy_view().map(|ui| ui.engine.top().get())
}

/// A wheel-up report, at coordinates chosen to be obviously irrelevant.
const WHEEL_UP: &[u8] = b"\x1b[<64;77;13M";
/// Its opposite, at different coordinates again.
const WHEEL_DOWN: &[u8] = b"\x1b[<65;2;19M";

#[tokio::test]
async fn wheel_up_opens_copy_mode_and_scrolls_it() {
    let mut fx = fixture("wheel-up").await;
    let app = &mut fx.app;

    let leaked = drive(app, WHEEL_UP);
    assert!(leaked.is_empty(), "the report reached the pane: {leaked:?}");
    assert_eq!(app.mode(), Mode::Copy, "wheel-up enters copy mode");

    // Opening parks the view at the live edge; the same turn that opened it
    // has already scrolled it one notch back, so the user sees history rather
    // than an unmoved screen.
    let opened = top(app).expect("copy mode is open");
    let live_edge = opened + NOTCH;
    assert!(
        live_edge <= NEWEST,
        "the fixture's history is shorter than one notch",
    );

    drive(app, WHEEL_UP);
    assert_eq!(
        top(app),
        Some(opened - NOTCH),
        "a second turn scrolls again"
    );

    fx.server.shutdown().await;
}

#[tokio::test]
async fn wheel_down_at_the_live_edge_leaves_copy_mode() {
    let mut fx = fixture("wheel-down").await;
    let app = &mut fx.app;

    drive(app, WHEEL_UP);
    let opened = top(app).expect("copy mode is open");

    // Back down to where it started, one notch at a time; the mode survives
    // every turn that still has somewhere to go.
    drive(app, WHEEL_DOWN);
    assert_eq!(app.mode(), Mode::Copy);
    assert_eq!(top(app), Some(opened + NOTCH));

    // And the turn after that, with the newest row already on screen, gives
    // the pane back.
    drive(app, WHEEL_DOWN);
    assert_eq!(app.mode(), Mode::Terminal, "the live edge ends the mode");
    assert!(top(app).is_none(), "the engine went with the mode");

    fx.server.shutdown().await;
}

/// Terminal mode's other half: a wheel-down when nothing is scrolled back is
/// already at the live edge, so it opens nothing.
#[tokio::test]
async fn wheel_down_in_terminal_mode_opens_nothing() {
    let mut fx = fixture("wheel-down-terminal").await;
    let app = &mut fx.app;

    let leaked = drive(app, WHEEL_DOWN);
    assert!(leaked.is_empty());
    assert_eq!(app.mode(), Mode::Terminal);

    fx.server.shutdown().await;
}

/// "Nothing else — no clicks, no taps, no drag, no hit-rects, ever" (10 §D14).
#[tokio::test]
async fn nothing_but_the_wheel_is_interpreted() {
    let mut fx = fixture("wheel-only").await;
    let app = &mut fx.app;

    for report in [
        &b"\x1b[<0;40;12M"[..], // left press
        b"\x1b[<0;40;12m",      // left release
        b"\x1b[<32;41;12M",     // drag
        b"\x1b[<2;40;12M",      // right press
        b"\x1b[<66;40;12M",     // the sideways wheel
        b"\x1b[<128;40;12M",    // button 8, one bank above the wheel's
    ] {
        let leaked = drive(app, report);
        assert!(leaked.is_empty(), "{report:?} reached the pane: {leaked:?}");
        assert_eq!(app.mode(), Mode::Terminal, "{report:?} was interpreted");
    }

    fx.server.shutdown().await;
}

/// A report split across two reads still opens the mode: the carry is the
/// machine's, and the wheel decode happens on the whole report either way.
#[tokio::test]
async fn a_split_wheel_report_still_scrolls() {
    let mut fx = fixture("wheel-split").await;
    let app = &mut fx.app;

    drive(app, b"\x1b[<64;77");
    assert_eq!(app.mode(), Mode::Terminal, "half a report does nothing yet");
    drive(app, b";13M");
    assert_eq!(app.mode(), Mode::Copy);

    fx.server.shutdown().await;
}

/// The picker is chrome and interprets no mouse event at all — and, more
/// sharply, a report's leading `Esc` is its cancel key, so a wheel turn over an
/// open picker must not close it.
#[tokio::test]
async fn the_picker_survives_a_wheel_turn() {
    let mut fx = fixture("wheel-picker").await;
    let app = &mut fx.app;

    drive(app, b"\x01p");
    assert!(app.picker_open(), "prefix `p` opens the picker");

    drive(app, WHEEL_UP);
    assert!(app.picker_open(), "a wheel turn cancelled the picker");
    assert_eq!(app.mode(), Mode::Terminal, "and did not open copy mode");

    drive(app, b"\x1b[<0;40;12M");
    assert!(app.picker_open(), "a click cancelled the picker");

    fx.server.shutdown().await;
}

/// D14's fence, asserted: the wheel path reads the button and stops. Four
/// reports with the same button and coordinates that agree about nothing must
/// do exactly the same thing.
#[tokio::test]
async fn the_chrome_path_sees_no_positional_value_at_all() {
    let mut parked = Vec::new();
    for (n, report) in [
        &b"\x1b[<64;1;1M"[..],
        b"\x1b[<64;999;999M",
        b"\x1b[<64;0;0M",
        b"\x1b[<64;;M",
    ]
    .into_iter()
    .enumerate()
    {
        let mut fx = fixture(&format!("wheel-fence-{n}")).await;
        drive(&mut fx.app, report);
        assert_eq!(fx.app.mode(), Mode::Copy, "{report:?}");
        // A second turn too, so the assertion covers scrolling and not only
        // entry: a coordinate that leaked into a step size would show here.
        drive(&mut fx.app, report);
        parked.push(top(&fx.app).expect("copy mode is open"));
        fx.server.shutdown().await;
    }
    assert!(
        parked.windows(2).all(|pair| pair[0] == pair[1]),
        "the wheel path depended on a coordinate: {parked:?}",
    );
}

/// The relay's own fence has the opposite shape and is worth pinning beside
/// it: a pane that *asked* for reports gets the position, translated into its
/// own frame rather than repeated (`docs/notes/m4-mouse-path.md` F-1).
#[tokio::test]
async fn a_pane_that_asked_gets_the_position_in_its_own_frame() {
    use amx_proto::control::session::{MouseEvents, MouseFormat, MouseMode};

    let mut fx = fixture("wheel-relay").await;
    let app = &mut fx.app;
    app.input().set_mouse_mode(
        fx.pane,
        Some(MouseMode {
            events: MouseEvents::Normal,
            format: MouseFormat::Sgr,
        }),
    );

    // One pane, so its box is the whole content area and its interior starts
    // at (1, 1) inside the border.
    let relayed = drive(app, b"\x1b[<0;10;6M");
    assert_eq!(relayed, vec![b"\x1b[<0;9;5M".to_vec()]);
    assert_eq!(
        app.mode(),
        Mode::Terminal,
        "a pane that asked keeps its wheel",
    );

    // ... and its wheel goes to it rather than into copy mode.
    let relayed = drive(app, b"\x1b[<64;10;6M");
    assert_eq!(relayed, vec![b"\x1b[<64;9;5M".to_vec()]);
    assert_eq!(app.mode(), Mode::Terminal);

    fx.server.shutdown().await;
}
