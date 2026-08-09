//! What a `SIGWINCH` does: note it, let the burst settle, act once.
//!
//! A terminal emulator coalesces some resize signals and not others, depending
//! on how the user dragged, so the client debounces rather than acting on each:
//! every signal overwrites the pending size and [`App::settle_resize`] acts on
//! the last one after [`RESIZE_DEBOUNCE`] of quiet. The wired loop's own arm is
//! what runs the clock ([`super::wired`]); what the settle then *does* — declare
//! the new viewport, drop the caches whose row widths may have reflowed — lives
//! there beside it.
//!
//! # Task ownership
//!
//! **X14** split this out of [`super`], which crossed the 500-line soft budget
//! when the agents view's field and its input routing landed on top of X15's
//! peek (R-M1-3, and this milestone's rule that no split waits for the hard
//! limit). It is the responsibility that comes out whole: three methods and one
//! constant, all about one signal, and nothing else in `mod.rs` reads
//! `pending_resize`.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Effect, Rect};

use super::App;
use crate::term::TermSize;

/// How long a burst of `SIGWINCH` is allowed to keep arriving before it is
/// treated as settled and acted on once.
pub const RESIZE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(60);

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Note that a resize happened; does not yet act on it.
    ///
    /// Called once per `SIGWINCH` in [`App::run`]. A burst of signals each
    /// overwrite the pending size rather than queuing, so [`Self::settle_resize`]
    /// always acts on the *last* one.
    pub fn note_resize(&mut self, size: TermSize) {
        self.pending_resize = Some(size);
    }

    /// Whether a resize is waiting to be settled.
    #[must_use]
    pub const fn has_pending_resize(&self) -> bool {
        self.pending_resize.is_some()
    }

    /// Act on the pending resize, if any: apply it to the model, report it
    /// once through `report`, and repaint once.
    ///
    /// Returns whether a resize was actually settled, so a caller (and a test)
    /// can tell "nothing was pending" from "one was applied".
    pub fn settle_resize(&mut self, report: &mut impl FnMut(TermSize)) -> bool {
        let Some(size) = self.pending_resize.take() else {
            return false;
        };
        self.model.term = Rect::new(0, 0, size.cols, size.rows);
        self.absorb(Effect::Layout);
        report(size);
        self.repaint();
        true
    }
}
