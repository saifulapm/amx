//! Whether a frame is owed, and what one draws.
//!
//! The tail of a repaint lives next door — [`super::overlay`] for the surfaces
//! drawn over the panes, [`super::status`] for the status line and the cursor,
//! [`super::narrow`] for the projection all three are drawn under. This file is
//! the frame itself: the drained batch, the rects it recomputes, the borders and
//! the grids.
//!
//! # Task ownership
//!
//! **X12** split this out of [`super`], which the narrow projection's fields and
//! the branch on it pushed past the 500-line soft budget (R-M1-3: no split waits
//! for the hard limit). It is the responsibility that comes out whole — every
//! other method left in `mod.rs` is the app's shape, its lifecycle or an input
//! consequence, and none of them writes to the frame buffer.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Level, PaneId};

use super::{App, Projection};
use crate::render::{chrome, grid};

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Whether the next [`repaint`](Self::repaint) would put something new on
    /// this terminal.
    ///
    /// What the effect fold buys over the boolean it replaced. Pane damage
    /// names its pane, and a pane this terminal is not drawing owes it no
    /// frame: stream bindings are per *connection*, not per workspace, so a
    /// grid stream bound while a workspace was on screen keeps delivering after
    /// this client has switched away from it, and every one of those deltas
    /// used to repaint a screen they could not appear on. Anything at
    /// [`Level::Layout`] or above is owed unconditionally — the chrome, the
    /// status line and the rects are not any pane's.
    ///
    /// Exposed for the same reason [`App::repaints`] is: what a client decided
    /// not to draw is invisible in the frame and exact here.
    #[must_use]
    pub fn frame_due(&self) -> bool {
        match self.effects.level() {
            Level::Nothing => false,
            Level::PaneDamage => self.effects.panes().iter().any(|&pane| self.showing(pane)),
            Level::Layout | Level::Full => true,
        }
    }

    /// Whether this terminal is drawing `pane`.
    ///
    /// Under [`Projection::Single`] that is one pane and not the workspace's
    /// whole layout: the grid streams for the panes D14 does not draw stay
    /// bound ([`super::narrow`]'s header says why), so on the phone that
    /// projection exists for, a flooding agent in a pane nobody is looking at
    /// would otherwise repaint the screen at its own rate.
    fn showing(&self, pane: PaneId) -> bool {
        match self.projection() {
            Projection::Single(shown) => shown == pane,
            Projection::Tiled => self
                .model
                .focused_workspace()
                .is_some_and(|ws| ws.layout.contains(pane)),
        }
    }

    /// Redraw everything: chrome and every pane this projection draws.
    ///
    /// Drains one batch of folded effects and draws it. The frame itself is
    /// always whole — this client emits absolute cursor moves into a buffer it
    /// rebuilds per frame, and there is no partial-frame path to take — so what
    /// the batch decides is the *layout*: recomputing the rects is the one
    /// thing a repaint can skip, and [`Level::Layout`] is when it must not.
    ///
    /// Allocation-free once the layout has settled: the only thing that can
    /// grow a buffer here is `FrameWriter`'s own `Vec<u8>` reaching a new
    /// high-water mark, which happens at most once per distinct frame size.
    /// [`App::projection`] is asked before the batch is examined and answers
    /// off a width comparison for every client wide enough to tile, which is
    /// what keeps that true for the clients the property was measured on.
    pub fn repaint(&mut self) {
        self.effects.drain_into(&mut self.scheduled);
        self.writer.begin_frame();
        let projection = self.projection();
        if self.scheduled.level() >= Level::Layout {
            let content = self.model.content_area();
            self.pane_rects.clear();
            match projection {
                // The whole content area, and nothing else in it: no slot, no
                // border, no other pane (D14 — see [`super::narrow`]).
                Projection::Single(pane) => self.pane_rects.push((pane, content)),
                Projection::Tiled => {
                    let rects = self.model.pane_rects(content);
                    self.pane_rects.extend_from_slice(&rects);
                }
            }
        }
        for &(pane, rect) in &self.pane_rects {
            let inner = match projection {
                Projection::Single(_) => rect,
                Projection::Tiled => {
                    chrome::draw_border(&mut self.writer, rect);
                    chrome::inset(rect)
                }
            };
            if let Some(pane_grid) = self.model.pane(pane) {
                grid::blit(&mut self.writer, pane_grid, inner);
            }
        }
        self.draw_overlays();
        self.draw_status();
        match projection {
            Projection::Single(pane) => self.place_cursor_full_bleed(pane),
            Projection::Tiled => self.place_cursor(),
        }
        self.repaints += 1;
    }

    /// The bytes the last [`Self::repaint`] produced.
    #[must_use]
    pub fn frame(&self) -> &[u8] {
        self.writer.bytes()
    }
}
