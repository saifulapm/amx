//! X09 acceptance: the client tracks dirtiness as a value, and the value is
//! read (D2, DR-10).
//!
//! Until M4 this client kept two booleans, so "something changed" was the most
//! it could ever say. The visible cost is here: stream bindings are per
//! *connection* and are never pruned, so a grid stream bound while a workspace
//! was on screen keeps delivering deltas after this terminal has moved on —
//! and every one of them repainted a screen the pane could not appear on.
//! [`App::frame_due`] is the reader `Effect::PaneDamage`'s pane id buys.
//!
//! The pane is made to paint by *resizing* it rather than by typing into it:
//! a second attached client declaring a larger viewport takes size authority
//! (04 §3), the pane's PTY resizes, and its grid stream answers with a
//! keyframe at a new generation. That is a fact about amx and not about
//! whichever shell the runner spawned, so the pump has something exact to wait
//! for.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::time::Duration;

use amx_client::app::App;
use amx_client::model::{PaneGrid, WorkspaceModel};
use amx_client::net::{self, Session};
use amx_core::{GridGeneration, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::{Method, client as client_proto};

/// How long a test waits for the session to answer before calling it a
/// failure. Never reached on the green path: each step blocks on a frame.
const DEADLINE: Duration = Duration::from_secs(10);

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info(name: &'static str) -> ClientInfo {
    ClientInfo {
        name: name.to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// Step the wire until `done` holds, failing on the deadline.
async fn pump_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) {
    let stepped = tokio::time::timeout(DEADLINE, async {
        while !done(app) {
            app.step_frame().await.expect("read one server frame");
        }
    })
    .await;
    assert!(
        stepped.is_ok(),
        "the session never sent what was waited for"
    );
}

/// The generation of `pane`'s cached grid, or `None` while it has none.
fn generation(app: &mut TestApp, pane: PaneId) -> Option<GridGeneration> {
    app.model().pane(pane).map(PaneGrid::generation)
}

/// Attach a second client and let it declare `rows`x`cols`, taking size
/// authority for every pane the session projects (04 §3).
async fn take_size_authority(socket: &std::path::Path, rows: u16, cols: u16) -> Session {
    let stream = net::connect(socket).await.expect("second connection");
    let (mut session, _welcome) =
        Session::attach(stream, client_info("amx-effects-sizer"), true, None)
            .await
            .expect("negotiate the second connection");
    let params = serde_json::to_value(client_proto::Viewport {
        rows,
        cols,
        panes: Vec::new(),
    })
    .expect("encode the viewport");
    session
        .call(Method::ClientViewport.wire_name(), params)
        .await
        .expect("declare the viewport");
    session
}

#[tokio::test]
async fn a_delta_for_a_pane_this_client_is_not_drawing_owes_it_no_frame() {
    let server = support::Server::start("fx-hidden").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-effects-test"),
    )
    .await
    .expect("attach to the real server over the real socket");
    let pane = app
        .focused_pane()
        .expect("the attach folded the seeded workspace's pane");

    // The control half: while the pane is on screen, its keyframe is owed.
    pump_until(&mut app, |app| {
        app.model().pane(pane).is_some_and(PaneGrid::complete)
    })
    .await;
    assert!(
        app.frame_due(),
        "a keyframe for a pane on screen must owe a frame"
    );
    app.repaint();
    assert!(!app.frame_due(), "a repaint draws what it drained");

    // Now show a workspace of this client's own. Nothing unbinds the seeded
    // pane's grid stream — bindings belong to the connection — so its deltas
    // keep arriving at a client that is no longer drawing it.
    app.adopt_workspace(
        WorkspaceId::new_v4(),
        WorkspaceModel {
            label: Some("elsewhere".to_owned()),
            layout: BspLayout::with_root(PaneId::new_v4()),
        },
    );
    app.repaint();
    assert!(!app.frame_due());
    let drawn = app.repaints;

    let was = generation(&mut app, pane).expect("the pane's grid is cached");
    let _sizer = take_size_authority(server.socket(), 40, 100).await;
    pump_until(&mut app, |app| generation(app, pane) != Some(was)).await;

    assert!(
        !app.frame_due(),
        "a delta for a pane this client is not drawing owed it a frame"
    );
    assert_eq!(
        app.repaints, drawn,
        "nothing repainted, because nothing on this screen changed"
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn a_delta_for_a_pane_on_screen_still_owes_a_frame() {
    let server = support::Server::start("fx-shown").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-effects-test"),
    )
    .await
    .expect("attach to the real server over the real socket");
    let pane = app
        .focused_pane()
        .expect("the attach folded the seeded workspace's pane");

    pump_until(&mut app, |app| {
        app.model().pane(pane).is_some_and(PaneGrid::complete)
    })
    .await;
    app.repaint();
    assert!(!app.frame_due());

    let was = generation(&mut app, pane).expect("the pane's grid is cached");
    let _sizer = take_size_authority(server.socket(), 40, 100).await;
    pump_until(&mut app, |app| generation(app, pane) != Some(was)).await;

    assert!(
        app.frame_due(),
        "the suppression must be about visibility and nothing else"
    );

    drop(app);
    server.shutdown().await;
}
