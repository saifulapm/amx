//! X15 acceptance, the wired half: a peek binds a pane this client is not
//! drawing, and moving the selection *moves* that binding.
//!
//! The whole difficulty of the surface is here. A peek is a subscription, and
//! the plan's own entry says so: "switching selection moves the stream rather
//! than accumulating them". Nothing on screen shows a stream that was left
//! running — the cells go on arriving for a pane nobody is looking at, and the
//! only symptom is bandwidth. [`App::peek_live`] is what makes it assertable,
//! and every test below would pass on a peek that never released anything if it
//! were left out.
//!
//! Over a real server on a real socket, like `tests/effects.rs`: what is under
//! test is a bind for a pane outside the declared viewport, which only a server
//! can refuse or serve. The panes are made to paint by *resizing* them — a
//! second client declaring a viewport takes size authority (04 §3) and the pane
//! answers with a keyframe at a new generation — because that is a fact about
//! amx rather than about whichever shell the runner spawned.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::path::Path;
use std::time::Duration;

use amx_client::app::App;
use amx_client::input::InputEvent;
use amx_client::model::PaneGrid;
use amx_client::net::{self, Session};
use amx_core::{GridGeneration, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::{Method, client as client_proto};
use serde_json::json;

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

/// A connection of its own, for the calls this suite makes on the session
/// rather than through the client under test.
///
/// Not `App::session().call`: that spelling drops any stream frame that lands
/// while the reply is outstanding (`Session::call`'s `on_frame` is empty), and
/// this suite is entirely about those frames.
async fn side_channel(socket: &Path, name: &'static str) -> Session {
    let stream = net::connect(socket).await.expect("a second connection");
    let (session, _welcome) = Session::attach(stream, client_info(name), false, None)
        .await
        .expect("negotiate the second connection");
    session
}

/// Create a workspace with a live root pane, unfocused, and answer its id.
async fn create_workspace(session: &mut Session, label: &str) -> WorkspaceId {
    let reply = session
        .call("workspace.create", json!({ "label": label }))
        .await
        .expect("workspace.create");
    serde_json::from_value(reply["workspace"].clone()).expect("decode the workspace id")
}

/// The pane a mirrored workspace holds, once the client has folded it.
fn only_pane(app: &mut TestApp, workspace: WorkspaceId) -> PaneId {
    let panes = app
        .model()
        .workspace(workspace)
        .expect("the workspace is mirrored")
        .layout
        .panes();
    assert_eq!(panes.len(), 1, "a fresh workspace holds its root pane");
    panes[0]
}

/// Attach a second client and let it declare `rows`x`cols`, taking size
/// authority for every pane the session projects (04 §3).
async fn take_size_authority(socket: &Path, rows: u16, cols: u16) -> Session {
    let stream = net::connect(socket).await.expect("sizer connection");
    let (mut session, _welcome) =
        Session::attach(stream, client_info("amx-peek-sizer"), true, None)
            .await
            .expect("negotiate the sizer");
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
async fn a_peek_binds_a_pane_outside_this_clients_projection_and_repaints_for_it() {
    let server = support::Server::start("peek-bind").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-peek-test"),
    )
    .await
    .expect("attach to the real server over the real socket");
    let shown = app
        .focused_pane()
        .expect("the attach folded the seeded workspace's pane");

    let mut side = side_channel(server.socket(), "amx-peek-maker").await;
    let elsewhere = create_workspace(&mut side, "elsewhere").await;
    app.resync_state().await.expect("fold the new workspace");
    let watched = only_pane(&mut app, elsewhere);
    assert_ne!(
        app.model().focused_workspace_id(),
        Some(elsewhere),
        "the peeked pane must be in a workspace this terminal is not drawing",
    );

    // `stream.bind` has no visibility check, so the bind is served — and the
    // pane's own cells arrive at a client whose viewport never named it.
    app.open_peek(watched).await.expect("open the peek");
    assert_eq!(app.peeked(), Some(watched));
    pump_until(&mut app, |app| {
        app.model().pane(watched).is_some_and(PaneGrid::complete)
    })
    .await;

    // And the region repaints for it: X09's hand-off is that `frame_due` asks
    // whether the damaged pane is one this terminal draws, and the peeked pane
    // is not one the projection draws.
    app.repaint();
    assert!(!app.frame_due(), "a repaint draws what it drained");
    let was = generation(&mut app, watched).expect("the peeked pane's grid is cached");
    let _sizer = take_size_authority(server.socket(), 40, 100).await;
    pump_until(&mut app, |app| generation(app, watched) != Some(was)).await;
    assert!(
        app.frame_due(),
        "a delta for the peeked pane owes this terminal a frame",
    );
    assert_ne!(shown, watched);

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn moving_the_selection_moves_the_stream_rather_than_accumulating_them() {
    let server = support::Server::start("peek-move").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-peek-test"),
    )
    .await
    .expect("attach to the real server over the real socket");

    let mut side = side_channel(server.socket(), "amx-peek-maker").await;
    let first = create_workspace(&mut side, "api").await;
    let second = create_workspace(&mut side, "web").await;
    let third = create_workspace(&mut side, "infra").await;
    app.resync_state().await.expect("fold the new workspaces");
    let (a, b, c) = (
        only_pane(&mut app, first),
        only_pane(&mut app, second),
        only_pane(&mut app, third),
    );

    // Three panes, none of them drawn, peeked one after another: at every step
    // exactly one stream is being sent on, and it is the selected one. Without
    // the release this reads `[a]`, `[a, b]`, `[a, b, c]` — a leak with nothing
    // on screen to show for it.
    app.open_peek(a).await.expect("peek the first");
    assert_eq!(app.peek_live(), vec![a]);
    app.open_peek(b).await.expect("peek the second");
    assert_eq!(app.peek_live(), vec![b]);
    app.open_peek(c).await.expect("peek the third");
    assert_eq!(app.peek_live(), vec![c]);

    // Back to one already bound: the stream it has is resumed, not a second one
    // opened beside it. Channels are a byte wide and never reused on a
    // connection, so re-peeking must cost nothing.
    app.open_peek(a).await.expect("peek the first again");
    assert_eq!(app.peek_live(), vec![a]);

    app.close_peek().await.expect("close the peek");
    assert_eq!(app.peeked(), None);
    assert!(
        app.peek_live().is_empty(),
        "a closed peek leaves nothing being sent on its account",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn a_released_stream_comes_back_when_its_pane_comes_on_screen() {
    let server = support::Server::start("peek-resume").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-peek-test"),
    )
    .await
    .expect("attach to the real server over the real socket");

    let mut side = side_channel(server.socket(), "amx-peek-maker").await;
    let elsewhere = create_workspace(&mut side, "elsewhere").await;
    app.resync_state().await.expect("fold the new workspace");
    let watched = only_pane(&mut app, elsewhere);

    app.open_peek(watched).await.expect("open the peek");
    pump_until(&mut app, |app| {
        app.model().pane(watched).is_some_and(PaneGrid::complete)
    })
    .await;
    app.close_peek().await.expect("close the peek");
    assert!(app.peek_live().is_empty(), "the peek released its stream");

    // Now look at the workspace that pane lives in. The binding table says the
    // pane is bound, so nothing here would ever bind it again; the stream it
    // has is the released one, and it has to be given back.
    app.model().focus_workspace(elsewhere);
    app.resync_state()
        .await
        .expect("rebind what is now visible");
    assert_eq!(
        app.peek_live(),
        vec![watched],
        "a pane the projection now draws must be being sent on",
    );

    // And the resume reached the server, not only this client's bookkeeping: a
    // stream that was still paused would never deliver the resize, and this
    // waits rather than sampling.
    let was = generation(&mut app, watched).expect("the pane's grid is cached");
    let _sizer = take_size_authority(server.socket(), 40, 100).await;
    pump_until(&mut app, |app| generation(app, watched) != Some(was)).await;

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn peeking_a_pane_the_projection_draws_borrows_its_stream_and_never_releases_it() {
    let server = support::Server::start("peek-borrow").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-peek-test"),
    )
    .await
    .expect("attach to the real server over the real socket");
    let shown = app
        .focused_pane()
        .expect("the attach folded the seeded workspace's pane");
    pump_until(&mut app, |app| {
        app.model().pane(shown).is_some_and(PaneGrid::complete)
    })
    .await;

    // The projection already binds this pane, so the peek opens nothing.
    app.open_peek(shown).await.expect("peek a visible pane");
    assert!(
        app.peek_live().is_empty(),
        "a borrowed stream is not this surface's to own",
    );
    app.close_peek().await.expect("close the peek");

    // The pane is still on screen and still painting: a release that paused
    // whatever the binding table happened to hold would have frozen it, and
    // this wait is what that costs.
    let was = generation(&mut app, shown).expect("the pane's grid is cached");
    let _sizer = take_size_authority(server.socket(), 40, 100).await;
    pump_until(&mut app, |app| generation(app, shown) != Some(was)).await;

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn no_keystroke_reaches_the_peeked_pane() {
    let server = support::Server::start("peek-input").await;
    let pty = support::open_pty();
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        client_info("amx-peek-test"),
    )
    .await
    .expect("attach to the real server over the real socket");
    let shown = app
        .focused_pane()
        .expect("the attach folded the seeded workspace's pane");

    let mut side = side_channel(server.socket(), "amx-peek-maker").await;
    let elsewhere = create_workspace(&mut side, "elsewhere").await;
    app.resync_state().await.expect("fold the new workspace");
    let watched = only_pane(&mut app, elsewhere);
    app.open_peek(watched).await.expect("open the peek");

    // D15: the peek is read-only and `ctrl+p` — the agents view's own prompt
    // key — is the only reply path. A peek moves nothing about where input is
    // addressed, so every byte still goes where it went before.
    let mut addressed = Vec::new();
    app.handle_input(b"hello", &mut |event| {
        if let InputEvent::Forward { pane, .. } = event {
            addressed.push(pane);
        }
    });
    assert!(
        !addressed.is_empty(),
        "the keystrokes went somewhere, or this proves nothing",
    );
    assert!(
        addressed.iter().all(|&pane| pane == shown),
        "input reached a pane other than the focused one: {addressed:?}",
    );
    assert!(
        !addressed.contains(&watched),
        "a keystroke reached the peeked pane",
    );

    drop(app);
    server.shutdown().await;
}
