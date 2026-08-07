//! M2's rows, frozen: the twelve method goldens and the event-delivery
//! envelopes (`docs/08-m2-plan.md` §4).
//!
//! A child module of [`super`] rather than a second suite, because it shares
//! that file's `check_golden`/`call_golden` and its fixture ids — two golden
//! writers with two notions of what a pane id is would produce two sets of
//! bytes for one wire. Split out because the milestone roughly doubled the
//! file and the two halves change for different reasons: M0/M1's rows are
//! frozen, M2's are being built.
//!
//! Values here are documentation. Where a field has a measured shape, it
//! carries the measured value — a Claude Code session id is the v4-shaped UUID
//! V01 recorded, a `PermissionRequest` carries the `tool_name` its payload
//! actually has — so a reader of the golden learns what the call looks like in
//! the field rather than what a generator's zeros look like.

use amx_core::agent::{
    AgentKind, AgentSnapshot, AgentState, HookToken, RefKind, RefSource, SessionRef, StatusCause,
};
use amx_core::event::{Delivery, Envelope, Event};
use amx_core::{RowId, ShortNumber};
use amx_proto::control::pane::PaneTarget;
use amx_proto::control::{Call, agent, pane, wait};
use amx_proto::rpc::Notification;

use super::{call_golden, check_golden, pane_id, workspace_id};

/// A blocked agent, as the status view and the wire carry it.
///
/// Blocked on purpose: it is the state that carries the most shape — a queue
/// position, a cause that must be `hook` (V01 measured `PermissionRequest`
/// arriving 8–14 ms *ahead* of the dialog, so `Blocked` entry is the one edge
/// amx trusts outright) and a ref captured from `SessionStart`.
pub(super) fn agent_snapshot() -> AgentSnapshot {
    AgentSnapshot {
        kind: Some(agent_kind()),
        state: AgentState::Blocked,
        cause: StatusCause::Hook,
        transition_seq: 51,
        attention: Some(0),
        session_ref: Some(session_ref()),
    }
}

fn agent_kind() -> AgentKind {
    AgentKind::new("claude").unwrap()
}

fn session_ref() -> SessionRef {
    SessionRef::new(RefKind::Id, "c9a3c73b-b184-4871-8e98-79b46b87b635").unwrap()
}

/// M2's twelve rows (`docs/08-m2-plan.md` §4), each frozen with an example
/// request and reply.
///
/// The coverage law in `tests/protocol.rs` derives what is owed from
/// `Method::ALL`, so a thirteenth row would fail there before it failed here.
/// This test exists so the twelve are *legible* — the values are documentation
/// of what each call looks like in the field, which a generator's zeros would
/// not be.
#[test]
fn method_goldens_cover_all_twelve_m2_rows() {
    call_golden(
        "method_agent_report",
        Call::AgentReport(Box::new(agent::ReportParams {
            pane: pane_id(),
            token: HookToken::new("b7f0c1a4d9e25638"),
            agent: agent_kind(),
            source: RefSource::for_agent(&agent_kind()),
            // The one edge amx enters `Blocked` from, with the payload that
            // says what the agent is blocked *on*.
            event: agent::HookEvent::PermissionRequest,
            seq: 1_754_524_800_123_456_789,
            scope: agent::ReportScope::Parent,
            session_id: Some("c9a3c73b-b184-4871-8e98-79b46b87b635".into()),
            transcript_path: Some("/home/s/.claude/projects/amx/c9a3c73b.jsonl".into()),
            start_source: None,
            tool: Some("Bash".into()),
            notification: None,
        })),
        agent::ReportReply {
            accepted: true,
            seq: 60,
        },
    );

    call_golden(
        "method_agent_start",
        Call::AgentStart(agent::StartParams {
            name: "dev".into(),
            kind: agent_kind(),
            args: vec!["--permission-mode".into(), "default".into()],
            cwd: Some("/home/s/amx".into()),
            workspace: Some(workspace_id()),
            timeout_ms: Some(30_000),
        }),
        agent::StartReply {
            pane: pane_id(),
            short: ShortNumber::new(3),
            readiness: agent::Readiness::Ready,
            agent: Some(AgentSnapshot {
                state: AgentState::Idle,
                cause: StatusCause::Screen,
                attention: None,
                ..agent_snapshot()
            }),
            seq: 61,
        },
    );

    call_golden(
        "method_agent_prompt",
        Call::AgentPrompt(agent::PromptParams {
            target: PaneTarget::new("dev"),
            text: "summarise the diff".into(),
            wait: agent::PromptWait::Blocked,
            timeout_ms: Some(120_000),
        }),
        agent::PromptReply {
            pane: pane_id(),
            agent: Some(agent_snapshot()),
            satisfied: true,
            submitted_seq: 62,
        },
    );

    call_golden(
        "method_agent_explain",
        Call::AgentExplain(agent::ExplainParams {
            target: PaneTarget::new("dev"),
        }),
        agent::ExplainReply {
            pane: pane_id(),
            kind: Some(agent_kind()),
            manifest: Some("claude.toml".into()),
            manifest_version: Some("1".into()),
            matched: Some("permission-dialog".into()),
            region_preview: vec![
                "  Bash(echo spike-permission-probe)".into(),
                "❯ 1. Yes".into(),
                "  2. No".into(),
            ],
            rules: vec![
                agent::RuleVerdict {
                    rule: "permission-dialog".into(),
                    priority: 100,
                    region: "bottom_non_empty_lines(8)".into(),
                    matched: true,
                    asserts: Some(AgentState::Blocked),
                    evidence: Some("contains \"1. Yes\"".into()),
                },
                agent::RuleVerdict {
                    rule: "idle-prompt".into(),
                    priority: 50,
                    region: "bottom_lines(3)".into(),
                    matched: false,
                    asserts: Some(AgentState::Idle),
                    evidence: None,
                },
            ],
            agent: Some(agent_snapshot()),
        },
    );

    call_golden(
        "method_agent_next",
        Call::AgentNext(agent::NextParams {}),
        agent::NextReply {
            pane: Some(pane_id()),
            workspace: Some(workspace_id()),
            waiting: 2,
            seq: 63,
        },
    );

    call_golden(
        "method_wait",
        Call::Wait(wait::WaitParams {
            until: wait::WaitUntil::Blocked,
            target: PaneTarget::new("dev"),
            timeout_ms: Some(60_000),
        }),
        wait::WaitReply {
            pane: pane_id(),
            satisfied: true,
            agent: Some(agent_snapshot()),
            status: None,
            seq: 64,
        },
    );

    call_golden(
        "method_events_subscribe",
        Call::EventsSubscribe(wait::SubscribeParams {
            after_seq: Some(41),
        }),
        wait::SubscribeReply { seq: 65 },
    );

    call_golden(
        "method_pane_send_text",
        Call::PaneSendText(pane::SendTextParams {
            target: PaneTarget::from(pane_id()),
            text: "cargo test\n".into(),
        }),
        pane::SendTextReply {
            pane: pane_id(),
            seq: 66,
        },
    );

    call_golden(
        "method_pane_send_keys",
        Call::PaneSendKeys(pane::SendKeysParams {
            target: PaneTarget::from(pane_id()),
            // `esc` is the one every M2 exit-test scenario sends: V01 measured
            // it emitting no hook event at all, on either agent.
            keys: vec!["ctrl+c".into(), "esc".into(), "f1".into()],
        }),
        pane::SendKeysReply {
            pane: pane_id(),
            keys: 3,
            seq: 67,
        },
    );

    call_golden(
        "method_pane_run",
        Call::PaneRun(pane::RunParams {
            target: PaneTarget::from(pane_id()),
            text: "claude --resume c9a3c73b-b184-4871-8e98-79b46b87b635".into(),
        }),
        pane::RunReply {
            pane: pane_id(),
            bracketed: true,
            seq: 68,
        },
    );

    call_golden(
        "method_pane_read",
        Call::PaneRead(pane::ReadParams {
            target: PaneTarget::from(pane_id()),
            lines: Some(3),
        }),
        pane::ReadReply {
            pane: pane_id(),
            rows: vec![
                "Interrupted · What should Claude do instead?".into(),
                String::new(),
                "> ".into(),
            ],
            seq: 69,
        },
    );

    call_golden(
        "method_pane_wait_output",
        Call::PaneWaitOutput(wait::WaitOutputParams {
            target: PaneTarget::from(pane_id()),
            match_text: None,
            regex: Some("^test result: ok".into()),
            timeout_ms: Some(300_000),
        }),
        wait::WaitOutputReply {
            pane: pane_id(),
            matched: true,
            line: Some("test result: ok. 41 passed; 0 failed".into()),
            row: Some(RowId::from_raw(118)),
            seq: 70,
        },
    );
}

/// The notification an `events.subscribe` delivery arrives as.
///
/// Two goldens, not one, and the second is the point: a `gap` is an ordinary
/// delivery on the same method, so a consumer cannot handle only what it
/// recognises and quietly skip the one message that says it missed something.
/// 04 §2 requires loss to be visible *and* recoverable, and this is the wire
/// half of that.
#[test]
fn event_delivery_notification_goldens_match() {
    check_golden(
        "notification_event",
        &Notification::new(
            wait::EVENT_METHOD,
            Some(
                serde_json::to_value(Delivery::Event(Envelope {
                    seq: 71,
                    event: Event::AgentStatus {
                        pane: pane_id(),
                        from: Some(AgentState::Working),
                        to: AgentState::Blocked,
                        cause: StatusCause::Hook,
                    },
                }))
                .unwrap(),
            ),
        ),
    );

    check_golden(
        "notification_event_gap",
        &Notification::new(
            wait::EVENT_METHOD,
            Some(serde_json::to_value(Delivery::Gap { from: 72, to: 96 }).unwrap()),
        ),
    );
}
