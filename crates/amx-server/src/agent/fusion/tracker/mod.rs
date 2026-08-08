//! The transition function: one pane's status, and how much of it is believed.
//!
//! 04 §5: "The per-pane status tracker is an explicit typed state machine —
//! states and transitions as data, property-tested — not 400 lines of mutable
//! locals (fixes W5's fragility)."
//!
//! The rulebook, in the order [`Tracker::apply`] applies it:
//!
//! 1. **A subagent-scoped edge does nothing at all** — not to the state, not to
//!    the ref, not to a deadline. It is as if it had not arrived, which is what
//!    the property test asserts literally.
//! 2. **Entry edges apply immediately**, including from another held state: a
//!    `PermissionRequest` mid-turn moves `Working` to `Blocked` without waiting
//!    for the screen, because V01 §3 M3 measured that edge arriving 8–14 ms
//!    *before* the dialog paints. What hooks get wrong is silence, never a lie.
//! 3. **A held state is left only through an [`ExitAuthority`]**: three screen
//!    verdicts agreeing against it, one `visible_idle` verdict, the staleness
//!    deadline, or — for a [`CoverageClass::Full`] agent alone — a hook.
//! 4. **Tier 3 never moves a held state and never contradicts an identified
//!    agent.** It cannot spell `blocked` (04 §5) and it has no authority to
//!    end a turn.

use std::time::Duration;

use amx_core::agent::{
    AgentKind, AgentState, CoverageClass, ExitAuthority, SessionRef, StatusCause,
};

mod inputs;

use super::{Deadline, Directive, IDENTITY_GRACE, Input, STALENESS};

/// A screen verdict that contradicts the held state, and how often it has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pending {
    /// What the screen keeps asserting instead.
    state: AgentState,
    /// How many consecutive evaluations have asserted it, capped at
    /// [`CONFIRMATIONS`] — the next one commits.
    seen: u32,
}

/// Which deadlines the hub is holding a timer for on this pane's behalf.
///
/// Mirrored rather than inferred so the machine can answer
/// [`Tracker::is_armed`] without replaying its own directives, and so the property
/// test has something to compare the directive stream *against* — a set derived
/// from the directives and a set the tracker believes in must agree after every
/// single input, or a session of idle agents is paying for wakeups it does not
/// need (03 §5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Armed {
    /// [`Deadline::Confirmation`].
    confirmation: bool,
    /// [`Deadline::Staleness`].
    staleness: bool,
    /// [`Deadline::IdentityGrace`].
    grace: bool,
}

impl Armed {
    /// Whether `deadline` is armed.
    const fn get(self, deadline: Deadline) -> bool {
        match deadline {
            Deadline::Confirmation => self.confirmation,
            Deadline::Staleness => self.staleness,
            Deadline::IdentityGrace => self.grace,
        }
    }

    /// Set `deadline`'s flag, returning what it was.
    const fn set(&mut self, deadline: Deadline, armed: bool) -> bool {
        let slot = match deadline {
            Deadline::Confirmation => &mut self.confirmation,
            Deadline::Staleness => &mut self.staleness,
            Deadline::IdentityGrace => &mut self.grace,
        };
        let was = *slot;
        *slot = armed;
        was
    }
}

/// One pane's status, and how much of it is believed.
///
/// The state it carries is fixed by D-M2-6: the identity, the current
/// [`AgentState`], the provenance of that state (hook-asserted at instant *T*
/// versus screen-confirmed *N* times), and the pending deadlines. The four
/// public fields are what `AgentHub` mirrors into a
/// [`AgentSnapshot`](amx_core::agent::AgentSnapshot); everything else is the
/// machine's own bookkeeping and is read through methods.
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
    /// How long after identification this agent's screen is worth reading.
    ///
    /// Set from the stanza's `startup_grace_ms` by [`identify`](Self::identify);
    /// [`IDENTITY_GRACE`] until then.
    grace: Duration,
    /// The conversation this pane is currently in, as the last hook said.
    session_ref: Option<SessionRef>,
    /// A screen contradiction accumulating against a held state.
    pending: Option<Pending>,
    /// The timers the hub is holding for this pane.
    armed: Armed,
    /// Whether a [`Status`](Directive::Status) has ever been emitted, so the first
    /// one can report `from: None` the way the directive's contract says.
    reported: bool,
    /// Whether the pane's process has ended. Terminal: a retired tracker
    /// publishes its exit and then nothing, ever.
    exited: bool,
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
            grace: IDENTITY_GRACE,
            session_ref: None,
            pending: None,
            armed: Armed {
                confirmation: false,
                staleness: false,
                grace: false,
            },
            reported: false,
            exited: false,
        }
    }

    /// Adopt the registry's answer for this pane's agent, and identify it.
    ///
    /// The one call `AgentHub` makes when tier 3 names a program: the class and
    /// the grace are stanza data the tracker has no way to look up, and they
    /// have to land in the same step as the identity or there is a window in
    /// which a `claude` pane is known to be `claude` and still ignoring its
    /// hooks. [`Input::Identified`] stays reachable on its own for tests that
    /// set the class up themselves.
    pub fn identify(
        &mut self,
        kind: AgentKind,
        coverage: CoverageClass,
        grace: Duration,
    ) -> Vec<Directive> {
        self.coverage = coverage;
        self.grace = grace;
        self.apply(Input::Identified { kind })
    }

    /// Continue a status a predecessor server established, across a handoff.
    ///
    /// R-M3-13: agent status is *carried*, not re-derived. Letting tier 2
    /// rebuild it after a live upgrade would cost a visible
    /// nothing-then-blocked flap on every blocked agent, and would lose the
    /// attention queue's order — which is block time, and is not recoverable
    /// from a screen.
    ///
    /// Two deliberate differences from a tracker that reached the same status
    /// by itself. It has already **reported**: the first transition after the
    /// swap says what it moved *from*, because the client watching it was told
    /// the earlier value by the exporter. And no identity grace is armed: the
    /// grace exists so a booting TUI's splash is not read as evidence, and an
    /// inherited agent booted on somebody else's clock long ago.
    ///
    /// Returns the directives the hub owes the wheel — a held state still needs
    /// its staleness deadline, or a block that ends during the swap would be
    /// held forever.
    pub fn adopt(
        &mut self,
        carried: &amx_core::agent::AgentSnapshot,
        coverage: CoverageClass,
        grace: Duration,
    ) -> Vec<Directive> {
        self.kind = carried.kind.clone();
        self.coverage = coverage;
        self.grace = grace;
        self.state = carried.state;
        self.cause = carried.cause;
        self.session_ref = carried.session_ref.clone();
        self.reported = true;
        let mut directives = Vec::new();
        if self.state.is_held() {
            self.arm(Deadline::Staleness, STALENESS, &mut directives);
        }
        directives
    }

    /// The conversation this pane can be resumed into, as the last hook said.
    #[must_use]
    pub const fn session_ref(&self) -> Option<&SessionRef> {
        self.session_ref.as_ref()
    }

    /// The state a screen contradiction is accumulating towards, if one is.
    #[must_use]
    pub const fn pending(&self) -> Option<AgentState> {
        match self.pending {
            Some(pending) => Some(pending.state),
            None => None,
        }
    }

    /// Whether the hub should be holding a timer for `deadline` right now.
    #[must_use]
    pub const fn is_armed(&self, deadline: Deadline) -> bool {
        self.armed.get(deadline)
    }

    /// Whether the pane's process has ended.
    #[must_use]
    pub const fn is_exited(&self) -> bool {
        self.exited
    }

    /// Apply one input; return what the hub should do about it.
    ///
    /// The invariants the property tests pin, each a sentence from 04 §5 or
    /// D-M2-6 turned into something a machine can check:
    ///
    /// - a subagent-scoped edge changes nothing, for any state, any class, any
    ///   interleaving — the rule that keeps V01's anonymous `SubagentStop`,
    ///   arriving two seconds after the parent's `Stop` on every tool turn,
    ///   from reviving an idle pane;
    /// - a held state is only left through an [`ExitAuthority`], so an `edges`
    ///   agent's `Working` can only end at the screen or at the deadline;
    /// - at most one [`Directive::Status`] per input;
    /// - every armed deadline is eventually disarmed or fired — no input
    ///   sequence leaves one pending against a state that has moved on.
    pub fn apply(&mut self, input: Input) -> Vec<Directive> {
        let mut directives = Vec::new();
        if self.exited {
            return directives;
        }
        match input {
            Input::Hook(edge) => self.on_hook(&edge, &mut directives),
            Input::Screen(verdict) => self.on_screen(&verdict, &mut directives),
            Input::Identified { kind } => self.on_identified(kind, &mut directives),
            Input::Probe(activity) => self.on_probe(activity, &mut directives),
            Input::Deadline(deadline) => self.on_deadline(deadline, &mut directives),
            Input::Exited => self.on_exited(&mut directives),
        }
        directives
    }
    /// An entry edge asserts `state`.
    pub(super) fn enter(
        &mut self,
        state: AgentState,
        cause: StatusCause,
        directives: &mut Vec<Directive>,
    ) {
        if state == self.state {
            // Re-asserted, not moved. The pane is corroborated: whatever
            // contradiction the screen was accumulating is stale, and the
            // staleness clock starts again.
            self.clear_pending(directives);
            if state.is_held() {
                self.arm(Deadline::Staleness, STALENESS, directives);
            }
            return;
        }
        self.transition(state, cause, directives);
    }

    /// A hook says the held state is over — [`CoverageClass::Full`] only, since
    /// nothing else can build the authority.
    pub(super) fn leave(&mut self, authority: ExitAuthority, directives: &mut Vec<Directive>) {
        if !self.state.is_held() {
            return;
        }
        self.transition(AgentState::Idle, authority.cause(), directives);
    }

    /// Move the pane, and emit the one status directive that says so.
    ///
    /// The fixed directive order lives here: the status, then the queue, then the
    /// timers.
    pub(super) fn transition(
        &mut self,
        to: AgentState,
        cause: StatusCause,
        directives: &mut Vec<Directive>,
    ) {
        let from = self.state;
        self.state = to;
        self.cause = cause;
        directives.push(Directive::Status {
            from: self.reported.then_some(from),
            to,
            cause,
        });
        self.reported = true;
        if from.wants_attention() {
            directives.push(Directive::Dequeue);
        }
        if to.wants_attention() {
            directives.push(Directive::Enqueue);
        }
        self.clear_pending(directives);
        if to.is_held() {
            self.arm(Deadline::Staleness, STALENESS, directives);
        } else {
            self.disarm(Deadline::Staleness, directives);
        }
    }

    /// Drop an accumulating contradiction, and the cap that bounded it.
    pub(super) fn clear_pending(&mut self, directives: &mut Vec<Directive>) {
        if self.pending.take().is_some() {
            self.disarm(Deadline::Confirmation, directives);
        }
    }

    /// Ask the hub for a timer, replacing any it holds of the same kind.
    pub(super) fn arm(
        &mut self,
        deadline: Deadline,
        after: Duration,
        directives: &mut Vec<Directive>,
    ) {
        self.armed.set(deadline, true);
        directives.push(Directive::Arm { deadline, after });
    }

    /// Release a timer, if one is held.
    pub(super) fn disarm(&mut self, deadline: Deadline, directives: &mut Vec<Directive>) {
        if self.armed.set(deadline, false) {
            directives.push(Directive::Disarm { deadline });
        }
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}
