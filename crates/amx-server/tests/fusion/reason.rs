//! D-M4-3: the pane's `reason` is its detector's own name, and nothing else.
//!
//! `docs/10-attention-surfaces.md` §D15 first wrote `"reason": "permission"`
//! against a comment reading `permission | idle_prompt | …`, which is a second
//! vocabulary somebody has to keep in step with every manifest that ships. The
//! plan replaced it with the name the detector already answers to, so these
//! scenarios are about *provenance*: which of the three tiers moved the pane,
//! spelled the way that tier spells itself.
//!
//! The absences carry as much of it as the names. A tier-3 `busy`, a staleness
//! exit and a dead process are all transitions no named rule caused, and a
//! `reason` on any of them would be a detector's name on something the detector
//! did not do.

use amx_core::agent::{Activity, AgentSnapshot, AgentState, CoverageClass, StatusCause};
use amx_proto::control::agent::HookEvent;
use amx_server::agent::fusion::{
    CONFIRMATION_CAP, CONFIRMATIONS, Deadline, IDENTITY_GRACE, Input, STALENESS, Tracker,
};

use crate::harness::{agent, claude, hook, named_screen, visible_idle};

/// Tier 1 names itself: the event is the reason.
///
/// `HookEvent::as_str` is the same spelling `amx _hook` forwarded and the same
/// one `docs/notes/hook-coverage.md` recorded, so nothing between the agent and
/// the wire renames anything.
#[test]
fn a_hook_asserted_state_is_named_by_the_hook_event() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    assert_eq!(tracker.state, AgentState::Blocked);
    assert_eq!(tracker.cause, StatusCause::Hook);
    assert_eq!(tracker.reason.as_deref(), Some("PermissionRequest"));

    // And the next edge renames it, because the next edge moved it.
    tracker.apply(hook(HookEvent::UserPromptSubmit));
    assert_eq!(tracker.state, AgentState::Working);
    assert_eq!(tracker.reason.as_deref(), Some("UserPromptSubmit"));
}

/// Tier 2 names itself too: the winning rule, not a category it belongs to.
#[test]
fn a_screen_asserted_state_is_named_by_the_rule_that_won() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(named_screen(AgentState::Working, "spinner_line_working"));
    assert_eq!(tracker.cause, StatusCause::Screen);
    assert_eq!(tracker.reason.as_deref(), Some("spinner_line_working"));

    // A `visible_idle` rule bypasses the confirmation hold, and names the
    // transition it bypassed it for.
    tracker.apply(visible_idle(AgentState::Idle));
    assert_eq!(tracker.state, AgentState::Idle);
    assert_eq!(tracker.reason.as_deref(), Some("idle.prompt_box"));
}

/// A contradiction confirmed over three verdicts commits under the rule that
/// was still asserting it — including when it is the cap that commits it.
///
/// The cap is the *common* path, not an exotic one: V01 measured every
/// user-initiated exit as silent, so a dialog the user cancelled goes quiet
/// after one verdict and the cap is what ends the block. A held rule name is
/// what keeps that transition from being the one row of the agents view with
/// nothing in its reason column.
#[test]
fn a_confirmed_contradiction_is_named_by_the_rule_that_carried_it() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    assert_eq!(tracker.reason.as_deref(), Some("PermissionRequest"));

    for _ in 0..CONFIRMATIONS {
        tracker.apply(named_screen(AgentState::Idle, "prompt_box_idle"));
    }
    assert_eq!(tracker.state, AgentState::Idle);
    assert_eq!(
        tracker.reason.as_deref(),
        Some("prompt_box_idle"),
        "the screen took the exit, so the screen's rule names it",
    );

    // The same exit, taken at the cap with the hold one verdict short.
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    tracker.apply(named_screen(AgentState::Idle, "prompt_box_idle"));
    assert_eq!(
        tracker.reason.as_deref(),
        Some("PermissionRequest"),
        "an accumulating contradiction has not moved the pane yet",
    );
    assert_eq!(CONFIRMATION_CAP.as_millis(), 700);
    tracker.apply(Input::Deadline(Deadline::Confirmation));
    assert_eq!(tracker.state, AgentState::Idle);
    assert_eq!(tracker.reason.as_deref(), Some("prompt_box_idle"));
}

/// Tier 3 has no detector to name, so it names nothing.
///
/// 04 §5 gives it two words and no rules: it saw output on a pty, or it saw
/// silence. `cause: probe` is the whole of the provenance there is.
#[test]
fn tier_three_names_nothing() {
    let mut tracker = Tracker::new();
    tracker.apply(Input::Probe(Activity::Busy));
    assert_eq!(tracker.state, AgentState::Busy);
    assert_eq!(tracker.cause, StatusCause::Probe);
    assert_eq!(tracker.reason, None);

    // Identification moves the pane out of tier 3's vocabulary, and that move
    // is tier 3's too — it is the activity it was already showing, reread.
    let directives = tracker.identify(claude(), CoverageClass::Edges, IDENTITY_GRACE);
    assert_eq!(tracker.state, AgentState::Working);
    assert_eq!(tracker.reason, None);
    assert!(!directives.is_empty(), "identification did happen");
}

/// A re-assertion corroborates; it does not move the pane, and so it does not
/// rename it.
///
/// This is the rule that keeps `reason` and the hub's `since` describing one
/// edge. A pane blocked by `permission_dialog` at 14:02 that gets a
/// `PermissionRequest` re-assertion at 14:06 has been blocked since 14:02 for
/// the reason it was blocked for, and a renamed reason with an unmoved `since`
/// would be two halves of two different answers.
#[test]
fn a_re_assertion_leaves_the_name_of_whatever_moved_the_pane() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(named_screen(AgentState::Blocked, "permission_dialog"));
    assert_eq!(tracker.reason.as_deref(), Some("permission_dialog"));

    // The hook that would have asserted the same state, arriving late.
    tracker.apply(hook(HookEvent::PermissionRequest));
    assert_eq!(tracker.state, AgentState::Blocked);
    assert_eq!(
        tracker.reason.as_deref(),
        Some("permission_dialog"),
        "the hook agreed with the block; it did not cause it",
    );

    // And the screen agreeing with itself changes nothing either.
    tracker.apply(named_screen(AgentState::Blocked, "some_other_rule"));
    assert_eq!(tracker.reason.as_deref(), Some("permission_dialog"));
}

/// The staleness exit is 04 §5's third one, taken because *nothing* said
/// anything for 30 s. There is no detector to credit.
#[test]
fn a_staleness_exit_names_nothing_because_nothing_named_it() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    assert!(tracker.is_armed(Deadline::Staleness));
    assert_eq!(STALENESS.as_secs(), 30);

    tracker.apply(Input::Deadline(Deadline::Staleness));
    assert_eq!(tracker.state, AgentState::Idle);
    assert_eq!(tracker.cause, StatusCause::Staleness);
    assert_eq!(
        tracker.reason, None,
        "a name here would credit a rule for a transition it did not make",
    );
}

/// A pane whose process ended is not in the state its last rule named.
#[test]
fn an_exit_clears_the_name_of_a_state_the_pane_has_left() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(named_screen(AgentState::Blocked, "permission_dialog"));
    assert_eq!(tracker.reason.as_deref(), Some("permission_dialog"));

    tracker.apply(Input::Exited);
    assert_eq!(tracker.state, AgentState::Quiet);
    assert_eq!(tracker.cause, StatusCause::Exited);
    assert_eq!(tracker.reason, None);
}

/// R-M3-13's carry rule, extended to the new field: an inherited status keeps
/// the exporter's name, because no evaluation has happened in this process yet
/// and blanking it would drop a value a client is already rendering.
#[test]
fn an_adopted_tracker_continues_the_exporters_name() {
    let mut source = agent(CoverageClass::Edges);
    source.apply(named_screen(AgentState::Blocked, "permission_dialog"));
    let carried = AgentSnapshot {
        kind: Some(claude()),
        state: source.state,
        cause: source.cause,
        transition_seq: 41,
        attention: Some(0),
        session_ref: None,
        reason: source.reason.clone(),
        since: Some(1_754_650_000_000),
    };

    let mut successor = Tracker::new();
    successor.adopt(&carried, CoverageClass::Edges, IDENTITY_GRACE);
    assert_eq!(successor.state, AgentState::Blocked);
    assert_eq!(successor.reason.as_deref(), Some("permission_dialog"));

    // And it is replaced by the first thing that actually moves the pane in
    // this process. A held state needs an exit authority to leave, so the
    // `visible_idle` rule is the shortest one — a screen verdict short of the
    // confirmation window would accumulate against the block without moving it,
    // and the carried name would still be the right answer.
    successor.apply(visible_idle(AgentState::Idle));
    assert_eq!(successor.state, AgentState::Idle);
    assert_eq!(successor.reason.as_deref(), Some("idle.prompt_box"));
}
