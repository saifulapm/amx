//! Builders and generators for the fusion suite.
//!
//! The machine takes data and returns data, so a "harness" here is nothing but
//! constructors terse enough that a scenario reads like the recording it came
//! from. The proptest strategies live here too: they are the interesting half
//! of V04's deliverable and they would otherwise bury the scenarios.

use std::time::Duration;

use amx_core::PaneId;
use amx_core::agent::{
    Activity, AgentKind, AgentState, CoverageClass, HookToken, RefKind, RefSource, SessionRef,
    StatusCause,
};
use amx_proto::control::agent::{HookEvent, ReportParams, ReportScope};
use amx_server::agent::fusion::edge::{IDLE_PROMPT, PERMISSION_PROMPT};
use amx_server::agent::fusion::{Deadline, Directive, HookEdge, Input, ScreenVerdict, Tracker};
use proptest::prelude::*;

/// Every deadline, for the invariants that must hold over all three.
pub const DEADLINES: [Deadline; 3] = [
    Deadline::Confirmation,
    Deadline::Staleness,
    Deadline::IdentityGrace,
];

/// The agent id both shipped stanzas' scenarios use.
pub fn claude() -> AgentKind {
    AgentKind::new("claude").unwrap()
}

/// A tracker for an identified agent of `class`, past its startup grace.
///
/// The grace is zero rather than lapsed-by-deadline so a scenario that is not
/// about the grace does not have to open by firing one; the two that *are*
/// about it build their own tracker.
pub fn agent(class: CoverageClass) -> Tracker {
    let mut tracker = Tracker::new();
    tracker.identify(claude(), class, Duration::ZERO);
    tracker
}

/// A parent-scoped hook edge for `event`, carrying nothing else.
pub fn hook(event: HookEvent) -> Input {
    Input::Hook(edge(event, false))
}

/// The same event, scoped to a subagent — an `agent_id` was present.
pub fn subagent(event: HookEvent) -> Input {
    Input::Hook(edge(event, true))
}

/// A bare edge, for the tables that want the value rather than the input.
pub fn edge(event: HookEvent, subagent: bool) -> HookEdge {
    HookEdge {
        event,
        subagent,
        tool: None,
        notification: None,
        session_ref: None,
    }
}

/// A `Notification` edge of `kind` — `permission_prompt` or `idle_prompt`.
pub fn notification(kind: &str) -> Input {
    Input::Hook(HookEdge {
        notification: Some(kind.to_owned()),
        ..edge(HookEvent::Notification, false)
    })
}

/// A tier-2 evaluation asserting `state`.
pub fn screen(state: AgentState) -> Input {
    Input::Screen(ScreenVerdict {
        asserts: Some(state),
        rule: Some("state.assert".to_owned()),
        visible_idle: false,
    })
}

/// A tier-2 evaluation asserting `state` from a rule flagged `visible_idle` —
/// the prompt box is actually painted.
pub fn visible_idle(state: AgentState) -> Input {
    Input::Screen(ScreenVerdict {
        asserts: Some(state),
        rule: Some("idle.prompt_box".to_owned()),
        visible_idle: true,
    })
}

/// A `skip_state_update` match: tier 2 declines to assert anything.
pub fn hold() -> Input {
    Input::Screen(ScreenVerdict {
        asserts: None,
        rule: Some("noise.spinner".to_owned()),
        visible_idle: false,
    })
}

/// Nothing in the manifest matched this screen.
pub fn no_match() -> Input {
    Input::Screen(ScreenVerdict {
        asserts: None,
        rule: None,
        visible_idle: false,
    })
}

/// Feed a whole scenario, returning every effect it produced in order.
pub fn drive(tracker: &mut Tracker, script: impl IntoIterator<Item = Input>) -> Vec<Directive> {
    script
        .into_iter()
        .flat_map(|input| tracker.apply(input))
        .collect()
}

/// The one status effect in `effects`, as `(from, to, cause)`.
///
/// Panics if there is more than one, which is the contract every scenario
/// leans on and the proptest states outright.
pub fn status(effects: &[Directive]) -> Option<(Option<AgentState>, AgentState, StatusCause)> {
    let mut found = effects.iter().filter_map(|effect| match effect {
        Directive::Status { from, to, cause } => Some((*from, *to, *cause)),
        _ => None,
    });
    let first = found.next();
    assert!(found.next().is_none(), "two status effects for one input");
    first
}

/// A report as `amx _hook` would send one, with the fields a scenario cares
/// about filled in.
pub fn report(event: HookEvent, session_id: Option<&str>, scope: ReportScope) -> ReportParams {
    ReportParams {
        pane: PaneId::new_v4(),
        token: HookToken::new("token"),
        agent: claude(),
        source: RefSource::for_agent(&claude()),
        event,
        seq: 1,
        scope,
        session_id: session_id.map(ToOwned::to_owned),
        transcript_path: session_id.map(|id| format!("/home/u/.claude/{id}.jsonl").into()),
        start_source: None,
        tool: None,
        notification: None,
    }
}

/// A ref shaped like the session ids V01 recorded.
pub fn session(id: &str) -> SessionRef {
    SessionRef::new(RefKind::Id, id).unwrap()
}

/// Which coverage classes a property runs over.
pub fn any_class() -> impl Strategy<Value = CoverageClass> {
    prop_oneof![
        Just(CoverageClass::Edges),
        Just(CoverageClass::Full),
        Just(CoverageClass::Identity),
        Just(CoverageClass::None),
    ]
}

/// Every event the emitter can forward, including one this build has never
/// heard of — these tools ship weekly.
fn any_event() -> impl Strategy<Value = HookEvent> {
    prop_oneof![
        Just(HookEvent::SessionStart),
        Just(HookEvent::SessionEnd),
        Just(HookEvent::UserPromptSubmit),
        Just(HookEvent::Stop),
        Just(HookEvent::PreToolUse),
        Just(HookEvent::PostToolUse),
        Just(HookEvent::PostToolUseFailure),
        Just(HookEvent::PermissionRequest),
        Just(HookEvent::SubagentStart),
        Just(HookEvent::SubagentStop),
        Just(HookEvent::Notification),
        Just(HookEvent::Other("SomethingNew".to_owned())),
    ]
}

/// An arbitrary edge: any event, either scope, either notification type, and
/// one of a small pool of session refs so `/clear`-style replacement happens
/// often enough to matter.
fn any_edge() -> impl Strategy<Value = HookEdge> {
    (
        any_event(),
        any::<bool>(),
        prop_oneof![
            Just(None),
            Just(Some(PERMISSION_PROMPT.to_owned())),
            Just(Some(IDLE_PROMPT.to_owned())),
        ],
        prop::option::of(0_u8..3),
    )
        .prop_map(|(event, subagent, notification, id)| HookEdge {
            event,
            subagent,
            tool: None,
            notification,
            session_ref: id.map(|id| session(&format!("0000-{id}"))),
        })
}

/// An arbitrary tier-2 verdict, including the two that assert nothing.
fn any_verdict() -> impl Strategy<Value = ScreenVerdict> {
    (
        prop_oneof![
            Just(None),
            Just(Some(AgentState::Idle)),
            Just(Some(AgentState::Working)),
            Just(Some(AgentState::Blocked)),
        ],
        any::<bool>(),
    )
        .prop_map(|(asserts, visible_idle)| ScreenVerdict {
            asserts,
            rule: asserts.map(|state| format!("state.{state}")),
            visible_idle,
        })
}

/// Anything at all that can reach a tracker.
///
/// Weighted towards the two tiers that actually stream — hooks and screens —
/// with exits rare, because a script that dies on its second input tests the
/// terminal rule and nothing else.
fn any_input() -> impl Strategy<Value = Input> {
    prop_oneof![
        8 => any_edge().prop_map(Input::Hook),
        8 => any_verdict().prop_map(Input::Screen),
        4 => prop_oneof![
            Just(Deadline::Confirmation),
            Just(Deadline::Staleness),
            Just(Deadline::IdentityGrace),
        ].prop_map(Input::Deadline),
        3 => prop_oneof![Just(Activity::Busy), Just(Activity::Quiet)].prop_map(Input::Probe),
        2 => prop_oneof![Just("claude"), Just("codex")]
            .prop_map(|id| Input::Identified { kind: AgentKind::new(id).unwrap() }),
        1 => Just(Input::Exited),
    ]
}

/// A whole interleaving.
pub fn script() -> impl Strategy<Value = Vec<Input>> {
    prop::collection::vec(any_input(), 0..24)
}

/// A tracker built the way the properties want it: a class, and a startup grace
/// that is either irrelevant or genuinely pending.
///
/// The identification's own effects come back with it, because the hub applied
/// them too — a property that replays the effect stream has to start from the
/// same place the hub did, grace timer and all.
pub fn any_tracker() -> impl Strategy<Value = (Tracker, Vec<Directive>)> {
    (any_class(), prop_oneof![Just(0_u64), Just(3_000_u64)]).prop_map(|(class, grace)| {
        let mut tracker = Tracker::new();
        let effects = tracker.identify(claude(), class, Duration::from_millis(grace));
        (tracker, effects)
    })
}
