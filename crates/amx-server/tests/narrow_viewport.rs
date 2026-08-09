//! D14's narrow projection, server side: what `Viewport.panes` now means
//! (D-M4-7, R-M4-14).
//!
//! Driven over the real socket rather than at `Core`'s mailbox, because the
//! whole point of the rule is that a *declaration* carries it: the client
//! computes a projection nobody else can see and the only thing that reaches
//! the server is the list of panes it named. A test that reached in and set
//! the field would prove nothing about the half that crosses.
//!
//! The failure this closes is measured, not argued —
//! `docs/notes/m4-live-smoke.md` §1.3 recorded a 45-column client whose panes
//! were sized to 21, 9, 4 and 1 columns, because `handle_viewport` read
//! `params.rows` and `params.cols` and never `params.panes`. Every assertion
//! below fails on that tree.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::PaneId;
use amx_proto::control::{pane as pane_proto, session};
use serde_json::json;

mod support;

use support::{Server, result_of};

/// A phone-sized terminal: the size the wave-1 smoke ran its second pty at.
const NARROW_ROWS: u16 = 20;
/// See [`NARROW_ROWS`].
const NARROW_COLS: u16 = 45;

/// Ask for the session's state and decode it.
async fn state_of(client: &mut support::Client) -> session::StateReply {
    let response = client.request(90, "session.state", json!({})).await;
    serde_json::from_value(result_of(&response).clone()).expect("decode session.state")
}

/// Split `pane`, answering with the pane that was minted.
async fn split(client: &mut support::Client, id: u64, pane: PaneId) -> PaneId {
    let response = client
        .request(
            id,
            "pane.split",
            json!({ "pane": pane.to_string(), "direction": "vertical" }),
        )
        .await;
    let reply: pane_proto::SplitReply =
        serde_json::from_value(result_of(&response).clone()).expect("decode pane.split");
    reply.pane
}

/// Declare a viewport naming exactly `panes`.
async fn declare(client: &mut support::Client, id: u64, rows: u16, cols: u16, panes: &[PaneId]) {
    let response = client
        .request(
            id,
            "client.viewport",
            json!({ "rows": rows, "cols": cols, "panes": panes }),
        )
        .await;
    let _ = result_of(&response);
}

/// Every pane's `(rows, cols)`, keyed by pane.
async fn sizes_of(client: &mut support::Client) -> Vec<(PaneId, u16, u16)> {
    let mut sizes: Vec<(PaneId, u16, u16)> = state_of(client)
        .await
        .panes
        .into_iter()
        .map(|pane| (pane.pane, pane.rows, pane.cols))
        .collect();
    sizes.sort_by_key(|(pane, ..)| *pane);
    sizes
}

/// The size of one pane.
fn size_of(sizes: &[(PaneId, u16, u16)], pane: PaneId) -> (u16, u16) {
    sizes
        .iter()
        .find(|(id, ..)| *id == pane)
        .map(|&(_, rows, cols)| (rows, cols))
        .expect("the pane is in session.state")
}

/// Every workspace's layout tree, as JSON — what "the layout is byte-identical"
/// is asserted against.
async fn layouts_of(client: &mut support::Client) -> serde_json::Value {
    let state = state_of(client).await;
    serde_json::to_value(
        state
            .workspaces
            .iter()
            .map(|ws| ws.layout.clone())
            .collect::<Vec<_>>(),
    )
    .expect("encode the layout trees")
}

/// Seed a session and split its root pane twice, answering with all three.
async fn three_panes(client: &mut support::Client) -> [PaneId; 3] {
    let root = {
        let state = state_of(client).await;
        let [pane] = state.panes.as_slice() else {
            panic!("a seeded session has one pane, not {}", state.panes.len());
        };
        pane.pane
    };
    let second = split(client, 10, root).await;
    let third = split(client, 11, second).await;
    [root, second, third]
}

/// The whole of D-M4-7: a declaration naming one pane of a layout that holds
/// three sizes that pane to the content area, and resizes nobody else.
#[tokio::test]
async fn a_viewport_declaring_one_pane_of_three_sizes_it_to_the_whole_content_area() {
    let server = Server::start("narrow-one").await;
    let mut client = server.attach_rendering().await;
    let [root, second, third] = three_panes(&mut client).await;

    // First the tiled declaration a client drawing chrome makes, so the
    // single-pane sizes below are a change from a measured start rather than
    // from whatever a pane spawned at.
    declare(
        &mut client,
        20,
        NARROW_ROWS,
        NARROW_COLS,
        &[root, second, third],
    )
    .await;
    let tiled = sizes_of(&mut client).await;
    let (_, tiled_cols) = size_of(&tiled, root);
    assert!(
        tiled_cols < NARROW_COLS / 2,
        "three panes tiled into {NARROW_COLS} columns leave the first far short of the screen, \
         not {tiled_cols}",
    );

    declare(&mut client, 21, NARROW_ROWS, NARROW_COLS, &[root]).await;
    let narrow = sizes_of(&mut client).await;

    // One status line off the bottom, and no border: the client drawing this
    // projection draws neither a slot nor a frame around it.
    assert_eq!(
        size_of(&narrow, root),
        (NARROW_ROWS - 1, NARROW_COLS),
        "the declared pane is sized to the whole content area",
    );
    for other in [second, third] {
        assert_eq!(
            size_of(&narrow, other),
            size_of(&tiled, other),
            "a pane the declaration left out keeps the size the last client that drew it gave it",
        );
    }

    server.shutdown().await;
}

/// The other direction: the declaration that names the whole layout still
/// tiles it, so the rule cannot swallow the ordinary client.
#[tokio::test]
async fn a_viewport_declaring_every_pane_still_tiles_them() {
    let server = Server::start("narrow-all").await;
    let mut client = server.attach_rendering().await;
    let [root, second, third] = three_panes(&mut client).await;

    declare(&mut client, 20, NARROW_ROWS, NARROW_COLS, &[root]).await;
    declare(
        &mut client,
        21,
        NARROW_ROWS,
        NARROW_COLS,
        &[root, second, third],
    )
    .await;

    let sizes = sizes_of(&mut client).await;
    let total: u16 = [root, second, third]
        .into_iter()
        .map(|pane| size_of(&sizes, pane).1)
        .sum();
    assert!(
        total < NARROW_COLS,
        "three vertical slots and their borders fit inside {NARROW_COLS} columns, not {total}",
    );
    assert_ne!(
        size_of(&sizes, root),
        (NARROW_ROWS - 1, NARROW_COLS),
        "a client that declared the whole layout is drawing the whole layout",
    );

    server.shutdown().await;
}

/// A one-pane workspace declares one pane and is *not* the narrow projection:
/// its client draws that pane's border, so its slot's interior is the answer
/// and the whole content area would overflow the box.
#[tokio::test]
async fn the_only_pane_of_a_workspace_is_still_sized_inside_its_border() {
    let server = Server::start("narrow-solo").await;
    let mut client = server.attach_rendering().await;
    let root = state_of(&mut client).await.panes[0].pane;

    declare(&mut client, 20, NARROW_ROWS, NARROW_COLS, &[root]).await;

    assert_eq!(
        size_of(&sizes_of(&mut client).await, root),
        (NARROW_ROWS - 3, NARROW_COLS - 2),
        "one status line and one border on every side, exactly as before D14",
    );

    server.shutdown().await;
}

/// Crossing the threshold is a projection change and never a layout one.
#[tokio::test]
async fn crossing_the_projection_leaves_the_layout_tree_untouched() {
    let server = Server::start("narrow-tree").await;
    let mut client = server.attach_rendering().await;
    let [root, second, third] = three_panes(&mut client).await;

    declare(
        &mut client,
        20,
        NARROW_ROWS,
        NARROW_COLS,
        &[root, second, third],
    )
    .await;
    let before = layouts_of(&mut client).await;

    declare(&mut client, 21, NARROW_ROWS, NARROW_COLS, &[root]).await;
    let narrow = layouts_of(&mut client).await;

    declare(
        &mut client,
        22,
        NARROW_ROWS,
        NARROW_COLS,
        &[root, second, third],
    )
    .await;
    let after = layouts_of(&mut client).await;

    assert_eq!(before, narrow, "the single-pane projection mutates nothing");
    assert_eq!(narrow, after, "and neither does crossing back");
    let state = state_of(&mut client).await;
    assert_eq!(state.panes.len(), 3, "no pane was closed to make room");

    server.shutdown().await;
}

/// A declaration naming a pane this session no longer holds falls back to
/// tiling rather than sizing nothing: a stale name must not freeze every
/// other pane's grid.
#[tokio::test]
async fn a_declaration_naming_a_departed_pane_tiles_the_layout() {
    let server = Server::start("narrow-gone").await;
    let mut client = server.attach_rendering().await;
    let [root, second, third] = three_panes(&mut client).await;

    declare(
        &mut client,
        20,
        NARROW_ROWS,
        NARROW_COLS,
        &[PaneId::new_v4()],
    )
    .await;

    let sizes = sizes_of(&mut client).await;
    for pane in [root, second, third] {
        let (rows, cols) = size_of(&sizes, pane);
        assert!(
            rows > 0 && cols > 0,
            "every pane keeps a live projection under a declaration nobody can honour",
        );
        assert_ne!(
            (rows, cols),
            (NARROW_ROWS - 1, NARROW_COLS),
            "and none of them is given the whole screen on the strength of a stale name",
        );
    }

    server.shutdown().await;
}
