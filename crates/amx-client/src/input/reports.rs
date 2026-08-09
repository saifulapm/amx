//! The machine's mouse half: the per-pane gate, the relay, the carry, and the
//! split a chrome surface reads its bytes through.
//!
//! Split out of [`super`] by X13 (R-M1-3, and the R-M4-5 rule that no split
//! waits for the hard limit): the mouse path took `input/mod.rs` from 443 lines
//! to 639 in one task, and the two halves change for different reasons. The
//! parent is the *modal* machine — prefix, navigate, the layer a byte lands in
//! — and this is everything about a mouse report, from recognising one to
//! deciding which pane may have it.
//!
//! Nothing here reads a coordinate except [`Input::relay`], and what that does
//! with one is arithmetic rather than interpretation. The `mouse` submodule's
//! header has the two fences and why they differ.

use amx_core::PaneId;
use amx_proto::control::session::{MouseFormat, MouseMode};

use super::{Action, ESC, Input, mouse};
use crate::app::Mode;

pub use mouse::Wheel;

/// One piece of a chrome surface's read, as [`Input::feed_chrome`] splits it.
///
/// There is no variant for a report that is not a wheel turn: a click, a drag
/// and a release are dropped inside the split and never reach a surface at all,
/// which is what "chrome never interprets a mouse report" means in code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chrome {
    /// `bytes[start..end]` belongs to the surface's own key table.
    Keys {
        /// Start of the run.
        start: usize,
        /// One past its end.
        end: usize,
    },
    /// A wheel turn, decoded from the button alone.
    Wheel(Wheel),
}

/// How a carried report candidate ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Carried {
    /// It completed: [`Input::carried`] holds a whole report.
    Report,
    /// A byte broke the pattern; what was held is not a report.
    NotAReport,
}

impl Input {
    /// Record what `pane`'s application asked its terminal to report.
    ///
    /// Fed from every `session.state` fold (`crate::app::events`), which is
    /// where `PaneState.mouse` arrives — the server reads the mode off the
    /// pane's own terminal and folds it into `Core`, so this is the answer and
    /// not a guess (`docs/notes/m4-mouse-path.md` §3).
    ///
    /// Answers whether this changed the pane's answer, so a caller can say
    /// something once per change instead of once per fold.
    pub fn set_mouse_mode(&mut self, pane: PaneId, mode: Option<MouseMode>) -> bool {
        let was = match mode {
            Some(mode) => self.mouse_modes.insert(pane, mode),
            None => self.mouse_modes.remove(&pane),
        };
        was != mode
    }

    /// What `pane` asked for, if anything.
    #[must_use]
    pub fn mouse_mode(&self, pane: PaneId) -> Option<MouseMode> {
        self.mouse_modes.get(&pane).copied()
    }

    /// Whether a report may be relayed to `pane`.
    ///
    /// True only for a pane whose application asked for the **SGR** encoding.
    /// A pane that enabled `?1000` without `?1006` is expecting the X10
    /// encoding, and handing it an SGR report delivers bytes it cannot parse
    /// (`docs/notes/m4-mouse-path.md` F-2) — so the honest answer is no, and
    /// the caller records the drop rather than producing a translation nobody
    /// has measured.
    #[must_use]
    pub fn mouse_enabled(&self, pane: PaneId) -> bool {
        self.mouse_mode(pane)
            .is_some_and(|mode| mode.format == MouseFormat::Sgr)
    }

    /// Rewrite `report` for a pane whose interior is `w`×`h` cells at `x`,`y`
    /// in this terminal, returning the bytes to relay — or `None` when the
    /// report lands outside that interior and is to be dropped.
    ///
    /// The caller has already chosen the pane (by focus, never by position);
    /// this only moves the numbers into that pane's frame.
    pub fn relay(&mut self, report: &[u8], x: u16, y: u16, w: u16, h: u16) -> Option<&[u8]> {
        if mouse::relocate(report, x, y, w, h, &mut self.relay) {
            Some(self.relay.as_slice())
        } else {
            None
        }
    }

    /// [`Self::relay`] for a report that arrived split across two reads, whose
    /// bytes are in [`Self::carried`].
    pub fn relay_carried(&mut self, x: u16, y: u16, w: u16, h: u16) -> Option<&[u8]> {
        // Two disjoint fields of one `&mut self`, which is exactly what the
        // borrow checker allows here and what a `carried()` call followed by a
        // `relay()` call would not be.
        if mouse::relocate(&self.done, x, y, w, h, &mut self.relay) {
            Some(self.relay.as_slice())
        } else {
            None
        }
    }

    /// The bytes behind the latest [`Action::CarriedMouse`] or
    /// [`Action::CarriedBytes`]. Valid until the next [`Input::feed`].
    #[must_use]
    pub fn carried(&self) -> &[u8] {
        &self.done
    }

    /// Take the reusable chrome buffer (empty, capacity kept).
    ///
    /// The `take_scratch` pair for [`Self::feed_chrome`]: a wheel held down
    /// over an open copy mode is a report every few milliseconds, and a `Vec`
    /// per read would allocate on every one of them.
    pub(crate) fn take_chrome(&mut self) -> Vec<Chrome> {
        std::mem::take(&mut self.chrome)
    }

    /// Return the chrome buffer for reuse.
    pub(crate) fn put_chrome(&mut self, mut chrome: Vec<Chrome>) {
        chrome.clear();
        self.chrome = chrome;
    }

    /// Continue an SGR report candidate left over from the previous read.
    /// Returns how many of `bytes` it consumed.
    pub(super) fn resume_carry(
        &mut self,
        mode: Mode,
        bytes: &[u8],
        out: &mut Vec<Action>,
    ) -> usize {
        if self.carry.is_empty() {
            return 0;
        }
        let (taken, end) = self.advance_carry(bytes);
        let action = match end {
            None => return taken,
            Some(Carried::Report) => Action::CarriedMouse(mouse::wheel_of(&self.carry)),
            // Not a report after all: release what was held, and let the byte
            // that broke the pattern keep its normal meaning.
            Some(Carried::NotAReport) => Action::CarriedBytes,
        };
        self.finish_carry();
        if mode == Mode::Terminal {
            out.push(action);
        }
        taken
    }

    /// Feed the carry from `bytes` until it resolves. Returns how many bytes
    /// that took and how it ended, `None` while it is still unresolved.
    fn advance_carry(&mut self, bytes: &[u8]) -> (usize, Option<Carried>) {
        for (at, &b) in bytes.iter().enumerate() {
            if mouse::param_byte(b) && self.carry.len() < mouse::MAX_REPORT {
                self.carry.push(b);
                continue;
            }
            return if b == b'M' || b == b'm' {
                self.carry.push(b);
                (at + 1, Some(Carried::Report))
            } else {
                (at, Some(Carried::NotAReport))
            };
        }
        (bytes.len(), None)
    }

    /// Move a finished carry into [`Self::carried`], where it stays readable
    /// until the next feed.
    fn finish_carry(&mut self) {
        std::mem::swap(&mut self.done, &mut self.carry);
        self.carry.clear();
    }

    /// Split a chrome surface's read into the bytes its key table owns and the
    /// wheel turns it may act on.
    ///
    /// The picker and copy mode read raw bytes, so without this a report's
    /// leading `ESC` cancels the picker and its digits move a copy-mode
    /// cursor. Every report is taken out of the stream here; only a wheel turn
    /// survives it, and only the button of that (D14's fence). The carry is
    /// the same one terminal mode uses, so a report split across two reads is
    /// held in a chrome mode exactly as it is in terminal mode — and a carry
    /// that turns out not to be a report is dropped rather than released,
    /// because nothing but a mouse report begins `ESC [ <`.
    pub fn feed_chrome(&mut self, bytes: &[u8], out: &mut Vec<Chrome>) {
        let mut i = self.resume_carry_chrome(bytes, out);
        let mut run = i;
        while i < bytes.len() {
            if bytes[i] == ESC {
                match mouse::scan(&bytes[i..]) {
                    mouse::Scan::Report(len) => {
                        if run < i {
                            out.push(Chrome::Keys { start: run, end: i });
                        }
                        if let Some(wheel) = mouse::wheel_of(&bytes[i..i + len]) {
                            out.push(Chrome::Wheel(wheel));
                        }
                        i += len;
                        run = i;
                        continue;
                    }
                    mouse::Scan::Partial => {
                        if run < i {
                            out.push(Chrome::Keys { start: run, end: i });
                        }
                        self.carry.extend_from_slice(&bytes[i..]);
                        return;
                    }
                    mouse::Scan::Not => {}
                }
            }
            i += 1;
        }
        if run < bytes.len() {
            out.push(Chrome::Keys {
                start: run,
                end: bytes.len(),
            });
        }
    }

    /// [`Self::resume_carry`] for a chrome surface: a completed report yields
    /// its wheel turn or nothing, and a candidate that was not one is dropped.
    fn resume_carry_chrome(&mut self, bytes: &[u8], out: &mut Vec<Chrome>) -> usize {
        if self.carry.is_empty() {
            return 0;
        }
        let (taken, end) = self.advance_carry(bytes);
        let wheel = match end {
            None => return taken,
            Some(Carried::Report) => mouse::wheel_of(&self.carry),
            Some(Carried::NotAReport) => None,
        };
        self.finish_carry();
        if let Some(wheel) = wheel {
            out.push(Chrome::Wheel(wheel));
        }
        taken
    }
}
