//! When a delta will not do.
//!
//! 04 §4: "When accumulated damage crosses a threshold (or on reconnect/resync),
//! the server sends a **keyframe** (`grid.reset` + full visible grid). Keyframes
//! exist in protocol v1 so overflow recovery is skew-compatible forever."
//!
//! There are exactly six ways a delta stops being applicable, and naming them
//! is most of the design:
//!
//! - the client has no grid yet ([`KeyframeReason::First`]);
//! - the pane resized, so the rects were computed against a geometry the client
//!   does not have ([`KeyframeReason::Resize`]);
//! - the grid generation moved, which is the same statement in protocol terms
//!   and is what a reconnect with a stale `Resume` produces
//!   ([`KeyframeReason::Generation`]);
//! - the client resumed and its cells cannot be certified
//!   ([`KeyframeReason::Resumed`]);
//! - the client asked, because it knows its copy is untrustworthy
//!   ([`KeyframeReason::Resync`]);
//! - or the accumulated set has grown to most of the grid
//!   ([`KeyframeReason::Threshold`]).
//!
//! The threshold is the only one that is a judgement rather than a fact, and it
//! is not primarily about bytes: a delta covering 90% of the rows carries very
//! nearly the same cells as a keyframe. It is about *doubt*. Damage spread that
//! wide means the client has been stalled through a lot of history — a scroll
//! storm, a full-screen application redrawing, a flood — and a keyframe is the
//! one message that does not require its previous state to be right. Paying a
//! few percent more bytes to stop depending on a stalled client's memory is the
//! trade 04 §4 asks for.

/// Why a grid stream is sending a keyframe instead of a delta.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyframeReason {
    /// The client has not been sent a grid yet.
    First,
    /// The visible grid changed shape.
    Resize,
    /// The pane's grid generation is not the one the client last saw.
    Generation,
    /// The client re-bound a pane it holds cells for, and the server cannot
    /// certify them.
    ///
    /// W08 opened such a bind **delta-only**, on the reading that a matching
    /// generation means the client's grid is current. It does not, and the
    /// module documentation of `conn/resume.rs` said so in the same breath: the
    /// generation moves on resize and reset only, so agreement means the
    /// *geometry* still matches. Between a client's last applied delta and its
    /// re-bind the pane can paint as much as it likes, and a delta-only open
    /// then leaves that client looking at a screen that is wrong forever — no
    /// error, no repaint, no way for anybody to notice.
    ///
    /// M3's exit suite found it exactly where it is most reachable: a live
    /// upgrade resumes every pane at the commit, and a client's reconnect lands
    /// *after* whatever the pane painted in the meantime.
    ///
    /// So a resumed grid bind repaints, which is what 04 §6 asks for in as many
    /// words ("keyframes for stale grids") — every grid is stale unless
    /// something proves otherwise, and nothing on this wire can. What *would*
    /// prove it is named in `docs/notes/m3-wave-outcomes.md` under W14: the
    /// missed `pane_damage` events a resuming client is already replayed say
    /// exactly which panes moved, so a client that drained its event replay
    /// before binding could vouch for the rest. That is a client change, and
    /// this is the server refusing to guess in the meantime.
    Resumed,
    /// The client sent [`FlowControl::Resync`](amx_proto::stream::FlowControl::Resync).
    Resync,
    /// Accumulated damage crossed [`KeyframePolicy::damage_percent`].
    Threshold,
}

impl KeyframeReason {
    /// A short tag, for logs and test assertions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Resize => "resize",
            Self::Generation => "generation",
            Self::Resumed => "resumed",
            Self::Resync => "resync",
            Self::Threshold => "threshold",
        }
    }
}

/// When accumulated damage is wide enough to be worth a keyframe.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyframePolicy {
    /// Percentage of the visible rows that, once dirty, forces a keyframe.
    pub damage_percent: u8,
}

impl KeyframePolicy {
    /// The default share of the grid that forces a keyframe.
    ///
    /// Three quarters: below it a delta is meaningfully smaller than a full
    /// grid, and above it the saving no longer pays for depending on the
    /// client's previous state.
    pub const DEFAULT_DAMAGE_PERCENT: u8 = 75;

    /// A policy with a custom threshold, clamped to a real percentage.
    #[must_use]
    pub const fn new(damage_percent: u8) -> Self {
        Self {
            damage_percent: if damage_percent > 100 {
                100
            } else {
                damage_percent
            },
        }
    }

    /// Whether `marked` of `rows` dirty rows crosses the threshold.
    ///
    /// A grid with no rows never crosses: there is nothing to send either way.
    #[must_use]
    pub const fn crossed(self, marked: u16, rows: u16) -> bool {
        if rows == 0 || marked == 0 {
            return false;
        }
        marked as u32 * 100 >= rows as u32 * self.damage_percent as u32
    }
}

impl Default for KeyframePolicy {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DAMAGE_PERCENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_threshold_is_three_quarters_of_the_grid() {
        let policy = KeyframePolicy::default();
        assert!(!policy.crossed(17, 24), "17 of 24 rows is under 75%");
        assert!(policy.crossed(18, 24), "18 of 24 rows is 75%");
        assert!(policy.crossed(24, 24));
    }

    #[test]
    fn an_empty_set_never_crosses() {
        assert!(!KeyframePolicy::new(0).crossed(0, 24));
        assert!(!KeyframePolicy::default().crossed(0, 0));
    }

    #[test]
    fn the_percentage_is_clamped() {
        assert_eq!(KeyframePolicy::new(200).damage_percent, 100);
        assert!(!KeyframePolicy::new(200).crossed(23, 24));
        assert!(KeyframePolicy::new(200).crossed(24, 24));
    }
}
