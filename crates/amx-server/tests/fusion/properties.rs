//! The properties, over arbitrary interleavings.
//!
//! D-M2-6 makes these as much V04's deliverable as the machine: "arbitrary
//! interleavings of hook edges, screen verdicts, and deadline fires must never
//! revive an idle pane from a subagent event, never wedge a tracker with an
//! unfired deadline, and never emit two status effects for one transition."
//!
//! The scenarios in the parent file replay what V01 recorded. These replay what
//! it did not: every order those inputs could have arrived in, across all four
//! coverage classes, including the ones no agent has ever produced. A machine
//! with no clock can be tested this way — which is exactly why deadlines are
//! inputs.

use std::collections::HashSet;

use amx_core::agent::{AgentState, CoverageClass, StatusCause};
use amx_server::agent::fusion::{Deadline, Directive, Input};
use proptest::prelude::*;

use crate::harness::{DEADLINES, any_tracker, script};

/// Whether this input is a subagent-scoped hook report.
fn is_subagent(input: &Input) -> bool {
    matches!(input, Input::Hook(edge) if edge.subagent)
}

proptest! {
    /// 04 §5: "Subagent-scoped events never override the parent turn's state."
    ///
    /// Stated at its strongest, which is the only form that survives V01 §3
    /// M4's finding that an anonymous `SubagentStop` lands after the parent's
    /// `Stop` on essentially every tool-using turn: a subagent-scoped report is
    /// not merely harmless, it is *indistinguishable from never having
    /// arrived*. Run the same interleaving twice, once with the subagent
    /// reports and once with them deleted, and both the effect stream and the
    /// tracker itself come out identical.
    #[test]
    fn subagent_events_never_change_parent_state(
        (mut with, mut without) in any_tracker().prop_map(|(tracker, _)| (tracker.clone(), tracker)),
        script in script(),
    ) {
        let mut with_effects = Vec::new();
        let mut without_effects = Vec::new();
        for input in &script {
            with_effects.extend(with.apply(input.clone()));
            if !is_subagent(input) {
                without_effects.extend(without.apply(input.clone()));
            }
        }
        prop_assert_eq!(&with_effects, &without_effects);
        prop_assert_eq!(&with, &without);
    }

    /// 03 §5's promise: a session of idle agents costs zero wakeups.
    ///
    /// The hub arms and disarms timers purely from the effect stream, so the
    /// set it would be holding is replayed here and compared against what the
    /// tracker believes after *every* input. A drift either way is a wakeup
    /// nobody asked for or a deadline that will never fire — herdr's stuck
    /// status, wearing a timer.
    #[test]
    fn no_input_sequence_leaves_a_tracker_with_a_stale_deadline(
        (mut tracker, setup) in any_tracker(),
        script in script(),
    ) {
        let mut wheel: HashSet<Deadline> = HashSet::new();
        // The identification the tracker was built with reached the hub's wheel
        // like any other effects, so the replay starts where the hub did.
        let mut effects = setup;
        let mut inputs = script.into_iter();
        loop {
            for effect in effects {
                match effect {
                    Directive::Arm { deadline, .. } => { wheel.insert(deadline); }
                    Directive::Disarm { deadline } => { wheel.remove(&deadline); }
                    _ => {}
                }
            }
            for deadline in DEADLINES {
                prop_assert_eq!(
                    wheel.contains(&deadline),
                    tracker.is_armed(deadline),
                    "the wheel and the tracker disagree about {:?}",
                    deadline
                );
            }
            // And what each deadline is *for*, which is what makes an armed
            // timer non-stale rather than merely accounted for.
            prop_assert_eq!(
                tracker.is_armed(Deadline::Staleness),
                tracker.state.is_held(),
                "staleness is armed exactly while a held state is held",
            );
            prop_assert_eq!(
                tracker.is_armed(Deadline::Confirmation),
                tracker.pending().is_some(),
                "the cap is armed exactly while a contradiction is accumulating",
            );
            if tracker.is_exited() {
                prop_assert!(wheel.is_empty(), "a retired tracker holds no timer");
            }

            let Some(input) = inputs.next() else { break };
            // The hub drops a deadline from its wheel when it fires it.
            if let Input::Deadline(deadline) = input {
                wheel.remove(&deadline);
            }
            effects = tracker.apply(input);
        }
    }

    /// One transition, one event — and one queue membership to match.
    ///
    /// `AgentHub` turns each [`Directive::Status`] into a `StatusView` write plus
    /// one published `agent_status`, so two for one input would publish a
    /// transition that never happened, and none for a state change would leave
    /// a `wait --until blocked` asleep through the thing it was waiting for.
    #[test]
    fn every_transition_emits_exactly_one_status_effect(
        (mut tracker, _) in any_tracker(),
        script in script(),
    ) {
        let mut queued = false;
        for input in script {
            let before = tracker.state;
            let effects = tracker.apply(input);
            let statuses: Vec<_> = effects
                .iter()
                .filter_map(|effect| match effect {
                    Directive::Status { from, to, cause } => Some((*from, *to, *cause)),
                    _ => None,
                })
                .collect();
            prop_assert!(statuses.len() <= 1, "two status effects for one input");
            if tracker.state != before {
                prop_assert_eq!(statuses.len(), 1, "a state change published nothing");
            }
            if let Some((from, to, cause)) = statuses.first().copied() {
                prop_assert_eq!(to, tracker.state);
                prop_assert_eq!(cause, tracker.cause);
                prop_assert!(from.is_none_or(|from| from == before));
                prop_assert!(
                    from != Some(to) || cause == StatusCause::Exited,
                    "a status effect that moved nothing",
                );
            }
            for effect in &effects {
                match effect {
                    Directive::Enqueue => {
                        prop_assert!(!queued, "enqueued twice without leaving");
                        queued = true;
                    }
                    Directive::Dequeue => {
                        prop_assert!(queued, "dequeued without being on the queue");
                        queued = false;
                    }
                    _ => {}
                }
            }
            prop_assert_eq!(
                queued,
                tracker.state.wants_attention(),
                "the attention queue is exactly the blocked panes",
            );
        }
    }

    /// 04 §5: "**Exits from working/blocked are confirmed**, not trusted."
    ///
    /// The rule as a property, over every class: a pane leaves `Working` or
    /// `Blocked` for an unheld state only under one of the three authorities —
    /// the screen, the staleness deadline, or (for a `full` agent alone) a
    /// hook — plus the pane's own death. Tier 3 is not on that list and neither
    /// is an `edges` agent's `Stop`, which is the whole of V01's finding that
    /// every exit-by-user is silent.
    #[test]
    fn a_held_state_is_only_left_through_an_exit_authority(
        (mut tracker, _) in any_tracker(),
        script in script(),
    ) {
        let class = tracker.coverage;
        for input in script {
            let before = tracker.state;
            for effect in tracker.apply(input) {
                let Directive::Status { to, cause, .. } = effect else {
                    continue;
                };
                if !before.is_held() || to.is_held() {
                    continue;
                }
                let allowed = match cause {
                    StatusCause::Screen | StatusCause::Staleness | StatusCause::Exited => true,
                    StatusCause::Hook => class == CoverageClass::Full,
                    StatusCause::Probe => false,
                };
                prop_assert!(
                    allowed,
                    "{:?} left {:?} for {:?} on a {} agent",
                    cause,
                    before,
                    to,
                    class
                );
                prop_assert_ne!(to, AgentState::Busy, "an identified agent never goes busy");
            }
        }
    }
}
