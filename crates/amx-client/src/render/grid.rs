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
//!
//! # Where a clip is taken from, and why it moved
//!
//! A clip keeps the **bottom-left** of the grid: the last rows, and each of
//! those rows from its first column. The padding of a letterbox still centres,
//! which is only ever a question about blank space.
//!
//! It used to centre both ways, and that was wrong in a way only a live run
//! showed. X14's smoke opened D15's peek on a 27-row pane in a 12-row region
//! and the region read as *empty*: an agent paints its dialog at the bottom of
//! its screen after scrolling everything above it off, so the middle twelve
//! rows were genuinely blank — confirmed against `pane read` on the same pane
//! in the same run. D15's peek exists to answer "what is this agent blocked
//! on", and a crop that cannot show the last line cannot answer it; that is the
//! same premise `agent.list`'s `last_line` is built on.
//!
//! It is one rule for every surface rather than a peek-only exception, on two
//! grounds. A terminal writes downward from column zero, so the newest rows and
//! the starts of lines are where its content is, whichever surface is showing
//! it — a tiled pane clipped by a client narrower than the size authority hid
//! the same thing for the same reason. And a peeked pane and a focused pane
//! must render identically (X18's own acceptance), which a second crop rule
//! would quietly end. 04 §3 says "letterbox/clip" and does not say from where,
//! so nothing above this file is contradicted.

use amx_core::Rect;

use crate::model::{Cell, PaneGrid};
use crate::render::FrameWriter;

/// Paint `grid` into `target`, centering it when smaller and cropping to its
/// bottom-left when larger.
///
/// `target` is always fully repainted, padding included, so a pane that
/// shrank between frames leaves no stale cells behind it.
pub fn blit(writer: &mut FrameWriter, grid: &PaneGrid, target: Rect) {
    let (visible_cols, x_pad, x_src) = fit(grid.cols(), target.w, Keep::Head);
    let (visible_rows, y_pad, y_src) = fit(grid.rows(), target.h, Keep::Tail);
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

/// Which end of an axis a clip keeps when the whole of it will not fit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Keep {
    /// The start: column zero, where every line's text begins.
    Head,
    /// The end: the last rows, where a terminal is currently writing.
    Tail,
}

/// How much of `content` fits in `avail`, where in `avail` it starts, and
/// where in `content` it starts reading from.
///
/// `content <= avail`: the whole thing shows, centered (a letterbox).
/// `content > avail`: `avail` cells show, taken from the `keep` end of
/// `content` (a clip) — the padding return is always `0` in this branch
/// because there is no room left to pad.
fn fit(content: u16, avail: u16, keep: Keep) -> (u16, u16, u16) {
    if content <= avail {
        return (content, (avail - content) / 2, 0);
    }
    let source = match keep {
        Keep::Head => 0,
        Keep::Tail => content - avail,
    };
    (avail, 0, source)
}
