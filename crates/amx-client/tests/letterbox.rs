//! T13 acceptance: a client whose own terminal is larger than a pane's
//! server-authoritative grid letterboxes it — centers it with blank padding —
//! rather than stretching the grid to fill the extra space. 04 §3: "the
//! pane's PTY grid size follows the most-recently-active client... non-active
//! clients letterbox/clip the server-sized pane grids inside their own
//! locally-computed chrome". Fails without the change: replace
//! `render::grid::blit`'s centering with a naive fill and the padding cells
//! stop being blank, or the pane's own `rows()`/`cols()` get resized to match
//! the client's rect.

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
        name: "amx-letterbox-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

#[tokio::test]
async fn non_active_client_letterboxes_rather_than_resizing_the_pane() {
    let server = support::Server::start("letterbox").await;
    // A generously sized client terminal: bigger, in both dimensions, than
    // the pane grid it is about to render.
    let pty = support::open_pty_sized(20, 40);

    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");

    let reply = app
        .session()
        .call("workspace.create", json!({}))
        .await
        .expect("workspace.create");
    let workspace: amx_core::WorkspaceId =
        serde_json::from_value(reply["workspace"].clone()).expect("decode the workspace id");

    let pane = PaneId::new_v4();
    app.adopt_workspace(
        workspace,
        WorkspaceModel {
            label: None,
            layout: BspLayout::with_root(pane),
        },
    );

    // The server-authoritative grid: small, and never touched by this
    // client's own (much larger) size.
    let (rows, cols) = (10_u16, 20_u16);
    let source: Vec<Cell> = (0..u32::from(rows) * u32::from(cols))
        .map(|i| Cell {
            ch: char::from(b'A' + (i % 26) as u8),
            attrs: Attrs::default(),
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

    // Term 20x40 → content area 40x19 (one status row) → border around the
    // sole pane leaves an inset of 38x17 → the 20x10 grid is centered inside
    // that: x_pad = (38-20)/2 = 9, y_pad = (17-10)/2 = 3, both relative to
    // the inset's own origin at (1, 1).
    let inset_origin = (1_u16, 1_u16);
    let x_pad = 9_u16;
    let y_pad = 3_u16;
    let window_row0 = inset_origin.0 + y_pad;
    let window_col0 = inset_origin.1 + x_pad;

    for r in 0..rows {
        for c in 0..cols {
            let expected = source[usize::from(r) * usize::from(cols) + usize::from(c)].ch;
            let actual = rendered
                .get(&(window_row0 + r, window_col0 + c))
                .copied()
                .unwrap_or_else(|| {
                    panic!("no cell at the expected letterboxed position ({r}, {c})")
                });
            assert_eq!(
                actual, expected,
                "letterboxed cell ({r}, {c}) must match the server grid exactly"
            );
        }
    }

    // Padding directly left of the letterboxed window, inside the inset,
    // must be blank — never a stretched copy of the grid's own content.
    for r in 0..rows {
        let pad_col = window_col0 - 1;
        let actual = rendered
            .get(&(window_row0 + r, pad_col))
            .copied()
            .unwrap_or(' ');
        assert_eq!(
            actual, ' ',
            "letterbox padding must be blank, not a stretched cell"
        );
    }

    // The pane's own grid was never resized to fit this client's rect — the
    // whole point of letterboxing instead of resizing.
    let cached = app.model().pane(pane).expect("the pane grid is cached");
    assert_eq!(
        cached.rows(),
        rows,
        "the pane's own row count must be untouched"
    );
    assert_eq!(
        cached.cols(),
        cols,
        "the pane's own column count must be untouched"
    );

    server.shutdown().await;
}
