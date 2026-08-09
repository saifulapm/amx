//! Where a clip is taken from when a grid does not fit its slot.
//!
//! X14's live smoke opened D15's peek on a 27-row pane in a 12-row region and
//! the region read as empty: the crop was centred, an agent paints its dialog
//! at the bottom of its screen after scrolling everything above it off, and the
//! middle twelve rows were genuinely blank. A monitor that cannot show the last
//! line cannot answer the question D15's peek exists to ask.
//!
//! So a clip keeps the bottom-left — the last rows, each from its first column
//! — for every surface that blits, which `render::grid`'s header argues at
//! length. Fails without the change: the centred crop shows the middle rows and
//! the middle columns, and the pane's last line is nowhere on screen.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::App;
use amx_client::model::{Attrs, Cell, PaneGrid, WorkspaceModel};
use amx_client::render::{FrameWriter, grid};
use amx_core::{GridGeneration, Layout as BspLayout, PaneId, Rect, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::stream::{Cursor, CursorShape};

fn cursor() -> Cursor {
    Cursor {
        row: 0,
        col: 0,
        visible: false,
        shape: CursorShape::default(),
        blink: false,
    }
}

/// A grid whose every cell names its own position: `row` in the first column,
/// `col` in the rest, both taken modulo the alphabet.
fn labelled(rows: u16, cols: u16) -> PaneGrid {
    let cells: Vec<Cell> = (0..rows)
        .flat_map(|row| {
            (0..cols).map(move |col| Cell {
                ch: char::from(if col == 0 {
                    b'A' + (row % 26) as u8
                } else {
                    b'a' + (col % 26) as u8
                }),
                attrs: Attrs::default(),
            })
        })
        .collect();
    let mut into = PaneGrid::blank(rows, cols);
    into.apply_reset(GridGeneration::FIRST.next(), rows, cols, &cells, cursor());
    into
}

/// Blit `grid` into a `rows` x `cols` slot at the screen origin and read back
/// what landed there.
fn blitted(grid: &PaneGrid, rows: u16, cols: u16) -> Vec<String> {
    let mut writer = FrameWriter::new();
    writer.begin_frame();
    grid::blit(&mut writer, grid, Rect::new(0, 0, cols, rows));
    let rendered = support::rasterize(writer.bytes());
    (0..rows)
        .map(|row| {
            (0..cols)
                .map(|col| rendered.get(&(row, col)).copied().unwrap_or(' '))
                .collect()
        })
        .collect()
}

#[test]
fn a_grid_taller_than_its_slot_shows_its_last_rows() {
    let grid = labelled(27, 8);
    let shown = blitted(&grid, 12, 8);

    // Rows 15..27 of the pane, in order — its bottom, where a terminal writes.
    let first_column: String = shown
        .iter()
        .map(|row| row.chars().next().unwrap())
        .collect();
    let expected: String = (15..27).map(|row| char::from(b'A' + (row % 26))).collect();
    assert_eq!(
        first_column, expected,
        "a clipped pane shows its last rows: {shown:?}",
    );
}

#[test]
fn a_grid_wider_than_its_slot_shows_the_start_of_every_line() {
    let grid = labelled(4, 40);
    let shown = blitted(&grid, 4, 10);

    let expected: String = std::iter::once('A')
        .chain((1..10).map(|col| char::from(b'a' + (col % 26))))
        .collect();
    assert_eq!(
        shown[0], expected,
        "a clipped pane shows each line from its first column: {shown:?}",
    );
}

#[test]
fn a_grid_smaller_than_its_slot_is_still_centred() {
    // The padding rule did not move: it is only ever a question about blank
    // space, and `tests/letterbox.rs` is its acceptance. Asserted here too so a
    // later change to `fit` cannot take the letterbox with it.
    let grid = labelled(2, 4);
    let shown = blitted(&grid, 6, 10);

    assert_eq!(shown[0].trim(), "", "the padding above stays blank");
    assert_eq!(shown[5].trim(), "", "the padding below stays blank");
    assert_eq!(
        shown[2], "   Abcd   ",
        "the grid sits in the middle: {shown:?}"
    );
    assert_eq!(
        shown[3], "   Bbcd   ",
        "the grid sits in the middle: {shown:?}"
    );
}

/// The live-smoke case, end to end: a peek region half the height of the pane
/// it is watching, and the pane's own last line the only thing written on it.
#[tokio::test]
async fn a_peek_shorter_than_the_pane_shows_the_line_the_agent_is_waiting_on() {
    let server = support::Server::start("crop-peek").await;
    let pty = support::open_pty_sized(30, 40);
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        ClientInfo {
            name: "amx-crop-test".to_owned(),
            version: "0.0.0".to_owned(),
            term: None,
        },
    )
    .await
    .expect("attach to the real server");

    let shown = PaneId::new_v4();
    let workspace = WorkspaceId::new_v4();
    app.model().set_workspace(
        workspace,
        WorkspaceModel {
            label: None,
            layout: BspLayout::with_root(shown),
        },
    );
    app.model().focus_workspace(workspace);

    // The smoke's pane: 27 rows, everything scrolled off, the dialog on the
    // last line. The peeked pane is in no workspace this client mirrors, which
    // is what an agent in another project looks like from here.
    let watched = PaneId::new_v4();
    app.open_peek(watched).await.expect("open the peek");
    let (rows, cols) = (27_u16, 38_u16);
    const WAITING: &str = "Do you want to proceed?";
    let mut cells = vec![Cell::default(); usize::from(rows) * usize::from(cols)];
    for (at, ch) in WAITING.chars().enumerate() {
        cells[usize::from(rows - 1) * usize::from(cols) + at] = Cell {
            ch,
            attrs: Attrs::default(),
        };
    }
    app.model().pane_mut(watched, rows, cols).apply_reset(
        GridGeneration::FIRST.next(),
        rows,
        cols,
        &cells,
        cursor(),
    );

    let rect = app
        .peek_layout()
        .peek
        .expect("the screen has room for a peek");
    assert!(
        rect.h < rows,
        "the region has to be shorter than the pane for this to be the case at all",
    );

    app.repaint();
    let rendered = support::rasterize(app.frame());
    let interior: Vec<String> = (0..rect.h - 2)
        .map(|row| {
            (0..rect.w - 2)
                .map(|col| {
                    rendered
                        .get(&(rect.y + 1 + row, rect.x + 1 + col))
                        .copied()
                        .unwrap_or(' ')
                })
                .collect()
        })
        .collect();

    assert_eq!(
        interior.last().map(|row| row.trim()),
        Some(WAITING),
        "the peek must end on the pane's last line: {interior:?}",
    );

    drop(app);
    server.shutdown().await;
}
