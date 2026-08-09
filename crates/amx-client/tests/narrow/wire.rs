//! X12 acceptance, the half that crosses the socket.
//!
//! The projection its sibling pins is invisible to the server: what reaches it
//! is a list of pane ids on `client.viewport`, and until D-M4-7 the server read
//! the two fields beside that list and never the list itself. Both tests here
//! drive a real `Core` over a real socket, because that is the only place the
//! two halves of the rule meet.

use std::collections::HashMap;

use amx_client::app::{App, Projection};
use amx_client::config::NarrowCols;
use amx_client::model::PaneGrid;
use amx_client::net::{self, Session};
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::{Method, client as client_proto, pane as pane_proto, session};
use serde_json::json;

use crate::{NARROW, TestApp, client_info, support};

/// The half that crosses the socket: a narrow client tells the server which
/// pane it is drawing, and the server sizes that pane to the screen.
///
/// Everything here is real — a real socket, the seeded workspace's real pane,
/// a real `pane.split` — because this is the assertion a mirrored fixture
/// cannot make: before D-M4-7 the declaration carried the same three fields
/// and the server read two of them.
#[tokio::test]
async fn a_narrow_client_declares_the_pane_it_draws_and_the_server_sizes_it() {
    let server = support::Server::start("narrow-wire").await;
    let (rows, cols) = NARROW;
    let pty = support::open_pty_sized(rows, cols);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");
    app.set_narrow_cols(NarrowCols(60));

    let root = app
        .focused_pane()
        .expect("the attach folded the seeded workspace's pane");
    app.session()
        .call(
            "pane.split",
            json!({ "pane": root, "direction": "vertical" }),
        )
        .await
        .expect("split the seeded workspace");
    // The fold is what re-declares: the layout it just replaced is what the
    // projection is computed from.
    app.sync_state().await.expect("re-read the session's state");

    let shown = app.focused_pane().expect("a focused pane after the split");
    assert_eq!(app.projection(), Projection::Single(shown));

    let value = app
        .session()
        .call("session.state", json!({}))
        .await
        .expect("read the session's state");
    let state: session::StateReply = serde_json::from_value(value).expect("decode session.state");
    let sizes: HashMap<PaneId, (u16, u16)> = state
        .panes
        .iter()
        .map(|pane| (pane.pane, (pane.rows, pane.cols)))
        .collect();

    assert_eq!(
        sizes.get(&shown).copied(),
        Some((rows - 1, cols)),
        "the pane this client draws is sized to the screen it draws it on; \
         before D-M4-7 it was sized to its slot in a layout nobody was drawing",
    );
    // Nothing is asserted about the *other* pane's size, and that is the rule
    // rather than a gap: a declaration that left a pane out gives it no rect,
    // so it keeps whatever size it last had. What must hold is that it is
    // still a grid a process can run in.
    assert_eq!(sizes.len(), 2, "the split left two panes");
    assert!(
        sizes.values().all(|&(rows, cols)| rows > 0 && cols > 0),
        "every pane keeps a live grid: {sizes:?}",
    );

    drop(app);
    server.shutdown().await;
}

/// A delta for a pane the narrow projection is hiding owes this terminal no
/// frame.
///
/// The panes D14 does not draw stay bound — moving between them on a phone must
/// not cost a keyframe each way — so their deltas keep arriving at a client with
/// one pane on screen, and `App::showing` is what stops them repainting it. X09
/// left this narrowing to whichever task built the projection, and this is it.
///
/// Every pane but the workspace's root runs `sleep`, so the only thing that can
/// paint one is amx resizing it: what a shell would print at its own pace is not
/// a fact about this client.
#[tokio::test]
async fn a_delta_for_a_pane_the_narrow_projection_hides_owes_no_frame() {
    let server = support::Server::start("narrow-hide").await;

    // The workspace is built from a verb connection *before* the client
    // attaches: a client that folded a `pane_created` of its own would resync
    // and re-declare, which is a second thing happening in the window this
    // test measures.
    let mut setup = session_on(server.socket()).await;
    let created = setup
        .call("workspace.create", json!({}))
        .await
        .expect("workspace.create");
    let workspace: WorkspaceId =
        serde_json::from_value(created["workspace"].clone()).expect("decode the workspace id");
    let root = only_pane_of(&mut setup, workspace).await;
    let second = split_silent(&mut setup, root).await;
    let third = split_silent(&mut setup, second).await;

    let (rows, cols) = NARROW;
    let pty = support::open_pty_sized(rows, cols);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");

    let shown = app.focused_pane().expect("a focused pane");
    assert_eq!(
        shown, third,
        "a split focuses the pane it minted, which is the silent one this test draws",
    );
    assert_eq!(app.projection(), Projection::Single(shown));

    // Every pane's first keyframe — and, for the pane this client declared,
    // the *second* one: the bind happens before the declaration, so the grid
    // this client is drawing arrives at the pane's spawn size and is resized to
    // the screen a round trip later. Waiting for the size is what makes the
    // window below hold nothing but the delta this test causes.
    for pane in [root, second] {
        pump_until(&mut app, |app| {
            app.model().pane(pane).is_some_and(PaneGrid::complete)
        })
        .await;
    }
    pump_until(&mut app, |app| {
        app.model()
            .pane(third)
            .is_some_and(|grid| grid.complete() && grid.rows() == rows - 1 && grid.cols() == cols)
    })
    .await;
    app.repaint();
    let drawn = app.repaints;
    assert!(!app.frame_due(), "a repaint draws what it drained");

    // Resize *only* the hidden pane, by declaring it and nothing else from a
    // second connection: the server-side half of this task is what makes that
    // a one-pane resize instead of a whole-layout one.
    let was = app.model().pane(second).map(PaneGrid::generation);
    let _sizer = declare_from(server.socket(), 40, 100, &[second]).await;
    pump_until(&mut app, |app| {
        app.model().pane(second).map(PaneGrid::generation) != was
    })
    .await;

    assert!(
        !app.frame_due(),
        "a delta for a pane the narrow projection is not drawing owed a frame",
    );
    assert_eq!(
        app.repaints, drawn,
        "and nothing repainted, because nothing on this screen changed",
    );

    drop(app);
    server.shutdown().await;
}

/// Step the wire until `done` holds, failing on the deadline.
async fn pump_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) {
    let stepped = tokio::time::timeout(std::time::Duration::from_secs(10), async {
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

/// A verb connection to `socket`: it declares no viewport, so building a
/// workspace through it takes size authority from nobody.
async fn session_on(socket: &std::path::Path) -> Session {
    let stream = net::connect(socket).await.expect("connect");
    let (session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .expect("negotiate");
    session
}

/// The one pane `workspace` was created with.
async fn only_pane_of(session: &mut Session, workspace: WorkspaceId) -> PaneId {
    let value = session
        .call("session.state", json!({}))
        .await
        .expect("session.state");
    let state: session::StateReply = serde_json::from_value(value).expect("decode session.state");
    state
        .workspaces
        .iter()
        .find(|ws| ws.workspace == workspace)
        .expect("the workspace just created")
        .layout
        .panes()[0]
}

/// Split `pane`, giving the new one a process that never prints.
async fn split_silent(session: &mut Session, pane: PaneId) -> PaneId {
    let value = session
        .call(
            "pane.split",
            json!({ "pane": pane, "direction": "vertical", "command": ["sleep", "60"] }),
        )
        .await
        .expect("pane.split");
    let reply: pane_proto::SplitReply = serde_json::from_value(value).expect("decode pane.split");
    reply.pane
}

/// Attach a second connection and let it declare `panes` at `rows`x`cols`,
/// taking size authority (04 §3).
async fn declare_from(socket: &std::path::Path, rows: u16, cols: u16, panes: &[PaneId]) -> Session {
    let mut session = session_on(socket).await;
    let params = serde_json::to_value(client_proto::Viewport {
        rows,
        cols,
        panes: panes.to_vec(),
    })
    .expect("encode the viewport");
    session
        .call(Method::ClientViewport.wire_name(), params)
        .await
        .expect("declare the viewport");
    session
}
