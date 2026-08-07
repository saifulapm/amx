//! The fusion state machine: hook edges and screen verdicts into one status.
//!
//! 04 §5 answers herdr's "two competing sources of truth" objection by saying
//! which source wins where:
//!
//! > - Hooks assert **entry edges** with high confidence and zero latency …
//! >   These apply immediately.
//! > - **Exits from working/blocked are confirmed**, not trusted: a
//! >   hook-asserted state is cleared by (a) a matching hook event, (b) tier-2
//! >   screen detection contradicting it, or (c) a bounded staleness timeout —
//! >   whichever comes first. User interrupts and dialog cancels, invisible to
//! >   hooks, are caught by (b)/(c).
//! > - Subagent-scoped events never override the parent turn's state.
//!
//! Everything here is data in, data out: a [`Tracker`] takes an [`Input`] and
//! returns [`Effect`]s. No clocks, no I/O, no tokio — deadlines *arrive* as
//! inputs, which is what lets V04 property-test arbitrary interleavings
//! exhaustively rather than at whatever speed a wall clock happened to run.
//!
//! # What V01 measured, and what it deletes
//!
//! `docs/notes/hook-coverage.md` settles the four gates the plan made V04 wait
//! on (M1, M2, M4, M6). Three findings shape the tables below:
//!
//! 1. **Every exit-by-user is silent**, on both agents. Esc during generation,
//!    Esc during a tool call, a permission dialog answered "No", and a dialog
//!    cancelled with Esc all emit nothing at all. So an `edges` agent has no
//!    hook exits, and [`ExitAuthority::from_hook`] refuses to hand it one.
//! 2. **`agent_id` is the subagent discriminator, and the hazard is the common
//!    case.** An anonymous `SubagentStop` arrives 1.9–3.0 s *after* the
//!    parent's `Stop` on essentially every tool-using turn. A machine that read
//!    it as a parent edge would churn a pane's status after every tool call.
//! 3. **`PermissionDenied` is dead data.** D-M2-6 provisionally had
//!    `PermissionDenied → Working`; the event never fired once, on any deny
//!    path V01 could produce — a human "No", an Esc, or a `permissions.deny`
//!    rule. There is deliberately no arm for it here, and
//!    [`HookEvent`](amx_proto::control::agent::HookEvent) has no variant for
//!    it. An arm no input reaches is an arm no test covers.
//!
//! # Task ownership
//!
//! V02 froze the vocabulary, the constants and the shape of the transition
//! function. **V04** fills [`Tracker::apply`] and [`precedence`], and its
//! property tests are as much the deliverable as the machine: arbitrary
//! interleavings must never revive an idle pane from a subagent event, never
//! wedge a tracker with an unfired deadline, and never emit two status effects
//! for one transition.

use std::time::Duration;

use amx_core::agent::{
    Activity, AgentKind, AgentState, CoverageClass, ExitAuthority, SessionRef, StatusCause,
};
use amx_proto::control::agent::{HookEvent, ReportParams};

/// How many consecutive screen verdicts confirm an exit from a held state.
///
/// Three, inherited from herdr's production value and **confirmed** by V01 §6
/// rather than carried forward on faith: nothing measured argues against it,
/// and the interrupt screen (`Interrupted · What should Claude do instead?`) is
/// stable text, so three evaluations is about 300 ms of certainty.
pub const CONFIRMATIONS: u32 = 3;

/// The minimum spacing between two screen evaluations of one pane.
///
/// V01 §6 keeps 100 ms and gives the reason in a ratio: hook dispatch has a
/// median of 26 ms, so 100 ms is about 4× it and a hook edge always wins a race
/// it should win.
pub const CONFIRMATION_SPACING: Duration = Duration::from_millis(100);

/// The longest a confirmation hold may last before the verdict is taken anyway.
///
/// It bounds the interrupt case — which V01 established is the *common* case,
/// not an exotic one, since every user-initiated exit is silent.
pub const CONFIRMATION_CAP: Duration = Duration::from_millis(700);

/// How long a hook-asserted state survives with nothing corroborating it.
///
/// 04 §5's third exit, and new ground: herdr has no staleness expiry at all —
/// state persists until contradicted — so R-M2-11 flags that sizing this wrong
/// shows up as flapping (too short) or as herdr's stuck-status bug (too long).
///
/// V01 §6 keeps 30 s and notes what now stands beside it: `Notification`
/// corroborates a block at 6 s and an idle at 60 s, including 60 s after a
/// silent interrupt. Those are second witnesses, not replacements — 30 s still
/// has to stand alone for a pane with no screen coverage, and Codex emits no
/// `Notification` at all.
///
/// It is also the only thing that ends V01's edge case 13: a Codex approval the
/// user denied produces nothing further, ever, so a tracker waiting for `Stop`
/// to leave `Blocked` waits forever.
pub const STALENESS: Duration = Duration::from_secs(30);

/// The default grace before a freshly spawned pane's screen is believed.
///
/// A booting TUI's splash matches nonsense. Per-agent, because V01 §6 measured
/// the two shipped agents differing: Claude Code is ready about 1.1 s after
/// launch, Codex took 2.8–4.6 s to finish its startup gates — which is why
/// [`AgentStanza::startup_grace_ms`](super::registry::AgentStanza::startup_grace_ms)
/// exists and this is only the fallback.
pub const IDENTITY_GRACE: Duration = Duration::from_secs(3);

/// What a hook report reduces to once the machine has read it.
///
/// The reduction is the point: `amx _hook` forwards everything it is installed
/// for, tagged, and *this* is where policy lives (D-M2-4). herdr baked the
/// filtering into installed scripts, so changing policy meant reinstalling
/// hooks on every machine; changing it here is shipping a binary.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HookEdge {
    /// The event, as the agent named it.
    pub event: HookEvent,
    /// Whether it was scoped to a subagent.
    ///
    /// The single most load-bearing field in the module. `true` means the
    /// report carried an `agent_id`, which V01 §3 M4 measured as present on
    /// every subagent-scoped event and absent from every parent one — 44
    /// `Stop`s without, 8 `SubagentStop`s with.
    pub subagent: bool,
    /// The tool a `PreToolUse`/`PermissionRequest` names, when it names one.
    pub tool: Option<String>,
    /// `Notification`'s type: `permission_prompt` or `idle_prompt`.
    pub notification: Option<String>,
    /// The conversation this report identifies, when it carries one.
    ///
    /// Taken from **every** `SessionStart`, not just the first: V01 §3 M8
    /// measured `/clear` minting a new session id inside one process, and the
    /// `source` field is the only warning that the previous ref is now stale.
    pub session_ref: Option<SessionRef>,
}

impl HookEdge {
    /// Reduce one report to an edge.
    ///
    /// **V04 fills this.** Building the [`SessionRef`] needs the stanza's
    /// `ref_kind`, so the caller supplies the kind rather than this guessing.
    #[must_use]
    pub fn from_report(report: &ReportParams) -> Self {
        let _ = report;
        todo!("V04: reduce a report to an edge, tagging subagent scope from its ReportScope")
    }
}

/// What one hook edge is allowed to do to a tracker, given its coverage class.
///
/// The precedence table of 04 §5 and D-M2-6, as a value. Three outcomes and no
/// fourth: an edge either enters a state now, asks to leave the held one, or
/// changes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeEffect {
    /// Apply this state immediately. Entry edges only, and only for a class
    /// whose [`hook_entries_apply`](CoverageClass::hook_entries_apply).
    Enter(AgentState),
    /// This edge says the held state is over.
    ///
    /// Carries the [`ExitAuthority`] that makes it legal, which for every class
    /// but [`CoverageClass::Full`] means this variant is unreachable — the
    /// authority cannot be built. That is the type doing the work 04 §5's
    /// prose does.
    Leave(ExitAuthority),
    /// Nothing: corroboration, a subagent's business, or an event this build
    /// has never heard of.
    Ignore,
}

/// What `edge` may do to a pane running an agent of class `class`.
///
/// **V04 fills this**, from the measured table. The shape it must produce, with
/// the recording behind each row:
///
/// | Edge | `edges` class | Why |
/// |---|---|---|
/// | `UserPromptSubmit` | `Enter(Working)` | Fires on both agents, median 26 ms after the keystroke |
/// | `PreToolUse` | `Enter(Working)` | Claude Code always; on Codex *conditional* (a plain file read produced only `PostToolUse`), so it corroborates there rather than driving |
/// | `PostToolUse` | `Enter(Working)` | The turn continues after a tool returns |
/// | `PermissionRequest` | `Enter(Blocked)` | Starts 8–14 ms *before* the dialog paints, so the entry is never late relative to the screen |
/// | `Stop` | `Ignore` | An exit, and exits are screen-owned — see the module docs |
/// | `SessionStart` | `Ignore` for state; carries the ref | Codex fires it only at the first prompt, so it says nothing about *doing* |
/// | `Notification` | `Ignore` | A 6 s/60 s backstop, far too slow to drive a status line |
/// | anything `subagent` | `Ignore` | Never touches the parent — not on entry, not on exit |
///
/// For [`CoverageClass::Identity`] and [`CoverageClass::None`] every row is
/// `Ignore`: those classes' state is the screen's, entirely.
#[must_use]
pub fn precedence(class: CoverageClass, edge: &HookEdge) -> EdgeEffect {
    let _ = (class, edge);
    todo!("V04: the measured precedence table above")
}

/// One tier-2 evaluation's answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScreenVerdict {
    /// The state the winning rule asserts, or `None` for a `skip_state_update`
    /// rule — herdr's assert-nothing third outcome, which freezes the previous
    /// state rather than contradicting it.
    pub asserts: Option<AgentState>,
    /// The rule that won, for `agent.explain` and for the effect's provenance.
    pub rule: Option<String>,
    /// Whether the winning rule is flagged `visible_idle`.
    ///
    /// The prompt box is actually on screen, so the confirmation hold is
    /// bypassed. herdr's flicker fix, kept as data rather than as a special
    /// case in the machine.
    pub visible_idle: bool,
}

/// A timer the tracker asked for, firing.
///
/// Deadlines are *inputs*, never something this module measures. That is what
/// makes V04's property tests exhaustive — a machine that read a clock could
/// only be tested at the speed the clock ran — and it is why the deadline wheel
/// lives in `AgentHub` (V08), which is allowed to own a timer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Deadline {
    /// [`CONFIRMATION_CAP`] elapsed with a screen verdict pending.
    Confirmation,
    /// [`STALENESS`] elapsed on a held state nothing has corroborated.
    Staleness,
    /// The identity grace elapsed; the screen may be believed now.
    IdentityGrace,
}

/// Everything that can reach a [`Tracker`].
///
/// Four variants, exactly the four D-M2-6 names, so the property tests can
/// enumerate arbitrary interleavings by enumerating this type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Input {
    /// A hook report arrived.
    Hook(HookEdge),
    /// A tier-2 evaluation produced a verdict.
    Screen(ScreenVerdict),
    /// Tier 3 identified the pane's foreground program.
    Identified {
        /// Which agent, and therefore which coverage class and manifest.
        kind: AgentKind,
    },
    /// Tier 3 has an opinion about an *unidentified* program.
    ///
    /// [`Activity`] and not [`AgentState`]: tier 3 cannot spell `blocked`, so
    /// 04 §5's "never fake `blocked`" holds because there is no way to say it.
    Probe(Activity),
    /// A deadline fired.
    Deadline(Deadline),
    /// The pane's process ended. Terminal.
    Exited,
}

/// What the tracker asks the hub to do about a transition.
///
/// Data, like the inputs. `AgentHub` turns [`Status`](Self::Status) into a
/// `StatusView` write plus a published event *in that order*, and the queue
/// effects into the attention queue — but none of that ordering is this
/// module's business, which is why it can be property-tested without one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// The pane moved. **At most one of these per input** — V04's
    /// `every_transition_emits_exactly_one_status_effect` is the property.
    Status {
        /// What it was, or `None` for a pane's first status.
        from: Option<AgentState>,
        /// What it is now.
        to: AgentState,
        /// Which of the three exits, or which tier, moved it.
        cause: StatusCause,
    },
    /// The pane's agent was named.
    Identified {
        /// Which agent.
        kind: AgentKind,
    },
    /// A conversation ref was learned or replaced.
    ///
    /// Replaced matters: `/clear` mints a new session id inside one process, so
    /// a ref captured before it is stale and restoring it would resume the
    /// wrong conversation.
    Ref {
        /// The ref now current for this pane.
        session_ref: SessionRef,
    },
    /// Join the attention queue, at the tail.
    Enqueue,
    /// Leave the attention queue.
    Dequeue,
    /// Arm a deadline, replacing any pending one of the same kind.
    Arm {
        /// Which deadline.
        deadline: Deadline,
        /// How far out.
        after: Duration,
    },
    /// Disarm a deadline that is no longer needed.
    ///
    /// A tracker that armed a deadline and then moved on without disarming it
    /// costs the session a wakeup it did not need — and 03 §5's promise is that
    /// a session of idle agents costs *zero*. V04's
    /// `no_input_sequence_leaves_a_tracker_with_a_stale_deadline` is the
    /// property.
    Disarm {
        /// Which deadline.
        deadline: Deadline,
    },
}

/// One pane's status, and how much of it is believed.
///
/// 04 §5: "The per-pane status tracker is an explicit typed state machine —
/// states and transitions as data, property-tested — not 400 lines of mutable
/// locals (fixes W5's fragility)."
///
/// **V04 fills the fields and [`apply`](Self::apply).** The state it has to
/// carry is fixed by D-M2-6: the identity, the current [`AgentState`], the
/// provenance of that state (hook-asserted at instant *T* versus
/// screen-confirmed *N* times), and the pending deadlines.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tracker {
    /// The agent identified in this pane, once one has been.
    pub kind: Option<AgentKind>,
    /// Its coverage class — [`CoverageClass::None`] until identity lands, which
    /// is what keeps an unidentified pane on tier 3 alone.
    pub coverage: CoverageClass,
    /// What the pane is doing.
    pub state: AgentState,
    /// What put it there.
    pub cause: StatusCause,
}

impl Tracker {
    /// A tracker for a pane nothing is known about yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            kind: None,
            coverage: CoverageClass::None,
            state: AgentState::Quiet,
            cause: StatusCause::Probe,
        }
    }

    /// Apply one input; return what the hub should do about it.
    ///
    /// **V04 fills this.** The invariants its property tests pin, each a
    /// sentence from 04 §5 or D-M2-6 turned into something a machine can check:
    ///
    /// - a subagent-scoped edge changes nothing, for any state, any class, any
    ///   interleaving — the rule that keeps V01's anonymous `SubagentStop`,
    ///   arriving two seconds after the parent's `Stop` on every tool turn,
    ///   from reviving an idle pane;
    /// - a held state is only left through an [`ExitAuthority`], so an `edges`
    ///   agent's `Working` can only end at the screen or at the deadline;
    /// - at most one [`Effect::Status`] per input;
    /// - every armed deadline is eventually disarmed or fired — no input
    ///   sequence leaves one pending against a state that has moved on.
    pub fn apply(&mut self, input: Input) -> Vec<Effect> {
        let _ = input;
        todo!("V04: the transition function, per the tables above")
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}
