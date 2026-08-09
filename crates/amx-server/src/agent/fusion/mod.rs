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
//! returns [`Directive`]s. No clocks, no I/O, no tokio — deadlines *arrive* as
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
//!    hook exits, and
//!    [`ExitAuthority::from_hook`](amx_core::agent::ExitAuthority::from_hook)
//!    refuses to hand it one.
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
//! # The three files
//!
//! | Module | What lives there |
//! |---|---|
//! | this one | the constants, the input and directive vocabulary, the timers |
//! | [`edge`] | one hook report reduced to an edge, and the precedence table |
//! | [`tracker`] | the transition function, which is the machine itself |
//!
//! The split is by responsibility, not by size: [`precedence`] is a pure
//! function of the measurement and is read as a table, while [`Tracker::apply`]
//! is the rulebook that decides what a table row is *allowed* to do to a pane
//! that is already holding a state.
//!
//! # Task ownership
//!
//! V02 froze the vocabulary, the constants and the shape of the transition
//! function. **V04** fills [`Tracker::apply`] and [`precedence`], and its
//! property tests are as much the deliverable as the machine: arbitrary
//! interleavings must never revive an idle pane from a subagent event, never
//! wedge a tracker with an unfired deadline, and never emit two status
//! directives for one transition.

pub mod edge;
pub mod tracker;

use std::time::Duration;

use amx_core::agent::{Activity, AgentKind, AgentState, SessionRef, StatusCause};

pub use edge::{EdgeEffect, HookEdge, precedence};
pub use tracker::Tracker;

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
///
/// The machine does not enforce it — it has no clock. `AgentHub`'s per-pane
/// coalescing of `PaneDamage` does, and this is the number it coalesces to; it
/// lives here because it is one half of the confirmation window's arithmetic
/// (three verdicts at 100 ms fit inside [`CONFIRMATION_CAP`], which is what
/// makes the cap a backstop rather than the usual path).
pub const CONFIRMATION_SPACING: Duration = Duration::from_millis(100);

/// The longest a confirmation hold may last before the verdict is taken anyway.
///
/// It bounds the interrupt case — which V01 established is the *common* case,
/// not an exotic one, since every user-initiated exit is silent.
///
/// Armed once, when a contradiction first appears, and *not* re-armed by the
/// verdicts that follow it: the cap bounds the whole hold, not the gap between
/// two evaluations. A pane whose damage stops arriving mid-hold — which is
/// itself evidence the screen has settled — leaves the held state here.
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
///
/// Measured from the last thing that agreed with the state, not from the
/// transition into it: a hook re-assertion, a screen verdict that concurs, and
/// a `permission_prompt` notification each push it out again.
///
/// # What it is a timeout on
///
/// **Evidence, not the state**, and the difference is the whole of the M4 exit's
/// first defect. Until then the fire demoted a held state to `Idle` the moment
/// it arrived, on the reading that thirty seconds with nothing corroborating a
/// block is thirty seconds of a block that is over. But tier 2 is evaluated on
/// *damage*, and a permission dialog waiting for a human produces none: the
/// screen that proves the block sits there unchanged, and therefore silent. So
/// the deadline was reading "no new evidence" as "no evidence", and
/// `docs/notes/m4-live-smoke.md` §4.8 and §6.8 measured what that costs — a real
/// Claude Code session blocked on a real dialog was called `idle` 35.4 s later,
/// `amx agents` reported five idle agents, the board showed no flag, and
/// `amx agent next` answered `waiting: 0` with the dialog plainly on the screen.
///
/// The fire therefore asks tier 2 before it takes anything away, and the answer
/// decides:
///
/// | What tier 2 can see, at the fire | What the deadline does |
/// |---|---|
/// | a state — the held one or another | the state stands, and the clock starts again: a pane whose screen is being read leaves a held state through the screen, under the confirmation window, and not here |
/// | nothing: [`Verdict::NoMatch`](super::manifest::Verdict::NoMatch) or a `skip_state_update` hold | the state stands, and the clock starts again. A detector that is looking and has no opinion has not said the block ended |
/// | nothing at all, because no manifest is bound | **the exit is taken.** This is 04 §5's clause (c), and the pane it was written for |
///
/// That last row is the one that has to keep working: V01's edge case 13 is a
/// Codex approval the user denied, which emits nothing further for as long as
/// anyone watched, so a pane with no screen coverage has no other way out and
/// would hold `Blocked` for the life of the session. The rows above it are
/// panes somebody *is* watching, and for those 04 §5 already puts the exit on
/// tier 2 — clause (b), not clause (c). The narrowing is between the clauses of
/// one rule, not away from it.
///
/// # Why holding is the right way to be wrong
///
/// The rule is deliberately asymmetric, because the two failures are not
/// comparable. A block amx still reports after it has ended is a wrong row a
/// person sees, jumps to, and clears in one keystroke — and the first repaint
/// clears it without them. A block amx has *forgotten* is a phone that says
/// nobody is waiting. D14 and D15 build four surfaces whose entire subject is
/// who needs a human, and a surface that under-reports is worse than one that
/// over-reports, because it is trusted and it is silent.
///
/// What it costs: a pane blocked all night now re-arms every 30 s instead of
/// demoting once, which is one wakeup and one evaluation of a frame already in
/// memory per 30 s per *blocked* pane. 03 §5's promise is about idle agents and
/// they still cost nothing — an unheld state arms nothing at all.
///
/// What it leaves standing: an agent whose idle screen no rule matches will hold
/// its last state until something repaints into a rule that does. Both shipped
/// manifests match their own prompt box — on Claude Code that is
/// `prompt_box_idle`, the most carefully guarded rule in the file — so the gap
/// is a manifest that has fallen behind its agent's UI, and it is visible in
/// `agent explain` rather than silent in the queue.
pub const STALENESS: Duration = Duration::from_secs(30);

/// The default grace before a freshly spawned pane's screen is believed.
///
/// A booting TUI's splash matches nonsense. Per-agent, because V01 §6 measured
/// the two shipped agents differing: Claude Code is ready about 1.1 s after
/// launch, Codex took 2.8–4.6 s to finish its startup gates — which is why
/// [`AgentStanza::startup_grace_ms`](super::registry::AgentStanza::startup_grace_ms)
/// exists and this is only the fallback.
pub const IDENTITY_GRACE: Duration = Duration::from_secs(3);

/// One tier-2 evaluation's answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScreenVerdict {
    /// The state the winning rule asserts, or `None` for a `skip_state_update`
    /// rule — herdr's assert-nothing third outcome, which freezes the previous
    /// state rather than contradicting it.
    ///
    /// `None` also carries [`Verdict::NoMatch`](super::manifest::Verdict::NoMatch),
    /// and the two mean the same thing to the machine: tier 2 has no opinion
    /// about this screen, which is not the same as an opinion that the pane is
    /// idle. Neither confirms a held state nor contradicts it, so neither
    /// touches the confirmation count.
    pub asserts: Option<AgentState>,
    /// The rule that won, for `agent.explain` and for the provenance the
    /// directive carries.
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
///
/// A deadline the tracker did not ask for is ignored on arrival: the hub owns
/// the wheel, so a fire that races a [`Directive::Disarm`] is the hub's normal
/// operation and not a state change.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Deadline {
    /// [`CONFIRMATION_CAP`] elapsed with a screen verdict pending.
    Confirmation,
    /// [`STALENESS`] elapsed on a held state nothing has corroborated.
    ///
    /// The one deadline whose arm consults something other than the tracker's
    /// own bookkeeping: it may not take a held state away from a screen tier 2
    /// is reading, so the hub evaluates the pane's current frame on the way to
    /// firing this and the machine decides from what came back. [`STALENESS`]
    /// has the table.
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
/// Named `Directive` and not `Effect`, which is what it was called until DR-10:
/// three unrelated enums of that name shadowed each other — the render
/// dirtiness [`amx_core::Effect`] (04 §2's "every message handler returns an
/// `Effect` value"), `amx_vt::callbacks`' parser-thread one, and this. The
/// dirtiness type is the one 04 names, so it keeps the name; this one is
/// renamed for what it is, an instruction the machine hands the hub. (The vt
/// shadow is X09's, split by file ownership.)
///
/// Data, like the inputs. `AgentHub` turns [`Status`](Self::Status) into a
/// `StatusView` write plus a published event *in that order*, and the queue
/// directives into the attention queue — but none of that ordering is this
/// module's business, which is why it can be property-tested without one.
///
/// One input's directives come out in a fixed order — the status, then the
/// queue, then the timers — so a test may compare a whole list rather than
/// searching it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Directive {
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
