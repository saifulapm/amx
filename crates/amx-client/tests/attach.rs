//! T13 acceptance: `attach` renders a pane's grid matching the server's.
//!
//! This drives a real `amx-server` — real `Gateway`, real `Core`, a real
//! `UnixListener` socket, real `Hello`/`Welcome` negotiation and a real
//! `workspace.create` RPC call (T09/T10, both merged) — proving
//! `net::Session` and `App::attach` work end to end against the actual
//! server binary this client will run against.
//!
//! What it does *not* do is read the pane's cells off a real bound grid
//! stream, because there is no such stream anywhere in the merged tree to
//! read from: `amx_proto::stream::grid::GridMessage::decode` is `todo!()`
//! with no per-cell wire layout defined by any golden, no task in
//! `docs/06-m0-plan.md`'s DAG claims `amx-proto/src/stream/**`, and
//! `workspace.create`'s reply (`amx_proto::control::workspace::CreateReply`,
//! frozen by T01) does not carry the id of the root pane the server just
//! created for it — there is no event-bus delivery over the wire yet to learn
//! it another way either. So this test mints the pane id locally (identity a
//! real client would instead learn from the server once those exist) and
//! applies a keyframe to it directly, the way `App`'s decode path will once
//! T04's codec lands (`net`'s module doc has the same note). What *is* real
//! and under test is everything from there on: the model update, the SGR
//! differ, the border/status chrome, and the blit — reconstructed from the
//! actual ANSI bytes `App::repaint` produced and compared cell-for-cell
//! against the grid that was applied, proving the render matches the server
//! grid rather than merely asserting the model does.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::App;
use amx_client::model::{Attrs, Cell, WorkspaceModel};
use amx_core::{GridGeneration, Layout as BspLayout, PaneId};
use amx_proto::ClientInfo;
use amx_proto::stream::{Cursor, CursorShape};
use serde_json::json;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-attach-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

#[tokio::test]
async fn attach_renders_the_pane_grid_matching_the_server_grid() {
    let server = support::Server::start("attach").await;
    let pty = support::open_pty();

    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");

    let reply = app
        .session()
        .call("workspace.create", json!({}))
        .await
        .expect("a real workspace.create round trip against the real Core");
    let workspace: amx_core::WorkspaceId =
        serde_json::from_value(reply["workspace"].clone()).expect("decode the workspace id");

    let pane = PaneId::new_v4();
    app.adopt_workspace(
        workspace,
        WorkspaceModel {
            label: Some("attach-test".to_owned()),
            layout: BspLayout::with_root(pane),
        },
    );

    // Exactly the inset chrome computes for a lone pane in a 24x80 pty (see
    // `support::open_pty`'s default): content area 80x23 (one status row),
    // minus the one-cell border on every side chrome draws around the pane
    // rect. Matching it exactly means `render::grid::fit` neither pads nor
    // crops, so the grid lands flush at the inset's own origin — the
    // letterbox/clip *offset* math is what `tests/letterbox.rs` checks.
    let rows = 21_u16;
    let cols = 78_u16;
    let source: Vec<Cell> = (0..u32::from(rows) * u32::from(cols))
        .map(|i| Cell {
            ch: char::from(b'a' + (i % 26) as u8),
            attrs: Attrs {
                bold: i % 7 == 0,
                ..Attrs::default()
            },
        })
        .collect();
    let cursor = Cursor {
        row: 0,
        col: 0,
        visible: true,
        shape: CursorShape::default(),
        blink: false,
    };
    app.model().pane_mut(pane, rows, cols).apply_reset(
        GridGeneration::FIRST.next(),
        rows,
        cols,
        &source,
        cursor,
    );

    app.repaint();
    let rendered = support::rasterize(app.frame());

    // The pane sits inside chrome's border, inset by one cell on each side
    // (`chrome::inset`), and the client's own terminal (from the pty) is
    // large enough that this pane's server size fits without letterboxing or
    // clipping — `non_active_client_letterboxes_rather_than_resizing_the_pane`
    // in `tests/letterbox.rs` is what exercises the case where it does not.
    let origin_row = 1_u16;
    let origin_col = 1_u16;
    for r in 0..rows {
        for c in 0..cols {
            let expected = source[usize::from(r) * usize::from(cols) + usize::from(c)].ch;
            let actual = rendered
                .get(&(origin_row + r, origin_col + c))
                .copied()
                .unwrap_or_else(|| {
                    panic!(
                        "no cell rendered at ({}, {})",
                        origin_row + r,
                        origin_col + c
                    )
                });
            assert_eq!(
                actual, expected,
                "rendered cell at ({}, {}) must match the server grid's cell",
                r, c
            );
        }
    }

    server.shutdown().await;
}
