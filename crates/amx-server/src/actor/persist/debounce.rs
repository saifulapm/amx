//! When a save is due, and what makes one due.
//!
//! Split out of [`super::actor`] by W03 (`docs/09-m3-plan.md` R-M3-7) before
//! M3 grew the loop it was sitting in. The two responsibilities part cleanly:
//! *when* to write is a pair of deadlines plus a reading of the bus, and it is
//! all here; *how* to write — capture, dump, encode, fsync, drain — stays with
//! the actor that owns the state those steps read.
//!
//! Both halves of the debounce are here because they are one decision. herdr's
//! trailing-only window (`SESSION_SAVE_DEBOUNCE`) never fires at all while a
//! session keeps changing; the cap is what makes staleness bounded instead of
//! merely usually-small (D-M1-7).

use std::time::Duration;

use amx_core::Event;
use tokio::time::Instant;

/// How long the session must be quiet before a debounced save fires.
///
/// The common path: structural change comes in bursts (a split, a focus move,
/// a layout settle) and half a second of quiet means the burst is over.
pub const QUIET_WINDOW: Duration = Duration::from_millis(500);

/// How long after the *first* unsaved change a save fires regardless.
///
/// The ceiling the quiet window alone does not have. A session that keeps
/// changing forever still saves every five seconds, which is what bounds the
/// loss a `kill -9` can cost (D-M1-7). Not shrunk casually: darwin's
/// `F_FULLFSYNC` is drastically slower than a Linux `fsync` (R-M1-6).
pub const STALENESS_CAP: Duration = Duration::from_secs(5);

/// When the pending change is due to be written.
///
/// Both deadlines are absolute and set at most once per dirty span: `quiet`
/// moves out with every new change, `cap` never does. The save fires at
/// whichever arrives first.
#[derive(Clone, Copy, Debug)]
struct Due {
    quiet: Instant,
    cap: Instant,
}

/// The pending-save deadline, armed or not.
///
/// A value rather than two fields on the actor, so "is anything unsaved, and
/// when is it owed?" is one question asked of one thing.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Debounce {
    due: Option<Due>,
}

impl Debounce {
    /// The deadline the timer should be armed to, or `None` when nothing is
    /// unsaved.
    pub(super) fn at(self) -> Option<Instant> {
        self.due.map(|due| due.quiet.min(due.cap))
    }

    /// Arm the debounce, or move its quiet deadline out if it is already armed.
    pub(super) fn mark(&mut self) {
        let now = Instant::now();
        match &mut self.due {
            Some(due) => due.quiet = now + QUIET_WINDOW,
            None => {
                self.due = Some(Due {
                    quiet: now + QUIET_WINDOW,
                    cap: now + STALENESS_CAP,
                });
            }
        }
    }

    /// Forget the pending deadline.
    ///
    /// Called before a save runs and never re-armed on failure: a save that
    /// fails is not retried on a timer (herdr's `+250 ms` re-arm is what
    /// backpressure replaces here), it waits for the next change.
    pub(super) fn disarm(&mut self) {
        self.due = None;
    }
}

/// What one event means for what belongs on disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Change {
    /// The session's shape moved: capture and write.
    Structure,
    /// A pane's history window moved, which only an opted-in sidecar cares
    /// about.
    History,
    /// Nothing durable changed.
    Nothing,
}

/// Classify one event.
///
/// Damage, titles, clients coming and going — and whatever a later version of
/// the enum adds — are [`Change::Nothing`]: none of it is the session's shape,
/// and folding per-frame damage in would put the quiet window out of reach of
/// any pane producing output (D-M1-7).
///
/// Scrollback is the session's *other* durable half, and it moves on its own
/// schedule: a session settles into a shape and then spends the rest of the day
/// producing history. A dump that only rode structural dirt would therefore
/// never happen in a real session — the pane that scrolls is usually the pane
/// nothing is being done to — so history gets its own answer rather than being
/// filed under noise.
pub(super) const fn change_in(event: &Event) -> Change {
    match event {
        Event::PaneCreated { .. }
        | Event::PaneExited { .. }
        | Event::PaneRenamed { .. }
        | Event::PaneResized { .. }
        | Event::WorkspaceCreated { .. }
        | Event::WorkspaceRenamed { .. }
        | Event::WorkspaceClosed { .. }
        | Event::FocusChanged { .. }
        | Event::LayoutChanged { .. } => Change::Structure,
        Event::HistoryCommitted { .. }
        | Event::HistoryEvicted { .. }
        | Event::HistoryInvalidated { .. } => Change::History,
        _ => Change::Nothing,
    }
}
