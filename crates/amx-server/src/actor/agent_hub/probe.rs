//! What one run of the hub did, and a window onto a run in progress.
//!
//! Counted at the seam and *returned*, not logged, for the reason
//! [`PersistReport`](crate::actor::persist::PersistReport) is: "did that report
//! actually land?" and "did a cancelled hub keep working?" are the questions
//! the suite asks, and a status line is not where either gets answered.
//!
//! Split from [`super`] because it is bookkeeping about the actor rather than
//! any part of what the actor does — nothing here reads a pane, a tracker or a
//! manifest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What one run of the hub did.
///
/// Returned rather than logged, for the reason [`PersistReport`] is: "did that
/// report actually land?" and "did a cancelled hub keep working?" are the
/// questions the suite asks, and counting at the seam is how they get answered
/// without reaching into a status line for the answer.
///
/// [`PersistReport`]: crate::actor::persist::PersistReport
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AgentReport {
    /// Hook reports accepted and applied.
    pub reports: u64,
    /// Hook reports dropped: a token that did not match the pane's, or a
    /// source claiming an agent it was not installed for.
    pub dropped: u64,
    /// Tier-2 screen evaluations run.
    pub evaluations: u64,
    /// Times the deadline wheel fired.
    pub wakeups: u64,
    /// Status transitions published.
    pub transitions: u64,
    /// Panes still tracked when the hub stopped.
    pub tracked: usize,
}

/// A live window onto a running hub's counters.
///
/// Cloneable and lock-free, the shape [`PaneProbe`](crate::actor::PaneProbe)
/// uses: a test watches the hub work without having to stop it first, which is
/// the only way to assert that a *cancelled* hub stopped working.
#[derive(Clone, Debug, Default)]
pub struct AgentProbe(Arc<Counters>);

#[derive(Debug, Default)]
struct Counters {
    reports: AtomicU64,
    dropped: AtomicU64,
    evaluations: AtomicU64,
    wakeups: AtomicU64,
    transitions: AtomicU64,
}

impl AgentProbe {
    /// A fresh set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hook reports accepted and applied.
    #[must_use]
    pub fn reports(&self) -> u64 {
        self.0.reports.load(Ordering::Relaxed)
    }

    /// Hook reports dropped for a bad token or a foreign source.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.0.dropped.load(Ordering::Relaxed)
    }

    /// Tier-2 screen evaluations run.
    #[must_use]
    pub fn evaluations(&self) -> u64 {
        self.0.evaluations.load(Ordering::Relaxed)
    }

    /// Times the deadline wheel fired.
    ///
    /// The number `idle_session_arms_no_timer` is about: with nothing armed the
    /// timer branch of the select is disabled, so this stays at zero however
    /// long the session sits there.
    #[must_use]
    pub fn wakeups(&self) -> u64 {
        self.0.wakeups.load(Ordering::Relaxed)
    }

    /// Status transitions published.
    #[must_use]
    pub fn transitions(&self) -> u64 {
        self.0.transitions.load(Ordering::Relaxed)
    }

    /// Count one accepted hook report.
    pub(super) fn counted_report(&self) {
        bump(&self.0.reports);
    }

    /// Count one report dropped for a bad token or a foreign source.
    pub(super) fn counted_drop(&self) {
        bump(&self.0.dropped);
    }

    /// Count one tier-2 evaluation.
    pub(super) fn counted_evaluation(&self) {
        bump(&self.0.evaluations);
    }

    /// Count one firing of the deadline wheel.
    pub(super) fn counted_wakeup(&self) {
        bump(&self.0.wakeups);
    }

    /// Count one published status transition.
    pub(super) fn counted_transition(&self) {
        bump(&self.0.transitions);
    }
}

/// Add one, unordered: these are read for assertions and for a report, never
/// to synchronise anything.
fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}
