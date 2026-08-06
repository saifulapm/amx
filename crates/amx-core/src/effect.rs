//! Render dirtiness as a value, not a flag (D2).
//!
//! 04 §2: "every message handler returns an `Effect` value (`Nothing <
//! PaneDamage(id) < Layout < Full`); the loop folds effects and schedules
//! output. There are no mutable `needs_render` flags to keep in sync —
//! forgetting is a type error, not a stale frame."

use crate::id::PaneId;

/// How much of the output a change invalidates.
///
/// Ordered `Nothing < PaneDamage < Layout < Full`, which is the ordering the
/// fold uses: absorbing two effects keeps the stronger one. The order is the
/// declaration order, and `Ord` is derived from it — moving a variant changes
/// the semantics, which is why the variants carry no explicit discriminants to
/// suggest otherwise.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Level {
    /// Nothing needs redrawing.
    #[default]
    Nothing,
    /// Cells inside one or more panes changed.
    PaneDamage,
    /// The layout tree changed shape: chrome and pane rects must be recomputed.
    Layout,
    /// Everything must be redrawn from scratch.
    Full,
}

/// What one handler changed.
///
/// Returned by every message handler in the server. A handler that changes
/// state and returns [`Effect::Nothing`] is a bug, but it is a *visible* bug at
/// the call site rather than a missing flag write 94 sites away.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Effect {
    /// No output change.
    #[default]
    Nothing,
    /// Cells inside this pane changed.
    PaneDamage(PaneId),
    /// The layout tree changed shape.
    Layout,
    /// Everything must be redrawn.
    Full,
}

impl Effect {
    /// The ordering level of this effect.
    #[must_use]
    pub const fn level(self) -> Level {
        match self {
            Self::Nothing => Level::Nothing,
            Self::PaneDamage(_) => Level::PaneDamage,
            Self::Layout => Level::Layout,
            Self::Full => Level::Full,
        }
    }
}

/// The fold of every effect produced in one batch.
///
/// The accumulator is reused across batches: [`drain_into`](Self::drain_into)
/// clears its buffers but never frees them, so folding a batch does not
/// allocate after warm-up (the CLAUDE.md hot-path rule — event dispatch is on
/// that list).
#[derive(Clone, Debug, Default)]
pub struct EffectSet {
    level: Level,
    panes: Vec<PaneId>,
}

impl EffectSet {
    /// An empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An accumulator that can absorb `panes` distinct panes without allocating.
    #[must_use]
    pub fn with_capacity(panes: usize) -> Self {
        Self {
            level: Level::Nothing,
            panes: Vec::with_capacity(panes),
        }
    }

    /// Fold one effect in.
    ///
    /// The level is the maximum of what has been absorbed. The pane list
    /// accumulates the panes named by [`Effect::PaneDamage`] and is only
    /// meaningful while the level *is* [`Level::PaneDamage`] — at `Layout` or
    /// `Full` the whole output is being rebuilt anyway.
    pub fn absorb(&mut self, effect: Effect) {
        if let Effect::PaneDamage(pane) = effect
            && !self.panes.contains(&pane)
        {
            self.panes.push(pane);
        }
        self.level = self.level.max(effect.level());
    }

    /// The strongest level absorbed so far.
    #[must_use]
    pub fn level(&self) -> Level {
        self.level
    }

    /// The panes named by absorbed [`Effect::PaneDamage`] effects, deduplicated.
    #[must_use]
    pub fn panes(&self) -> &[PaneId] {
        &self.panes
    }

    /// Whether nothing has been absorbed since the last drain.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.level == Level::Nothing
    }

    /// Move the fold into `out` and reset for the next batch.
    ///
    /// `out` is cleared first: both buffers keep their capacity, so a steady
    /// state of "fold a batch, schedule output, repeat" performs no allocation.
    pub fn drain_into(&mut self, out: &mut Scheduled) {
        out.clear();
        out.level = self.level;
        out.panes.extend_from_slice(&self.panes);
        self.level = Level::Nothing;
        self.panes.clear();
    }
}

/// One batch's worth of folded effects, handed to the output scheduler.
///
/// Same buffer discipline as [`EffectSet`]: cleared and refilled, never
/// reallocated per frame.
#[derive(Clone, Debug, Default)]
pub struct Scheduled {
    level: Level,
    panes: Vec<PaneId>,
}

impl Scheduled {
    /// An empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The strongest level in the batch.
    #[must_use]
    pub fn level(&self) -> Level {
        self.level
    }

    /// The damaged panes, meaningful only at [`Level::PaneDamage`].
    #[must_use]
    pub fn panes(&self) -> &[PaneId] {
        &self.panes
    }

    /// Whether this batch scheduled no output at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.level == Level::Nothing
    }

    /// Reset to empty, keeping the pane buffer's capacity.
    pub fn clear(&mut self) {
        self.level = Level::Nothing;
        self.panes.clear();
    }
}
