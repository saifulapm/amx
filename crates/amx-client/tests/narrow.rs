//! X12 acceptance: D14's narrow-viewport projection, client side.
//!
//! The failure it removes is on the record rather than argued —
//! `docs/notes/m4-live-smoke.md` §1.3 ran a real attach on a 45×20 pty against
//! a five-pane workspace and got five boxes, the widest 21 cells and one of
//! them one cell wide, with the permission dialog the user was supposed to
//! answer wrapped across four rows. Below `client.narrow_cols` this client now
//! draws one pane over the whole content area, and declares that so the server
//! sizes the pane to the screen instead of to a slot nobody is drawing
//! (D-M4-7 — `crates/amx-server/tests/narrow_viewport.rs` is the other half).
//!
//! This file is the *projection*: a pure function of the threshold, this
//! terminal's width and the layout, driven over an adopted mirror because a
//! mirror is the only way to pin the rendered cells exactly. [`wire`] is the
//! other shape — a real server over a real socket — because the declaration is
//! the half that crosses, and a projection nobody declared would letterbox
//! exactly as it did before.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

// A test crate root's module directory is `tests/`, so the halves of this suite
// need their path spelled out to live beside it rather than beside everybody's.
#[path = "narrow/wire.rs"]
mod wire;

use std::collections::HashMap;

use amx_client::app::{App, Projection};
use amx_client::model::{Attrs, Cell, WorkspaceModel};
use amx_client::term::TermSize;
use amx_core::{Direction, GridGeneration, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::stream::{Cursor, CursorShape};

/// The `App` these tests drive: a real pty fd in, a byte buffer out.
pub type TestApp = App<std::fs::File, Vec<u8>>;

/// The phone-sized terminal the wave-1 smoke ran its second pty at.
pub const NARROW: (u16, u16) = (20, 45);
/// A terminal with room to tile, for the control arm of every assertion.
const WIDE: (u16, u16) = (24, 80);

/// How this suite identifies itself in the handshake.
pub fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-narrow-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// A client at `rows`x`cols` over a mirrored three-pane workspace, each pane's
/// grid filled with one repeated letter so a rasterized frame says which pane
/// a cell came from.
struct Fixture {
    server: support::Server,
    _master: std::fs::File,
    app: TestApp,
    panes: [PaneId; 3],
}

async fn fixture(tag: &str, (rows, cols): (u16, u16)) -> Fixture {
    let server = support::Server::start(tag).await;
    let pty = support::open_pty_sized(rows, cols);
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server");

    let panes = [PaneId::new_v4(), PaneId::new_v4(), PaneId::new_v4()];
    let mut layout = BspLayout::with_root(panes[0]);
    layout
        .split(panes[0], Direction::Right, panes[1], 0.5)
        .expect("split the mirrored layout");
    layout
        .split(panes[1], Direction::Right, panes[2], 0.5)
        .expect("split the mirrored layout again");
    app.adopt_workspace(
        WorkspaceId::new_v4(),
        WorkspaceModel {
            label: None,
            layout,
        },
    );
    for (n, &pane) in panes.iter().enumerate() {
        fill(&mut app, pane, letter(n), rows, cols);
    }
    Fixture {
        server,
        _master: pty.master,
        app,
        panes,
    }
}

/// The letter pane `n`'s grid is filled with.
fn letter(n: usize) -> char {
    ['A', 'B', 'C'][n]
}

/// Give `pane` a grid of `rows`x`cols` cells, every one of them `ch`.
///
/// Sized to the whole terminal so no assertion below can pass by accident on a
/// grid too small to reach the edges it is checking, and with the cursor
/// hidden so the frame's trailing `CSI ?25l` lands past the status line: the
/// harness's rasterizer tracks cursor *positioning* and reads the three bytes
/// of a visibility sequence as cells, which would otherwise stamp three
/// characters into whichever cell the cursor was parked on.
fn fill(app: &mut TestApp, pane: PaneId, ch: char, rows: u16, cols: u16) {
    let cells: Vec<Cell> = (0..u32::from(rows) * u32::from(cols))
        .map(|_| Cell {
            ch,
            attrs: Attrs::default(),
        })
        .collect();
    app.model().pane_mut(pane, rows, cols).apply_reset(
        GridGeneration::FIRST.next(),
        rows,
        cols,
        &cells,
        Cursor {
            row: 0,
            col: 0,
            visible: false,
            shape: CursorShape::default(),
            blink: false,
        },
    );
}

/// Which letters the rendered frame holds, and how many cells each covers.
fn letters(rendered: &HashMap<(u16, u16), char>) -> HashMap<char, usize> {
    let mut counts = HashMap::new();
    for ch in rendered.values() {
        *counts.entry(*ch).or_insert(0) += 1;
    }
    counts
}

/// A 45-column client draws one pane over the whole content area, with no
/// border and no sign of the panes it is not drawing.
#[tokio::test]
async fn below_the_threshold_one_pane_fills_the_content_area() {
    let mut fx = fixture("narrow-draw", NARROW).await;
    let shown = fx.app.focused_pane().expect("a focused pane");
    assert_eq!(
        fx.app.projection(),
        Projection::Single(shown),
        "45 columns is under the shipped 60-column threshold",
    );

    fx.app.repaint();
    let rendered = support::rasterize(fx.app.frame());
    let counts = letters(&rendered);

    let index = fx
        .panes
        .iter()
        .position(|&p| p == shown)
        .expect("of the three");
    let (rows, cols) = NARROW;
    let content = usize::from(rows - 1) * usize::from(cols);
    assert_eq!(
        counts.get(&letter(index)).copied().unwrap_or(0),
        content,
        "the shown pane covers every cell above the status line",
    );
    for other in 0..3 {
        if other == index {
            continue;
        }
        assert_eq!(
            counts.get(&letter(other)).copied().unwrap_or(0),
            0,
            "a pane the narrow projection is not drawing puts no cell on the screen",
        );
    }
    for edge in ['┌', '┐', '└', '┘', '│', '─'] {
        assert_eq!(
            counts.get(&edge).copied().unwrap_or(0),
            0,
            "the single-pane projection draws no border: found {edge:?}",
        );
    }

    fx.server.shutdown().await;
}

/// The same layout in a terminal with room for it still tiles, borders and
/// all: the policy is keyed on width and nothing else.
#[tokio::test]
async fn above_the_threshold_the_same_layout_still_tiles() {
    let mut fx = fixture("narrow-wide", WIDE).await;
    assert_eq!(fx.app.projection(), Projection::Tiled);

    fx.app.repaint();
    let counts = letters(&support::rasterize(fx.app.frame()));
    for n in 0..3 {
        assert!(
            counts.get(&letter(n)).copied().unwrap_or(0) > 0,
            "every pane of a tiled workspace is on the screen",
        );
    }
    assert!(
        counts.get(&'┌').copied().unwrap_or(0) >= 3,
        "one border corner per pane, at least",
    );

    fx.server.shutdown().await;
}

/// A workspace holding one pane is what the tiled projection already draws, so
/// it stays tiled however narrow the terminal is — and it must, because the
/// server reads its declaration the same way and sizes it inside its border.
#[tokio::test]
async fn a_workspace_of_one_pane_is_never_the_narrow_projection() {
    let mut fx = fixture("narrow-solo", NARROW).await;
    let pane = PaneId::new_v4();
    fx.app.adopt_workspace(
        WorkspaceId::new_v4(),
        WorkspaceModel {
            label: None,
            layout: BspLayout::with_root(pane),
        },
    );
    let (rows, cols) = NARROW;
    fill(&mut fx.app, pane, 'Z', rows, cols);

    assert_eq!(fx.app.projection(), Projection::Tiled);
    fx.app.repaint();
    let counts = letters(&support::rasterize(fx.app.frame()));
    assert!(
        counts.get(&'┌').copied().unwrap_or(0) > 0,
        "the sole pane keeps the border the tiled projection has always drawn",
    );

    fx.server.shutdown().await;
}

/// Split navigation changes which pane is *shown* rather than splitting a
/// screen with no room to split.
#[tokio::test]
async fn split_navigation_moves_the_shown_pane() {
    let mut fx = fixture("narrow-nav", NARROW).await;
    let first = fx.app.focused_pane().expect("a focused pane");

    // `prefix w` enters navigate; `l` is focus-right over the real tiling,
    // which the layout tree still describes whatever this client is drawing.
    let mut seen = Vec::new();
    fx.app
        .handle_input(b"\x01wl", &mut |event| seen.push(format!("{event:?}")));
    let second = fx.app.focused_pane().expect("still a focused pane");
    assert_ne!(second, first, "focus-right moved to the next pane");
    assert_eq!(fx.app.projection(), Projection::Single(second));

    fx.app.repaint();
    let counts = letters(&support::rasterize(fx.app.frame()));
    let index = fx
        .panes
        .iter()
        .position(|&p| p == second)
        .expect("of the three");
    assert!(
        counts.get(&letter(index)).copied().unwrap_or(0) > 0,
        "the newly focused pane is the one on the screen",
    );
    assert!(
        seen.iter().any(|event| event.contains("PaneFocus")),
        "and the server's own focus is still echoed: {seen:?}",
    );

    fx.server.shutdown().await;
}

/// Crossing the threshold in both directions is a projection change: the
/// mirrored layout tree — server truth, the only thing a mutation could show
/// up in — is identical on both sides of it.
#[tokio::test]
async fn crossing_the_threshold_mutates_no_layout() {
    let mut fx = fixture("narrow-cross", WIDE).await;
    let tree = |app: &mut TestApp| {
        serde_json::to_value(
            app.model()
                .focused_workspace()
                .expect("the adopted workspace")
                .layout
                .clone(),
        )
        .expect("encode the layout")
    };
    let before = tree(&mut fx.app);
    assert_eq!(fx.app.projection(), Projection::Tiled);

    let (rows, cols) = NARROW;
    fx.app.note_resize(TermSize { rows, cols });
    assert!(fx.app.settle_resize(&mut |_| {}));
    assert!(matches!(fx.app.projection(), Projection::Single(_)));
    assert_eq!(tree(&mut fx.app), before, "narrowing rewrites no tree");

    let (rows, cols) = WIDE;
    fx.app.note_resize(TermSize { rows, cols });
    assert!(fx.app.settle_resize(&mut |_| {}));
    assert_eq!(fx.app.projection(), Projection::Tiled);
    assert_eq!(tree(&mut fx.app), before, "and neither does widening");
    assert_eq!(
        fx.app
            .model()
            .focused_workspace()
            .expect("the adopted workspace")
            .layout
            .panes()
            .len(),
        3,
        "all three panes are still there, having never been closed",
    );

    fx.server.shutdown().await;
}

/// The picker fills the screen under the narrow projection instead of
/// reserving eight rows of twenty and leaving a pane showing under the rest.
#[tokio::test]
async fn the_picker_is_full_screen_under_the_narrow_projection() {
    let mut fx = fixture("narrow-pick", NARROW).await;
    fx.app.handle_input(b"\x01p", &mut |_| {});
    assert!(fx.app.picker_open());
    fx.app.repaint();

    let rendered = support::rasterize(fx.app.frame());
    let (rows, cols) = NARROW;
    let shown = fx.app.focused_pane().expect("a focused pane");
    let index = fx
        .panes
        .iter()
        .position(|&p| p == shown)
        .expect("of the three");
    for row in 0..rows - 1 {
        for col in 0..cols {
            let cell = rendered.get(&(row, col)).copied().unwrap_or(' ');
            assert_ne!(
                cell,
                letter(index),
                "row {row} col {col} still shows the pane behind the picker",
            );
        }
    }

    fx.server.shutdown().await;
}
