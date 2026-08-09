//! D14's narrow-viewport projection: what this client draws below
//! `client.narrow_cols`, and what it declares while it is drawing it.
//!
//! A phone's SSH terminal meets a client that assumes a wide viewport. The
//! shipped projection tiles the whole layout, so five agents in 45 columns are
//! five boxes of which the widest is 21 cells and one is one cell wide, with a
//! permission dialog word-wrapped across four rows — measured, not argued
//! (`docs/notes/m4-live-smoke.md` §1.3). D14's answer is a *projection* change
//! and never a layout one: below the threshold this client draws one pane and
//! the layout tree is untouched, so the desktop watching the same session goes
//! on drawing all five.
//!
//! # The one pane fills the content area, and the server has to agree
//!
//! 10 §D14 as first written said the server and the pane grids were untouched
//! by this. They cannot be: `Core` sizes every pane's PTY to its slot in the
//! declared viewport, so a client showing one pane full-screen while declaring
//! a 45-column viewport over a five-pane layout would be handed a 21-column
//! grid to letterbox inside 45 — correct, and useless. D-M4-7 is the
//! amendment, and it is one rule on each side of the socket:
//!
//! - here: below the threshold, [`Projection::Single`] draws the focused pane
//!   over the whole content area with **no border**, and
//!   [`App::report_viewport`](super::App::report_viewport) declares
//!   `Viewport { rows, cols, panes: [that pane] }`;
//! - there: a declaration naming one pane of a layout that holds more sizes
//!   that pane to the whole content area (`amx-server/src/actor/core/view.rs`).
//!
//! The declaration is the only thing that carries the rule across, which is why
//! the two halves are worded from the same fact — *the layout holds more panes
//! than this client declared* — rather than from "the client is narrow", which
//! the server cannot see. It is also why a workspace holding exactly one pane
//! stays [`Projection::Tiled`] however narrow the terminal is: that pane's
//! border is what the tiled projection already draws, the server already sizes
//! it to the interior, and a single-pane projection here would ask for a grid
//! two cells wider than the box drawn for it.
//!
//! # What it deliberately does not do
//!
//! - **No layout mutation, in either direction.** Crossing the threshold sends
//!   no `pane.close` and no `pane.split`; the server's tree is byte-identical
//!   on both sides of it, and the panes that are not drawn keep running.
//! - **No re-binding.** The grid streams stay bound for every pane of the
//!   focused workspace, so moving between panes on a phone costs no keyframe.
//!   `Viewport.panes` is a sizing declaration in M4, not a traffic filter — it
//!   never had a reader for the traffic clause and X15's peek needs a bind
//!   outside the declaration to stay legitimate.
//! - **No new keys.** Split navigation still moves focus over the real tiling
//!   (`Layout::neighbour` works off the tree, not off what is drawn), so `hjkl`
//!   changes which pane is shown instead of splitting a screen that has no room
//!   to split.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{PaneId, Rect};

use super::App;
use crate::render::chrome;

/// What this client is drawing of the focused workspace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Projection {
    /// Every pane of the focused workspace, each inside its own border — the
    /// shipped projection, and what any terminal wide enough to afford it
    /// draws.
    Tiled,
    /// One pane, filling the content area with no border at all (D14).
    Single(PaneId),
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// What this client is drawing right now.
    ///
    /// Pure, and derived rather than stored: the three inputs — the configured
    /// threshold, this terminal's width and the focused workspace's layout —
    /// all move on their own, and a cached answer is the stale-flag bug D2
    /// exists to prevent, one level up from the effects the module doc
    /// describes.
    #[must_use]
    pub fn projection(&self) -> Projection {
        if !self.narrow.is_narrow(self.model.term.w) {
            return Projection::Tiled;
        }
        let Some(ws) = self.model.focused_workspace() else {
            return Projection::Tiled;
        };
        // A one-pane workspace is already what the tiled projection draws, and
        // the server reads the resulting declaration the same way. See the
        // module header.
        if ws.layout.panes().len() < 2 {
            return Projection::Tiled;
        }
        self.shown_pane()
            .map_or(Projection::Tiled, Projection::Single)
    }

    /// The one pane [`Projection::Single`] shows.
    ///
    /// The zoomed pane when the layout has one — zoom is already a single-pane
    /// projection (`Layout::rects` returns it alone) and the two must not
    /// disagree about which pane that is — and otherwise this terminal's
    /// recorded focus, self-healing to the layout's first pane the way
    /// [`App::focused_pane`](super::App::focused_pane) does. Read-only, because
    /// a repaint reports what the client knows and healing focus is an input
    /// consequence.
    pub(super) fn shown_pane(&self) -> Option<PaneId> {
        let workspace = self.model.focused_workspace_id()?;
        let layout = &self.model.focused_workspace()?.layout;
        if let Some(zoomed) = layout.zoomed() {
            return Some(zoomed);
        }
        if let Some(&pane) = self.focus.get(&workspace)
            && layout.contains(pane)
        {
            return Some(pane);
        }
        layout.panes().first().copied()
    }

    /// The cells inside a pane's slot: the slot itself under
    /// [`Projection::Single`], which draws no border, and the border's interior
    /// otherwise.
    ///
    /// Every surface drawn *over* a pane — copy mode's rows, the cursor —
    /// measures itself against this rather than against `chrome::inset`, so a
    /// full-bleed pane does not have an inset drawn into it that nothing put
    /// there.
    #[must_use]
    pub(super) fn pane_interior(&self, rect: Rect) -> Rect {
        match self.projection() {
            Projection::Single(_) => rect,
            Projection::Tiled => chrome::inset(rect),
        }
    }

    /// Park the terminal cursor on the shown pane's cursor cell.
    ///
    /// [`Projection::Single`]'s own placement, beside the projection it belongs
    /// to: the tiled one is `super::status::place_cursor`, and the two differ by
    /// exactly the border this projection does not draw.
    pub(super) fn place_cursor_full_bleed(&mut self, pane: PaneId) {
        if self.overlay_open() {
            self.writer.set_cursor_visible(false);
            return;
        }
        let placed = (|| {
            let &(_, rect) = self.pane_rects.iter().find(|(id, _)| *id == pane)?;
            let grid = self.model.pane(pane)?;
            let cursor = grid.cursor();
            (cursor.visible && cursor.row < rect.h && cursor.col < rect.w)
                .then(|| (rect.y + cursor.row, rect.x + cursor.col))
        })();
        match placed {
            Some((row, col)) => {
                self.writer.move_to(row, col);
                self.writer.set_cursor_visible(true);
            }
            None => self.writer.set_cursor_visible(false),
        }
    }
}
