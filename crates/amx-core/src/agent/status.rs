//! What a pane's agent is doing, and how much of that its hooks can be
//! believed about.
//!
//! Four types and one predicate table, all of them a reading of 04 §5 and of
//! the measurement in `docs/notes/hook-coverage.md`:
//!
//! - [`AgentState`] — what a pane's foreground program is doing.
//! - [`Activity`] — tier 3's *whole* vocabulary, which is what keeps 04 §5's
//!   "never fake `blocked`" a property of the types rather than of care.
//! - [`CoverageClass`] — how much of an agent's lifecycle its hooks report.
//! - [`StatusCause`] — what moved a pane into the state it is in.
//! - [`ExitAuthority`] — the three, and only three, ways to leave a held state.
//!
//! Split from [`super`] to keep both files inside the module budget; the two
//! halves are "who the agent is" there and "what it is doing" here.

use serde::{Deserialize, Serialize};

/// How much of an agent's lifecycle its hook system actually reports.
///
/// 04 §5's table, verbatim:
///
/// | Class | Meaning |
/// |---|---|
/// | `full` | hook system covers the complete lifecycle; hooks may own state outright |
/// | `edges` | hooks see turn/tool/permission edges but miss interrupts/cancels → fusion |
/// | `identity` | hooks carry session identity only → screen detection owns state |
/// | `none` | tier-3 heuristics only |
///
/// and the sentence that governs how a class is chosen: "classes are measured,
/// not aspirational". A stanza's class is copied out of
/// `docs/notes/hook-coverage.md`, and V03's conformance test reads that
/// document as its fixture so the two cannot drift.
///
/// As of V01 both shipped agents are [`Edges`](Self::Edges). Nothing measured
/// has ever put an agent in [`Full`](Self::Full) — 04 §5's `full` examples come
/// from herdr's production data, not from an experiment (V01 §9, L12) — so the
/// variant exists to keep the fusion machine honest about *why* it trusts an
/// exit, not because anything ships in it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageClass {
    /// Hooks cover the complete lifecycle and may own state outright.
    Full,
    /// Hooks see turn, tool and permission edges but miss interrupts and
    /// cancels — the fusion case, and the one both shipped agents are in.
    Edges,
    /// Hooks carry session identity only; screen detection owns state.
    Identity,
    /// No usable hooks at all: tier-3 heuristics, `busy`/`quiet`, nothing else.
    None,
}

impl CoverageClass {
    /// Whether a hook edge may put a pane *into* `Working` or `Blocked` on its
    /// own authority, with no screen corroboration.
    ///
    /// 04 §5: "Hooks assert **entry edges** with high confidence and zero
    /// latency … These apply immediately." V01 §3 M3 measured the strongest
    /// case: `PermissionRequest` starts its hook process 8–14 ms *before* the
    /// permission dialog paints, so a `Blocked` taken from that edge is never
    /// late relative to the screen.
    #[must_use]
    pub const fn hook_entries_apply(self) -> bool {
        matches!(self, Self::Full | Self::Edges)
    }

    /// Whether a hook edge may take a pane *out of* `Working` or `Blocked`.
    ///
    /// True for [`Full`](Self::Full) alone. For every other class 04 §5's
    /// "**Exits from working/blocked are confirmed**, not trusted" applies, and
    /// V01 measured why for the two agents amx ships: on Claude Code 2.1.224
    /// and Codex 0.147.0, *every* user-initiated exit — Esc during generation,
    /// Esc during a tool call, a permission dialog answered "No", a dialog
    /// cancelled with Esc — emits nothing at all.
    ///
    /// A `Stop` hook does still arrive when a turn ends *by itself*; that is an
    /// exit edge and this predicate refuses it anyway, because a machine that
    /// took the happy-path exit from a hook and the interrupted exit from the
    /// screen would have two code paths for one transition and would be tested
    /// on the one that fires in a demo. One exit path, screen-owned, always.
    #[must_use]
    pub const fn hook_exits_apply(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Whether leaving a held state is the screen's job (with the staleness
    /// deadline as the backstop).
    ///
    /// The inverse of [`hook_exits_apply`](Self::hook_exits_apply), spelled out
    /// because it is the sentence the fusion machine is written against and the
    /// one the M2 exit test replays: for an `edges` agent, **exit is
    /// screen-owned**.
    #[must_use]
    pub const fn exit_is_screen_owned(self) -> bool {
        !self.hook_exits_apply()
    }

    /// Whether this class needs tier-2 screen detection running at all.
    ///
    /// Everything but [`Full`](Self::Full): an `edges` agent needs it to leave
    /// held states, an `identity` agent needs it for the whole of its state,
    /// and a `none` agent has nothing but tier 3 — for which `false` here would
    /// be wrong, since a manifest may still match an unidentified program's
    /// prompt.
    #[must_use]
    pub const fn needs_screen(self) -> bool {
        !matches!(self, Self::Full)
    }

    /// This class's wire and registry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Edges => "edges",
            Self::Identity => "identity",
            Self::None => "none",
        }
    }
}

impl std::fmt::Display for CoverageClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a pane's foreground program is doing, as far as amx can tell.
///
/// The first three are 04 §5's agent statuses and are only ever asserted for a
/// pane whose agent has been *identified*. The last two are tier 3's whole
/// vocabulary for an unknown foreground program: "Process-tree foreground job +
/// prompt detection for unknown programs: `busy/quiet`, **never fake
/// `blocked`**."
///
/// That rule is the reason [`Activity`] exists as a separate type: tier 3
/// produces an `Activity`, an `Activity` converts *into* an `AgentState`, and
/// there is no `Activity` value that becomes [`Blocked`](Self::Blocked). A
/// probe cannot fake a block because it cannot spell one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// The agent is at its prompt, waiting for the user.
    Idle,
    /// The agent is working on a turn.
    Working,
    /// The agent is waiting for the user to answer something — a permission
    /// dialog, a confirmation. This is what the attention queue is ordered by.
    Blocked,
    /// An unidentified foreground program is producing output.
    Busy,
    /// An unidentified foreground program is not producing output.
    Quiet,
}

impl AgentState {
    /// Whether this state belongs to an identified agent.
    ///
    /// `false` for [`Busy`](Self::Busy) and [`Quiet`](Self::Quiet), which
    /// belong to tier 3 and to nothing else.
    #[must_use]
    pub const fn is_agent(self) -> bool {
        matches!(self, Self::Idle | Self::Working | Self::Blocked)
    }

    /// Whether a pane in this state is on the attention queue.
    ///
    /// Exactly [`Blocked`](Self::Blocked) — the queue is "agents entering
    /// `blocked`", 04 §5 — so the status line's `⚑N`, `agent.next` and the
    /// enqueue notification all derive from one predicate rather than three
    /// matches that could disagree.
    #[must_use]
    pub const fn wants_attention(self) -> bool {
        matches!(self, Self::Blocked)
    }

    /// Whether this state must be *left* through a confirmed exit rather than
    /// simply overwritten.
    ///
    /// The `working`/`blocked` of 04 §5's "Exits from working/blocked are
    /// confirmed, not trusted". A pane holding one of these needs an
    /// [`ExitAuthority`] to move.
    #[must_use]
    pub const fn is_held(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }

    /// This state's wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Busy => "busy",
            Self::Quiet => "quiet",
        }
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tier 3's whole vocabulary for a pane whose program it does not recognise.
///
/// 04 §5: "Tier 3 — heuristics. Process-tree foreground job + prompt detection
/// for unknown programs: `busy/quiet`, never fake `blocked`." The type is what
/// enforces the second half of that sentence — see [`AgentState`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    /// Output is arriving.
    Busy,
    /// Nothing is arriving.
    Quiet,
}

impl From<Activity> for AgentState {
    fn from(activity: Activity) -> Self {
        match activity {
            Activity::Busy => Self::Busy,
            Activity::Quiet => Self::Quiet,
        }
    }
}

/// What moved a pane into the state it is in.
///
/// Rides `agent_status` so `amx events --json`, the status line and `agent
/// explain` can all say *why*, which is the difference between a detection you
/// can fix and one you can only distrust.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCause {
    /// A hook edge asserted it. Entry edges only, unless the agent's class is
    /// [`CoverageClass::Full`] — see [`ExitAuthority`].
    Hook,
    /// Tier-2 screen detection asserted it, having held the verdict for the
    /// confirmation window (or matched a rule flagged `visible_idle`, which
    /// bypasses the hold).
    Screen,
    /// The staleness deadline expired on a hook-asserted state with no screen
    /// coverage to contradict it. 04 §5's third exit.
    Staleness,
    /// Tier-3 identity probing produced it: an [`Activity`], or the first state
    /// a freshly identified agent takes.
    Probe,
    /// The pane's process ended. Terminal: a retired tracker publishes this and
    /// then nothing.
    Exited,
}

/// Permission to leave a held [`AgentState`].
///
/// 04 §5, normatively: "**Exits from working/blocked are confirmed**, not
/// trusted: a hook-asserted state is cleared by (a) a matching hook event,
/// (b) tier-2 screen detection contradicting it, or (c) a bounded staleness
/// timeout — whichever comes first."
///
/// This type is (a), (b) and (c) as the only three ways to spell an exit, with
/// (a) gated on the agent's coverage class. There is no other constructor, so
/// a fusion arm that wanted to leave `Working` on a hook edge for an `edges`
/// agent cannot be written by accident — [`from_hook`](Self::from_hook) hands
/// it a `None` and the arm has to say what it does about it.
///
/// ```
/// use amx_core::agent::{CoverageClass, ExitAuthority, StatusCause};
///
/// // Measured, V01: every user-initiated exit on an `edges` agent is silent,
/// // so a hook may not be believed about one.
/// assert!(ExitAuthority::from_hook(CoverageClass::Edges).is_none());
/// assert!(ExitAuthority::from_hook(CoverageClass::Full).is_some());
///
/// // The two that always exist. They are what an Esc-interrupt lands through.
/// assert_eq!(ExitAuthority::from_screen().cause(), StatusCause::Screen);
/// assert_eq!(ExitAuthority::from_staleness().cause(), StatusCause::Staleness);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExitAuthority(StatusCause);

impl ExitAuthority {
    /// Exit (a): a matching hook event — for a [`CoverageClass::Full`] agent
    /// and no other.
    ///
    /// `None` is the measured answer for everything amx ships. V01 §1: on both
    /// Claude Code and Codex "every exit-by-user is silent", so a class that
    /// let a hook close a turn would be correct exactly on the turns nobody
    /// interrupted.
    #[must_use]
    pub const fn from_hook(class: CoverageClass) -> Option<Self> {
        if class.hook_exits_apply() {
            Some(Self(StatusCause::Hook))
        } else {
            None
        }
    }

    /// Exit (b): tier-2 screen detection has contradicted the held state for
    /// the confirmation window.
    ///
    /// Always available, for every class including [`CoverageClass::None`] — a
    /// manifest may match a program amx never identified.
    #[must_use]
    pub const fn from_screen() -> Self {
        Self(StatusCause::Screen)
    }

    /// Exit (c): the staleness deadline expired.
    ///
    /// The backstop for a pane with no screen coverage at all, and new ground —
    /// herdr holds a detected state forever until contradicted (R-M2-11). It is
    /// also the only thing that ends V01's edge case 13, a Codex approval the
    /// user denied: nothing at all arrives afterwards, so a tracker waiting for
    /// `Stop` waits forever.
    #[must_use]
    pub const fn from_staleness() -> Self {
        Self(StatusCause::Staleness)
    }

    /// Which of the three this is, for the `agent_status` event.
    #[must_use]
    pub const fn cause(self) -> StatusCause {
        self.0
    }
}
