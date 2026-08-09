//! Tier 2, scheduled: what a damage batch costs and what it concludes.
//!
//! 03 §5's promise is push, never poll, and D-M2-3 says what pushes:
//!
//! > evaluation gated on content change — for amx that is the `PaneDamage`
//! > event stream plus per-pane coalescing in `AgentHub`, not a 300 ms timer.
//!
//! The coalescing is the whole of the cost control. A pane streaming an
//! answer produces damage at frame rate; a pane at its prompt produces none at
//! all. So an evaluation runs immediately when the pane has been quiet, and
//! otherwise waits out the remainder of [`CONFIRMATION_SPACING`] — one owed
//! evaluation, not a queue of them, because an evaluation reads the *current*
//! frame and a hundred coalesced batches and one are the same work.
//!
//! The spacing is 100 ms and the reason is a ratio (V01 §6): hook dispatch has
//! a median of 26 ms, so the screen never wins a race against a hook edge that
//! the hook edge should win. It is also one third of the confirmation window,
//! which is what makes three verdicts fit inside
//! [`CONFIRMATION_CAP`](crate::agent::fusion::CONFIRMATION_CAP) with room to
//! spare.
//!
//! What an evaluation reads is the published snapshot — the live visible grid.
//! herdr had to anchor its detection buffer to the scrollback bottom and
//! regression-test that a scrolled viewport never moved it; amx's server never
//! holds a scrolled view at all (04 §3), so "detection never reads the scrolled
//! viewport" is true by construction here.

use amx_core::PaneId;
use amx_vt::Snapshot;
use tokio::time::Instant;

use super::AgentHub;
use crate::agent::fusion::{CONFIRMATION_SPACING, Input, ScreenVerdict};
use crate::agent::manifest::{ScreenBuf, Verdict};

/// How many rows of the visible grid an evaluation reads.
///
/// The whole grid, in practice: every shipped region is either the title or a
/// bottom slice of the screen, and a terminal taller than this is not a thing
/// (`u16::MAX` rows would be sixty thousand lines of visible pane). It exists
/// as a named bound rather than a `latest().grid()` walk so the cost of one
/// evaluation is a number somebody chose.
const READ_ROWS: u16 = 512;

impl AgentHub {
    /// A damage batch arrived for `pane`: evaluate now, or owe an evaluation.
    ///
    /// The floor is measured from the last evaluation that *ran*, so a burst of
    /// damage cannot push the next one further and further out — the same rule
    /// [`ProbeGate`](crate::agent::identity::ProbeGate) keeps for identity
    /// probes, for the same reason.
    pub(super) fn schedule_detection(&mut self, pane: PaneId, now: Instant) {
        let Some(tracked) = self.panes.get_mut(&pane) else {
            return;
        };
        match tracked.last_eval {
            Some(last) if now.saturating_duration_since(last) < CONFIRMATION_SPACING => {
                // One slot: an evaluation already owed is not owed twice.
                tracked.eval_due.get_or_insert(last + CONFIRMATION_SPACING);
            }
            _ => self.evaluate(pane, now),
        }
    }

    /// Run one tier-2 evaluation of `pane` against its current frame.
    pub(super) fn evaluate(&mut self, pane: PaneId, now: Instant) {
        let Some(tracked) = self.panes.get_mut(&pane) else {
            return;
        };
        tracked.last_eval = Some(now);
        tracked.eval_due = None;
        let Some(manifest) = tracked.manifest.clone() else {
            // No manifest, no tier 2: an unidentified pane, or an agent whose
            // stanza names none. Its state stays tier 3's and tier 1's.
            return;
        };
        let frame = tracked.frames.latest();
        fill(&mut tracked.screen, Some(&frame));
        let verdict = match manifest.evaluate(tracked.screen.screen(&tracked.title)) {
            Verdict::Assert { rule, state } => ScreenVerdict {
                asserts: Some(state),
                rule: Some(rule.name().to_owned()),
                visible_idle: rule.visible_idle(),
            },
            // `skip_state_update`: the rule matched and asserts nothing, which
            // freezes the previous state rather than contradicting it. Named,
            // because "which rule froze this" is the question `agent explain`
            // exists to answer.
            Verdict::Hold { rule } => ScreenVerdict {
                asserts: None,
                rule: Some(rule.name().to_owned()),
                visible_idle: false,
            },
            Verdict::NoMatch => ScreenVerdict {
                asserts: None,
                rule: None,
                visible_idle: false,
            },
        };
        self.probe.counted_evaluation();
        let directives = self.apply(pane, Input::Screen(verdict));
        self.absorb(pane, &directives);
    }

    /// Ask tier 2 about `pane` because its staleness deadline is firing.
    ///
    /// The one evaluation in this module that no damage asked for, and the
    /// reason it is worth an exception: the deadline is about to decide whether
    /// a held state survives, and the machine decides that from what tier 2 can
    /// see (`fusion::STALENESS`). Every other evaluation is scheduled by a
    /// screen *change*, which is exactly the thing a pane waiting on a human
    /// stops producing — so without this, the answer the deadline consults
    /// could be older than the block it is about, or absent entirely for a pane
    /// whose manifest was bound after its last damage.
    ///
    /// It costs a manifest pass over a frame already in memory, into a
    /// `ScreenBuf` this pane already owns: no I/O, no allocation, and only for
    /// a pane actually holding a state.
    pub(super) fn corroborate(&mut self, pane: PaneId, now: Instant) {
        let held = self
            .panes
            .get(&pane)
            .is_some_and(|tracked| tracked.tracker.state.is_held());
        if held {
            self.evaluate(pane, now);
        }
    }

    /// Evaluate every tracked pane that owes one at `now`.
    pub(super) fn evaluate_due(&mut self, now: Instant) {
        let due: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, tracked)| tracked.eval_due.is_some_and(|due| due <= now))
            .map(|(pane, _)| *pane)
            .collect();
        for pane in due {
            self.evaluate(pane, now);
        }
    }

    /// Owe every tracked pane an evaluation.
    ///
    /// What a bus [`Gap`](amx_core::Delivery::Gap) means here: a gap can only
    /// hide transitions, and damage is the transition this actor cares about,
    /// so the honest reaction is to look at every screen again. Cheap, because
    /// looking is reading a frame that is already published.
    pub(super) fn schedule_all(&mut self, now: Instant) {
        let panes: Vec<PaneId> = self.panes.keys().copied().collect();
        for pane in panes {
            self.schedule_detection(pane, now);
        }
    }
}

/// Refill `screen` from `frame`'s visible rows.
///
/// The one place a snapshot becomes rule text, so a fixture on disk and a live
/// pane are normalised identically — [`ScreenBuf::fill`] trims each row's
/// padding and the blank tail, which is what makes a `$`-anchored rule and a
/// `bottom_lines(3)` region mean what they say.
pub(super) fn fill(screen: &mut ScreenBuf, frame: Option<&Snapshot>) {
    match frame {
        Some(frame) => screen.fill(frame.tail(READ_ROWS).map(amx_vt::Row::line)),
        None => screen.fill(std::iter::empty()),
    }
}
