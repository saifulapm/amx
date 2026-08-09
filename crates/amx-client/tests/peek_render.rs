//! X15 acceptance, the drawn half: where the peek region is, and what it puts
//! there.
//!
//! The cells are fabricated into the client's own mirror rather than typed into
//! a shell, the shape `tests/letterbox.rs` established: what is under test is
//! the projection from a pane's grid onto a region of this terminal, and a
//! prompt whose text depends on whichever shell the runner has is not a fact
//! about amx. The stream that fills that mirror in production is
//! `tests/peek.rs`.
//!
//! Fails without the change in three separate ways: no region is reserved, so
//! `peek_layout` gives the list everything; nothing draws the peeked pane, so
//! the region holds whatever the panes underneath left; and a pane that has
//! left the session is indistinguishable from one whose keyframe has not
//! arrived.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::collections::HashMap;

use amx_client::app::App;
use amx_client::model::{Attrs, Cell, WorkspaceModel};
use amx_core::{Direction, GridGeneration, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::stream::{Cursor, CursorShape};

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-peek-render-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// The character this fixture puts at `(row, col)` of grid `seed`.
///
/// Seeded so two fabricated grids are never the same picture: under D14's
/// single-pane projection the peek covers the pane it is drawn over exactly,
/// and two identical grids would make a region that drew nothing at all look
/// like one that drew the right thing.
fn glyph(seed: u32, row: u16, col: u16) -> char {
    let at = seed * 7 + u32::from(row) * 97 + u32::from(col);
    char::from(b'a' + (at % 26) as u8)
}

/// Give `pane` a complete grid of [`glyph`]s, `rows` by `cols`.
fn fabricate(app: &mut TestApp, pane: PaneId, seed: u32, rows: u16, cols: u16) {
    let cells: Vec<Cell> = (0..rows)
        .flat_map(|row| {
            (0..cols).map(move |col| Cell {
                ch: glyph(seed, row, col),
                attrs: Attrs::default(),
            })
        })
        .collect();
    let cursor = Cursor {
        row: 0,
        col: 0,
        visible: false,
        shape: CursorShape::default(),
        blink: false,
    };
    app.model().pane_mut(pane, rows, cols).apply_reset(
        GridGeneration::FIRST.next(),
        rows,
        cols,
        &cells,
        cursor,
    );
}

/// Mirror a workspace holding `panes`, and show it.
fn show_workspace(app: &mut TestApp, panes: &[PaneId]) -> WorkspaceId {
    let id = WorkspaceId::new_v4();
    let mut layout = BspLayout::with_root(panes[0]);
    for &pane in &panes[1..] {
        layout
            .split(panes[0], Direction::Right, pane, 0.5)
            .expect("split the mirrored layout");
    }
    app.model().set_workspace(
        id,
        WorkspaceModel {
            label: None,
            layout,
        },
    );
    app.model().focus_workspace(id);
    id
}

/// What the frame shows across `rows` x `cols` starting at `(y, x)`.
fn region(
    rendered: &HashMap<(u16, u16), char>,
    y: u16,
    x: u16,
    rows: u16,
    cols: u16,
) -> Vec<String> {
    (0..rows)
        .map(|row| {
            (0..cols)
                .map(|col| rendered.get(&(y + row, x + col)).copied().unwrap_or(' '))
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn the_peek_region_holds_the_peeked_panes_own_cells() {
    let server = support::Server::start("peek-draw").await;
    let pty = support::open_pty_sized(24, 80);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");

    // On screen: one pane of this client's own, filling the content area.
    let shown = PaneId::new_v4();
    show_workspace(&mut app, &[shown]);
    fabricate(&mut app, shown, 0, 22, 78);

    // Peeked: a pane in no mirrored workspace at all, which is what a pane in
    // another project looks like to a client that has not been told about it.
    let watched = PaneId::new_v4();
    app.open_peek(watched).await.expect("open the peek");

    // 24x80 terminal → 80x23 content area → the list keeps the odd row, so the
    // peek is the bottom 11 and its border leaves 78x9 of interior at (13, 1).
    let layout = app.peek_layout();
    assert_eq!(layout.list.h, 12, "the list keeps the odd row");
    let rect = layout.peek.expect("a peek is open and the screen has room");
    assert_eq!((rect.x, rect.y, rect.w, rect.h), (0, 12, 80, 11));

    fabricate(&mut app, watched, 1, 9, 78);
    app.repaint();
    let rendered = support::rasterize(app.frame());

    let drawn = region(&rendered, 13, 1, 9, 78);
    let expected: Vec<String> = (0..9)
        .map(|row| (0..78).map(|col| glyph(1, row, col)).collect())
        .collect();
    assert_eq!(drawn, expected, "the peek region must hold the peeked pane");

    // And the pane this terminal is actually drawing keeps the rows above it:
    // the peek is a region, not a takeover.
    assert_eq!(
        rendered.get(&(1, 1)).copied(),
        Some(glyph(0, 0, 0)),
        "the shown pane still owns the top of the screen",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_peeked_panes_attributes_survive_the_region() {
    let server = support::Server::start("peek-attr").await;
    let pty = support::open_pty_sized(24, 80);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");
    let shown = PaneId::new_v4();
    show_workspace(&mut app, &[shown]);

    let watched = PaneId::new_v4();
    app.open_peek(watched).await.expect("open the peek");

    // One bold cell, one plain one, side by side: the differ emits an escape
    // for the first and not for the second, so a region that dropped attributes
    // would render two identical cells.
    let styled = vec![
        Cell {
            ch: 'X',
            attrs: Attrs {
                bold: true,
                ..Attrs::default()
            },
        },
        Cell {
            ch: 'Y',
            attrs: Attrs::default(),
        },
    ];
    let cursor = Cursor {
        row: 0,
        col: 0,
        visible: false,
        shape: CursorShape::default(),
        blink: false,
    };
    app.model().pane_mut(watched, 1, 2).apply_reset(
        GridGeneration::FIRST.next(),
        1,
        2,
        &styled,
        cursor,
    );

    app.repaint();
    let frame = String::from_utf8(app.frame().to_vec()).expect("the frame is utf-8");
    assert!(
        frame.contains("\x1b[1mX"),
        "the peek must emit the pane's own bold, not a flattened cell",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn a_peeked_pane_that_left_the_session_says_so_and_leaves_the_view_usable() {
    let server = support::Server::start("peek-gone").await;
    let pty = support::open_pty_sized(24, 80);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");
    let shown = PaneId::new_v4();
    show_workspace(&mut app, &[shown]);
    fabricate(&mut app, shown, 0, 22, 78);

    // In a mirrored layout, and with no cells yet: the window between opening a
    // peek and its keyframe landing. Blank, deliberately — a message here would
    // call every healthy agent dead for a frame.
    let waiting = PaneId::new_v4();
    let ws = show_workspace(&mut app, &[shown, waiting]);
    assert!(app.model().workspace(ws).is_some());
    app.open_peek(waiting).await.expect("open the peek");
    app.repaint();
    let rendered = support::rasterize(app.frame());
    let rect = app.peek_layout().peek.expect("the screen has room");
    let interior = region(&rendered, rect.y + 1, rect.x + 1, rect.h - 2, rect.w - 2);
    assert!(
        interior.iter().all(|row| row.trim().is_empty()),
        "a peek waiting for its first keyframe says nothing: {interior:?}",
    );

    // Now a pane no mirrored workspace holds: the pane closed while the view
    // had it selected.
    let gone = PaneId::new_v4();
    app.open_peek(gone).await.expect("move the peek");
    app.repaint();
    let rendered = support::rasterize(app.frame());
    let interior = region(&rendered, rect.y + 1, rect.x + 1, rect.h - 2, rect.w - 2);
    let said: Vec<&String> = interior
        .iter()
        .filter(|row| !row.trim().is_empty())
        .collect();
    assert_eq!(said.len(), 1, "exactly one row says it: {interior:?}");
    assert_eq!(said[0].trim(), "pane closed");

    // Usable: the rows above the region still hold the panes this terminal is
    // drawing, and the peek closes.
    let above = region(&rendered, 1, 1, rect.y - 1, 78);
    assert!(
        above.iter().all(|row| !row.trim().is_empty()),
        "a dead peek must not blank the screen above it: {above:?}",
    );
    app.close_peek().await.expect("close the peek");
    assert_eq!(app.peeked(), None);
    assert_eq!(app.peek_layout().peek, None);

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn under_the_narrow_projection_the_peek_replaces_the_list() {
    let server = support::Server::start("peek-narrow").await;
    // Below the shipped 60-column threshold, and a workspace of two panes, so
    // `Projection::Single` is what this client draws (D14, `app/narrow.rs`).
    let pty = support::open_pty_sized(20, 40);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");
    let shown = PaneId::new_v4();
    let other = PaneId::new_v4();
    show_workspace(&mut app, &[shown, other]);
    fabricate(&mut app, shown, 0, 19, 40);

    let watched = PaneId::new_v4();
    app.open_peek(watched).await.expect("open the peek");
    fabricate(&mut app, watched, 1, 19, 40);

    let layout = app.peek_layout();
    assert_eq!(layout.list.h, 0, "10 §D14: the peek replaces the list");
    let rect = layout.peek.expect("a peek is open");
    assert_eq!(
        (rect.x, rect.y, rect.w, rect.h),
        (0, 0, 40, 19),
        "the whole content area, and no border to pay for",
    );

    app.repaint();
    let rendered = support::rasterize(app.frame());
    let drawn = region(&rendered, 0, 0, 19, 40);
    let expected: Vec<String> = (0..19)
        .map(|row| (0..40).map(|col| glyph(1, row, col)).collect())
        .collect();
    assert_eq!(
        drawn, expected,
        "the peek fills the content area full-bleed"
    );

    drop(app);
    server.shutdown().await;
}
