//! One arm each: what the tracker does with a hook edge, a screen verdict, an
//! identification, a probe, a deadline and an exit.
//!
//! Split out of [`super`] by X02 before X06 grew it further
//! (`docs/11-m4-plan.md` R-M4-5); the code is V04's, moved and not changed. The
//! line it splits on is the rulebook's own: [`super`] holds the state machine's
//! primitives — enter, leave, transition, arm, disarm — and this holds the six
//! places that decide *whether* to reach for one. A reader asking "what does a
//! `SubagentStop` do" comes here; a reader asking "what does leaving a held
//! state cost" stays there.

use amx_core::agent::{Activity, AgentKind, AgentState, ExitAuthority, StatusCause};

use super::super::edge::{EdgeEffect, HookEdge, PERMISSION_PROMPT, precedence};
use super::super::{
    CONFIRMATION_CAP, CONFIRMATIONS, Deadline, Directive, STALENESS, ScreenVerdict,
};
use super::{Pending, Sight, Tracker};

impl Tracker {
    /// A hook report reached this pane.
    pub(super) fn on_hook(&mut self, edge: &HookEdge, directives: &mut Vec<Directive>) {
        if edge.subagent {
            return;
        }
        if let Some(session_ref) = &edge.session_ref
            && self.session_ref.as_ref() != Some(session_ref)
        {
            self.session_ref = Some(session_ref.clone());
            directives.push(Directive::Ref {
                session_ref: session_ref.clone(),
            });
        }
        // The hook's own name for itself, which is what D-M4-3 puts on the
        // wire: `PermissionRequest` and not a `permission` this module would
        // have had to invent and then maintain per agent.
        let named = || Some(edge.event.as_str().to_owned());
        match precedence(self.coverage, edge) {
            EdgeEffect::Enter(state) => self.enter(state, StatusCause::Hook, named(), directives),
            EdgeEffect::Leave(authority) => self.leave(authority, named(), directives),
            // The one thing an ignored edge may still do: agree with what the
            // pane already believes. V01 §3 M5 measured a `permission_prompt`
            // notification 6.0 s into an unanswered dialog, which is a free
            // second witness that a held `Blocked` is real — so it pushes the
            // staleness deadline out rather than letting a genuine block be
            // cleared at 30 s.
            //
            // Its sibling `idle_prompt` deliberately does *not* end a held
            // `Working`, even though V01 calls it "a free contradiction": at
            // 60 s it is far too slow to be an edge, the screen will have
            // settled the question forty times over, and taking an exit from a
            // hook is exactly what `ExitAuthority::from_hook` refuses an
            // `edges` agent.
            EdgeEffect::Ignore => {
                if self.state == AgentState::Blocked && edge.is_notification(PERMISSION_PROMPT) {
                    self.arm(Deadline::Staleness, STALENESS, directives);
                }
            }
        }
    }

    /// A tier-2 evaluation produced a verdict.
    pub(super) fn on_screen(&mut self, verdict: &ScreenVerdict, directives: &mut Vec<Directive>) {
        // A booting TUI's splash matches nonsense: until the grace elapses the
        // screen is not evidence about anything.
        if self.armed.grace {
            return;
        }
        // Somebody is reading this pane's screen, and *that* is a fact the
        // staleness exit needs separately from anything the verdict asserts:
        // its whole question is whether this pane has a witness other than the
        // clock. Recorded before the early return below, because a tier 2 with
        // no opinion is still a tier 2 that is looking.
        self.sight = verdict.asserts.map_or(Sight::Silent, Sight::Asserted);
        // `skip_state_update` and "nothing matched" alike: tier 2 has no
        // opinion, which neither confirms a held state nor contradicts one.
        let Some(asserts) = verdict.asserts else {
            return;
        };
        if asserts == self.state {
            self.clear_pending(directives);
            if self.state.is_held() {
                self.arm(Deadline::Staleness, STALENESS, directives);
            }
            return;
        }
        // Nothing is being *left*: an unheld state is overwritten on sight, and
        // a `visible_idle` rule means the prompt box is actually painted, which
        // is herdr's flicker fix kept as data.
        if !self.state.is_held() || verdict.visible_idle {
            self.transition(
                asserts,
                StatusCause::Screen,
                verdict.rule.clone(),
                directives,
            );
            return;
        }
        let restart = !matches!(&self.pending, Some(pending) if pending.state == asserts);
        let seen = if restart {
            1
        } else {
            self.pending.as_ref().map_or(1, |pending| pending.seen + 1)
        };
        if seen >= CONFIRMATIONS {
            self.transition(
                asserts,
                ExitAuthority::from_screen().cause(),
                verdict.rule.clone(),
                directives,
            );
            return;
        }
        self.pending = Some(Pending {
            state: asserts,
            // The latest verdict's rule, not the first hold's: three
            // consecutive verdicts agreeing on a state need not have come from
            // one rule, and the honest name for the transition is the rule that
            // was still asserting it when it committed.
            rule: verdict.rule.clone(),
            seen,
        });
        // Armed once, at the head of the hold: the cap bounds the whole
        // confirmation window, not the gap between two evaluations.
        if restart {
            self.arm(Deadline::Confirmation, CONFIRMATION_CAP, directives);
        }
    }

    /// Tier 3 named the pane's foreground program.
    pub(super) fn on_identified(&mut self, kind: AgentKind, directives: &mut Vec<Directive>) {
        if self.kind.as_ref() == Some(&kind) {
            return;
        }
        self.kind = Some(kind.clone());
        // A different agent is a different manifest, so whatever tier 2 last
        // saw here it saw with somebody else's rules. The pane has not been
        // read by the rules that will decide its next verdict, and the
        // staleness exit is entitled to know that.
        self.sight = Sight::Blind;
        directives.push(Directive::Identified { kind });
        if !self.grace.is_zero() {
            self.arm(Deadline::IdentityGrace, self.grace, directives);
        }
        // A pane whose program has a name is no longer an unknown one, so it
        // may not keep tier 3's vocabulary. The activity it was showing is the
        // best first reading there is: output arriving is a turn in progress,
        // silence is a prompt waiting. Both are corrected within a confirmation
        // window by the first hook edge or screen verdict that disagrees.
        //
        // No reason rides with it: tier 3 has no detector to name. It saw
        // output or silence on a pty, which is the whole of what it knows, and
        // `cause: probe` already says so in the closed vocabulary.
        match self.state {
            AgentState::Busy => {
                self.transition(AgentState::Working, StatusCause::Probe, None, directives)
            }
            AgentState::Quiet => {
                self.transition(AgentState::Idle, StatusCause::Probe, None, directives)
            }
            _ => {}
        }
    }

    /// Tier 3 has an opinion about an unidentified program.
    pub(super) fn on_probe(&mut self, activity: Activity, directives: &mut Vec<Directive>) {
        // An identified agent's state belongs to its hooks and its manifest,
        // and a held state belongs to whatever holds an `ExitAuthority` — a
        // probe has none. Either way tier 3 is not consulted, which is 04 §5's
        // "`busy/quiet`, never fake `blocked`" arriving at the same place from
        // the other direction.
        if self.kind.is_some() || self.state.is_held() {
            return;
        }
        let state = AgentState::from(activity);
        if state != self.state {
            self.transition(state, StatusCause::Probe, None, directives);
        }
    }

    /// A deadline fired.
    pub(super) fn on_deadline(&mut self, deadline: Deadline, directives: &mut Vec<Directive>) {
        // The hub owns the wheel, so a fire that raced a `Disarm` is ordinary
        // operation. A deadline nobody asked for asserts nothing.
        if !self.armed.set(deadline, false) {
            return;
        }
        match deadline {
            // The screen may be believed from here on; nothing else changes.
            Deadline::IdentityGrace => {}
            // The screen's answer, taken at the cap instead of at the third
            // verdict, so it is named by the rule that was asserting it.
            Deadline::Confirmation => {
                if let Some(pending) = self.pending.take() {
                    self.transition(
                        pending.state,
                        ExitAuthority::from_screen().cause(),
                        pending.rule,
                        directives,
                    );
                }
            }
            // 04 §5's third exit, and the only arm here that consults something
            // the tracker did not work out for itself. `STALENESS` carries the
            // table and the reasoning; the rule is that this exit belongs to a
            // pane *nobody can see*, and `Sight` is what says whether this is
            // one.
            Deadline::Staleness => {
                if !self.state.is_held() {
                    return;
                }
                if self.sight != Sight::Blind {
                    // Tier 2 is reading this pane, so tier 2 owns its exits —
                    // 04 §5's clause (b), which is where an `edges` agent's
                    // exits live anyway. The state stands and the clock starts
                    // again. No status directive, because nothing moved: a
                    // fold of timers alone publishes nothing and writes
                    // nothing, so a dialog waiting all night costs one wakeup
                    // every 30 s and not one event.
                    self.arm(Deadline::Staleness, STALENESS, directives);
                    return;
                }
                // Nothing named this one, because nothing said anything at all:
                // no manifest is bound here, so no verdict is coming, ever.
                // `cause: stale` is the whole of the answer, and a reason would
                // be a detector's name on a transition no detector caused.
                self.transition(
                    AgentState::Idle,
                    ExitAuthority::from_staleness().cause(),
                    None,
                    directives,
                );
            }
        }
    }

    /// The pane's process ended.
    pub(super) fn on_exited(&mut self, directives: &mut Vec<Directive>) {
        self.exited = true;
        let from = self.state;
        self.pending = None;
        self.state = AgentState::Quiet;
        self.cause = StatusCause::Exited;
        // The process ending is not a detection, and the last rule that named
        // this pane named a state it is no longer in.
        self.reason = None;
        directives.push(Directive::Status {
            from: self.reported.then_some(from),
            to: AgentState::Quiet,
            cause: StatusCause::Exited,
        });
        self.reported = true;
        if from.wants_attention() {
            directives.push(Directive::Dequeue);
        }
        for deadline in [
            Deadline::Confirmation,
            Deadline::Staleness,
            Deadline::IdentityGrace,
        ] {
            self.disarm(deadline, directives);
        }
    }
}
