//! Pane grid blitting: letterbox or clip, never resize (04 §3).
//!
//! "Non-active clients letterbox/clip the server-sized pane grids inside
//! their own locally-computed chrome" rather than asking the server to
//! resize the pane to fit. [`blit`] is the one function that reconciles a
//! [`PaneGrid`] (always exactly the server's size) against a target
//! [`Rect`] (this client's own layout projection): when the grid is smaller
//! than its slot the remainder is padding (a letterbox), and when it is
//! larger the surplus is simply not drawn (a clip) — the grid itself is
//! never touched either way.

use amx_core::Rect;

use crate::model::{Cell, PaneGrid};
use crate::render::FrameWriter;

/// Paint `grid` into `target`, centering it when smaller and center-cropping
/// it when larger.
///
/// `target` is always fully repainted, padding included, so a pane that
/// shrank between frames leaves no stale cells behind it.
pub fn blit(writer: &mut FrameWriter, grid: &PaneGrid, target: Rect) {
    let (visible_cols, x_pad, x_src) = fit(grid.cols(), target.w);
    let (visible_rows, y_pad, y_src) = fit(grid.rows(), target.h);
    let blank = Cell::default();

    for row in 0..target.h {
        writer.move_to(target.y + row, target.x);
        let row_visible = row >= y_pad && row < y_pad + visible_rows;
        for col in 0..target.w {
            let col_visible = col >= x_pad && col < x_pad + visible_cols;
            if row_visible && col_visible {
                let cell = grid
                    .cell(y_src + (row - y_pad), x_src + (col - x_pad))
                    .unwrap_or(&blank);
                writer.write_cell(cell);
            } else {
                writer.write_cell(&blank);
            }
        }
    }
}

/// Paint `target` blank, with `text` centered on its middle row.
///
/// What a slot shows when there is no grid to blit into it. D15's peek region
/// has two such cases and they are not the same fact: a pane the session no
/// longer holds, which is told (`text`), and a pane whose first keyframe is
/// still a round trip away, which is not (`text` empty) — a message that
/// flashed for one frame every time a peek opened would call every healthy
/// agent dead.
///
/// The region is repainted whole either way, for [`blit`]'s own reason: a slot
/// that stopped being drawn would leave the previous frame's cells under it.
/// A `text` wider than `target` is clipped rather than wrapped — this is a slot,
/// not a paragraph.
pub fn blit_absent(writer: &mut FrameWriter, target: Rect, text: &str) {
    let label = text.chars().take(usize::from(target.w)).collect::<Vec<_>>();
    let width = u16::try_from(label.len()).unwrap_or(target.w);
    let row = target.h / 2;
    let pad = (target.w - width) / 2;
    let blank = Cell::default();

    for line in 0..target.h {
        writer.move_to(target.y + line, target.x);
        for col in 0..target.w {
            let at = (line == row && col >= pad).then(|| label.get(usize::from(col - pad)));
            match at.flatten() {
                Some(&ch) => writer.write_cell(&Cell {
                    ch,
                    ..Cell::default()
                }),
                None => writer.write_cell(&blank),
            }
        }
    }
}

/// How much of `content` fits in `avail`, where in `avail` it starts, and
/// where in `content` it starts reading from.
///
/// `content <= avail`: the whole thing shows, centered (a letterbox).
/// `content > avail`: `avail` cells show, centered on `content` (a
/// center-crop) — the padding return is always `0` in this branch because
/// there is no room left to pad.
fn fit(content: u16, avail: u16) -> (u16, u16, u16) {
    if content <= avail {
        (content, (avail - content) / 2, 0)
    } else {
        (avail, 0, (content - avail) / 2)
    }
}
