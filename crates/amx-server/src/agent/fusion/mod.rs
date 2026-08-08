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
/// a `permission_prompt` notification each push it out again. A pane the screen
/// keeps confirming as `Working` is never cleared for staleness; a pane nothing
/// has said anything about for 30 s always is.
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
