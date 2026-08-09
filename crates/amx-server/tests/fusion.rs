#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! V04 acceptance: the fusion state machine, over recorded transitions.
//!
//! Every scenario here is a transition `docs/notes/hook-coverage.md` measured
//! on Claude Code 2.1.224 or Codex CLI 0.147.0, replayed as the inputs the hub
//! would hand a tracker. The machine has no clock and no I/O, so a scenario is
//! literally the recording: the hook events that fired, in order, and — for the
//! four transitions V01 found silent — the ones that did not.
//!
//! The silences are the point. An Esc during generation, an Esc during a tool
//! call, a permission dialog answered "No" and a dialog cancelled with Esc emit
//! **nothing at all** on either agent, so half of these scenarios prove what
//! the tracker does when tier 1 says nothing whatsoever.
//!
//! The properties in [`properties`] carry the other half of the deliverable:
//! the scenarios show that the measured transitions land, the properties show
//! that no *unmeasured* interleaving can wedge a tracker, revive an idle pane
//! or leave a timer armed against a state that has moved on.

use std::time::Duration;

use amx_core::agent::{Activity, AgentState, CoverageClass, RefKind, SessionRef, StatusCause};
use amx_proto::control::agent::{HookEvent, ReportScope};
use amx_server::agent::fusion::edge::{IDLE_PROMPT, PERMISSION_PROMPT, precedence};
use amx_server::agent::fusion::{
    CONFIRMATION_CAP, CONFIRMATIONS, Deadline, Directive, EdgeEffect, HookEdge, IDENTITY_GRACE,
    Input, STALENESS, Tracker,
};

// A test crate root's module directory is `tests/`, so both of this suite's
// modules spell their path rather than landing next to everybody else's.
#[path = "fusion/harness.rs"]
mod harness;
#[path = "fusion/properties.rs"]
mod properties;
#[path = "fusion/reason.rs"]
mod reason;

use harness::{
    agent, claude, drive, edge, hold, hook, named_screen, no_match, notification, report, screen,
    session, status, subagent, visible_idle,
};

/// The entry edges, applied the instant they arrive.
///
/// V01 §3 M9 measured `UserPromptSubmit` starting its hook process a median of
/// 26 ms after the keystroke, and §3 M3 measured `PermissionRequest` starting
/// 8–14 ms *before* the permission dialog paints. Neither needs corroboration:
/// a state taken from them is never late relative to the screen.
#[test]
fn hook_entry_edges_apply_instantly_for_edges_class() {
    let mut tracker = agent(CoverageClass::Edges);
    assert_eq!(tracker.state, AgentState::Idle);

    let effects = tracker.apply(hook(HookEvent::UserPromptSubmit));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Idle),
            AgentState::Working,
            StatusCause::Hook
        ))
    );
    assert!(effects.contains(&Directive::Arm {
        deadline: Deadline::Staleness,
        after: STALENESS,
    }));

    // A second entry edge for the state the pane is already in is
    // corroboration, not a transition: no event, and the staleness clock
    // starts again.
    let effects = tracker.apply(hook(HookEvent::PreToolUse));
    assert_eq!(status(&effects), None);
    assert_eq!(
        effects,
        vec![Directive::Arm {
            deadline: Deadline::Staleness,
            after: STALENESS,
        }]
    );

    // A permission dialog interrupts a turn in progress, and the pane joins the
    // attention queue in the same breath.
    let effects = tracker.apply(hook(HookEvent::PermissionRequest));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Working),
            AgentState::Blocked,
            StatusCause::Hook
        ))
    );
    assert!(effects.contains(&Directive::Enqueue));

    // The tool ran, so the answer was yes: back to working, off the queue.
    let effects = tracker.apply(hook(HookEvent::PostToolUse));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Blocked),
            AgentState::Working,
            StatusCause::Hook
        ))
    );
    assert!(effects.contains(&Directive::Dequeue));

    // And the one edge that does *not* apply: `Stop` fires when a turn ends by
    // itself, but exits are screen-owned for an `edges` agent, because the
    // interrupted turn — which V01 found is the common case — emits nothing at
    // all. One exit path, always.
    assert_eq!(tracker.apply(hook(HookEvent::Stop)), Vec::new());
    assert_eq!(tracker.state, AgentState::Working);
}

/// V01 §7 edge case 1 and 2: Esc during generation, and Esc during a tool call.
///
/// `UserPromptSubmit` (or `PreToolUse`), then no event ever — the log stays
/// empty while the screen goes from `esc to interrupt` to `Interrupted · What
/// should Claude do instead?`. Tier 2 is the only witness, so it has to carry
/// the transition alone, and it does it under the confirmation window.
#[test]
fn esc_interrupt_with_no_hook_event_settles_idle_via_screen_confirmations() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::UserPromptSubmit));
    tracker.apply(hook(HookEvent::PreToolUse));

    // The user presses Esc. Nothing at all arrives from tier 1, ever again.
    let first = tracker.apply(screen(AgentState::Idle));
    assert_eq!(status(&first), None, "one verdict is not a confirmation");
    assert_eq!(
        first,
        vec![Directive::Arm {
            deadline: Deadline::Confirmation,
            after: CONFIRMATION_CAP,
        }]
    );
    assert_eq!(tracker.pending(), Some(AgentState::Idle));

    // The cap is armed once, at the head of the hold — the second verdict does
    // not push it out.
    assert_eq!(tracker.apply(screen(AgentState::Idle)), Vec::new());
    assert_eq!(tracker.state, AgentState::Working);

    let third = tracker.apply(screen(AgentState::Idle));
    assert_eq!(
        status(&third),
        Some((
            Some(AgentState::Working),
            AgentState::Idle,
            StatusCause::Screen
        ))
    );
    assert_eq!(
        third,
        vec![
            Directive::Status {
                from: Some(AgentState::Working),
                to: AgentState::Idle,
                cause: StatusCause::Screen,
            },
            Directive::Disarm {
                deadline: Deadline::Confirmation,
            },
            Directive::Disarm {
                deadline: Deadline::Staleness,
            },
        ],
        "settling disarms both timers it was holding",
    );
    assert_eq!(CONFIRMATIONS, 3, "three verdicts, per V01 §6");
}

/// V01 §7 edge cases 3 and 4: a dialog answered "No", and one cancelled with
/// Esc.
///
/// Byte-identical hook traces (nothing) and byte-identical screens
/// (`Interrupted · What should Claude do instead?`), which is why the fixtures
/// must not distinguish them: nothing downstream can. Here the screen goes
/// quiet after one verdict — the user walked away from a settled terminal — and
/// the confirmation cap is what ends the block.
#[test]
fn dialog_cancel_with_no_hook_event_unblocks_within_the_confirmation_cap() {
    let mut tracker = agent(CoverageClass::Edges);
    let blocked = tracker.apply(hook(HookEvent::PermissionRequest));
    assert_eq!(tracker.state, AgentState::Blocked);
    assert!(blocked.contains(&Directive::Enqueue));

    let pending = tracker.apply(screen(AgentState::Idle));
    assert_eq!(
        pending,
        vec![Directive::Arm {
            deadline: Deadline::Confirmation,
            after: CONFIRMATION_CAP,
        }]
    );
    assert_eq!(CONFIRMATION_CAP, Duration::from_millis(700));

    // No further damage, so no further verdict: the cap fires with the
    // contradiction still one short of the window, and it is taken anyway.
    let capped = tracker.apply(Input::Deadline(Deadline::Confirmation));
    assert_eq!(
        status(&capped),
        Some((
            Some(AgentState::Blocked),
            AgentState::Idle,
            StatusCause::Screen
        ))
    );
    assert!(capped.contains(&Directive::Dequeue));
    assert!(!tracker.is_armed(Deadline::Confirmation));
    assert!(!tracker.is_armed(Deadline::Staleness));
}

/// herdr's flicker fix, kept as data: a rule that matched the prompt box
/// itself is not a guess about idleness, so it does not serve the hold.
#[test]
fn visible_idle_bypasses_the_confirmation_hold() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::UserPromptSubmit));

    let effects = tracker.apply(visible_idle(AgentState::Idle));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Working),
            AgentState::Idle,
            StatusCause::Screen
        ))
    );
    assert_eq!(tracker.pending(), None);

    // And the ordinary rule still waits its three turns, from the same state.
    tracker.apply(hook(HookEvent::UserPromptSubmit));
    assert_eq!(status(&tracker.apply(screen(AgentState::Idle))), None);
}

/// 04 §5's third exit, and V01 §7 edge case 13 — the pane it is *for*.
///
/// A Codex approval the user answered "No, and tell Codex what to do
/// differently" produces nothing further — no `PostToolUse`, no `Stop`, for as
/// long as the spike watched. A pane with no screen coverage at all, which is a
/// stanza that names no manifest, would hold `Blocked` for the life of the
/// session: nothing is reading its screen, so no verdict is ever coming. The
/// staleness deadline is the only thing that can end it, and it does.
///
/// The scope of that is the point, and it is narrower than this test used to
/// read: "no screen coverage" is *nobody is looking*, not *nobody recognised
/// what they saw*. The two tests below are the other side of the same rule.
#[test]
fn staleness_deadline_clears_a_hook_held_state_with_no_screen_coverage() {
    let mut tracker = agent(CoverageClass::Edges);
    let blocked = tracker.apply(hook(HookEvent::PermissionRequest));
    assert!(blocked.contains(&Directive::Arm {
        deadline: Deadline::Staleness,
        after: STALENESS,
    }));
    assert_eq!(STALENESS, Duration::from_secs(30));

    // Tier 2 never speaks — not once. Only the deadline does.
    let effects = tracker.apply(Input::Deadline(Deadline::Staleness));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Blocked),
            AgentState::Idle,
            StatusCause::Staleness
        ))
    );
    assert!(effects.contains(&Directive::Dequeue));
    assert!(!tracker.is_armed(Deadline::Staleness));

    // A screen verdict that agrees with a held state pushes the deadline out,
    // so a genuinely working pane is never cleared for staleness.
    tracker.apply(hook(HookEvent::UserPromptSubmit));
    assert_eq!(
        tracker.apply(screen(AgentState::Working)),
        vec![Directive::Arm {
            deadline: Deadline::Staleness,
            after: STALENESS,
        }]
    );
}

/// The M4 exit's first defect, at the machine: a block the screen is still
/// showing is not demoted by the clock.
///
/// `docs/notes/m4-live-smoke.md` §4.8 measured this against a stand-in — a pane
/// whose permission dialog was still on screen, whose `agent explain` still
/// reported the shipped `permission_dialog` rule matching it, and which the
/// deadline called `idle` 30.4 s after its last paint anyway. Tier 2 runs on
/// damage and a dialog waiting for a human produces none, so the last verdict
/// is the last *change*, and the deadline was reading the silence after it as
/// evidence.
///
/// What the fire does instead is nothing at all, loudly: the state stands, the
/// clock starts again, and the whole of its output is the re-arm — so the
/// attention queue keeps the pane and nothing is published about a pane that
/// did not move.
#[test]
fn staleness_holds_a_block_the_screen_is_still_showing() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    tracker.apply(named_screen(AgentState::Blocked, "permission_dialog"));
    assert_eq!(tracker.state, AgentState::Blocked);

    // Thirty seconds in which the dialog sat there and nothing repainted it.
    let effects = tracker.apply(Input::Deadline(Deadline::Staleness));
    assert_eq!(status(&effects), None, "nobody said the block ended");
    assert_eq!(
        effects,
        vec![Directive::Arm {
            deadline: Deadline::Staleness,
            after: STALENESS,
        }],
        "the clock starts again and nothing else happens",
    );
    assert_eq!(tracker.state, AgentState::Blocked);
    assert_eq!(tracker.reason.as_deref(), Some("PermissionRequest"));
    assert!(tracker.is_armed(Deadline::Staleness));

    // And it is not one reprieve: the next thirty seconds go the same way, and
    // the one after that, for as long as the dialog is what the screen shows.
    for _ in 0..4 {
        assert_eq!(
            tracker.apply(Input::Deadline(Deadline::Staleness)),
            vec![Directive::Arm {
                deadline: Deadline::Staleness,
                after: STALENESS,
            }]
        );
    }
    assert_eq!(tracker.state, AgentState::Blocked);

    // The exit is still there. It belongs to the screen, which is where 04 §5
    // puts an `edges` agent's exits, and it names the rule that took it.
    let effects = tracker.apply(visible_idle(AgentState::Idle));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Blocked),
            AgentState::Idle,
            StatusCause::Screen
        ))
    );
    assert!(effects.contains(&Directive::Dequeue));
    assert_eq!(tracker.reason.as_deref(), Some("idle.prompt_box"));
}

/// The same rule where it is hardest: tier 2 is looking and cannot read the
/// screen.
///
/// `docs/notes/m4-live-smoke.md` §6.8 is this with a real agent. The dialog was
/// a Write confirmation — *"Do you want to overwrite exit-probe.txt?"* — which
/// the shipped `permission_dialog` rule does not match, so `agent explain`
/// answered `matched: null` with the dialog plainly on the pane, the hook was
/// the only detector there was, and 35.4 s later `amx agent next` said
/// `waiting: 0`.
///
/// Silence from a detector that is looking is not a verdict. A screen tier 2
/// cannot name is a screen nobody has said anything about, and "nobody has said
/// anything" is the condition this deadline used to treat as proof — which is
/// exactly backwards for a pane whose block is the reason its screen stopped
/// moving.
#[test]
fn staleness_holds_a_block_tier_2_is_looking_at_and_cannot_name() {
    for verdict in [no_match(), hold()] {
        let mut tracker = agent(CoverageClass::Edges);
        tracker.apply(hook(HookEvent::PermissionRequest));
        tracker.apply(verdict);
        assert_eq!(tracker.state, AgentState::Blocked);

        let effects = tracker.apply(Input::Deadline(Deadline::Staleness));
        assert_eq!(status(&effects), None);
        assert_eq!(
            effects,
            vec![Directive::Arm {
                deadline: Deadline::Staleness,
                after: STALENESS,
            }]
        );
        assert_eq!(tracker.state, AgentState::Blocked);
    }

    // The same for a held `Working`: an interrupted turn whose screen no rule
    // matches is a turn amx cannot see the end of, not a turn that ended.
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::UserPromptSubmit));
    tracker.apply(no_match());
    assert_eq!(
        status(&tracker.apply(Input::Deadline(Deadline::Staleness))),
        None
    );
    assert_eq!(tracker.state, AgentState::Working);
}

/// A verdict from the *previous* agent's manifest is not coverage of this one.
///
/// Identity can move — tier 3 names a program the argv guess got wrong, or a
/// hook report claims a different agent — and the manifest moves with it. The
/// screen the old rules read has not been read by the new ones, so the pane is
/// back to having no witness but the clock until something evaluates it, and
/// the machine says so rather than crediting the old verdict.
#[test]
fn a_re_identified_pane_has_not_been_read_by_its_new_manifest() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    tracker.apply(named_screen(AgentState::Blocked, "permission_dialog"));
    assert_eq!(
        status(&tracker.apply(Input::Deadline(Deadline::Staleness))),
        None,
        "read by claude's rules, and held",
    );

    tracker.apply(Input::Identified {
        kind: harness::codex(),
    });
    assert_eq!(
        tracker.state,
        AgentState::Blocked,
        "identity moves no state"
    );
    assert_eq!(
        status(&tracker.apply(Input::Deadline(Deadline::Staleness))),
        Some((
            Some(AgentState::Blocked),
            AgentState::Idle,
            StatusCause::Staleness
        )),
        "and codex's rules have never seen this screen",
    );
}

/// The two classes at the ends of the table, read off the same inputs.
///
/// `identity` carries session identity only, so its state is the screen's
/// entirely; `full` covers the whole lifecycle, so its hooks close their own
/// turns. Nothing amx ships is `full` — V01 promoted nobody — but the class is
/// what keeps the `edges` rule honest about *why* it refuses a hook exit.
#[test]
fn identity_class_ignores_hook_state_and_full_class_trusts_it() {
    let mut identity = agent(CoverageClass::Identity);
    assert_eq!(
        identity.apply(hook(HookEvent::UserPromptSubmit)),
        Vec::new()
    );
    assert_eq!(
        identity.apply(hook(HookEvent::PermissionRequest)),
        Vec::new()
    );
    assert_eq!(identity.state, AgentState::Idle);
    // …but its screen still owns everything, immediately.
    let effects = identity.apply(screen(AgentState::Working));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Idle),
            AgentState::Working,
            StatusCause::Screen
        ))
    );

    let mut full = agent(CoverageClass::Full);
    let effects = full.apply(hook(HookEvent::UserPromptSubmit));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Idle),
            AgentState::Working,
            StatusCause::Hook
        ))
    );
    let effects = full.apply(hook(HookEvent::Stop));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Working),
            AgentState::Idle,
            StatusCause::Hook
        ))
    );

    // The type is what makes the difference, not an `if` in the table.
    assert_eq!(
        precedence(CoverageClass::Edges, &edge(HookEvent::Stop, false)),
        EdgeEffect::Ignore
    );
    assert!(matches!(
        precedence(CoverageClass::Full, &edge(HookEvent::Stop, false)),
        EdgeEffect::Leave(_)
    ));
}

/// V01 §7 edge case 5, which is the *default* behaviour of a tool-using turn:
/// an anonymous `SubagentStop`, with an `agent_id` no `SubagentStart` ever
/// announced, arrives 1.9–3.0 s after the parent's `Stop`.
///
/// By then the pane has long settled `Idle` on the screen. herdr's "never
/// revive an idle pane" is this test.
#[test]
fn an_anonymous_subagent_stop_after_the_parent_stop_never_revives_the_pane() {
    let mut tracker = agent(CoverageClass::Edges);
    drive(
        &mut tracker,
        [
            hook(HookEvent::UserPromptSubmit),
            hook(HookEvent::PreToolUse),
            subagent(HookEvent::SubagentStart),
            subagent(HookEvent::SubagentStop),
            hook(HookEvent::PostToolUse),
            hook(HookEvent::Stop),
            screen(AgentState::Idle),
            screen(AgentState::Idle),
            screen(AgentState::Idle),
        ],
    );
    assert_eq!(tracker.state, AgentState::Idle);

    // Two seconds later, out of nowhere.
    assert_eq!(tracker.apply(subagent(HookEvent::SubagentStop)), Vec::new());
    assert_eq!(tracker.state, AgentState::Idle);

    // And a subagent's *entry* edges are no more welcome than its exits.
    assert_eq!(
        tracker.apply(subagent(HookEvent::UserPromptSubmit)),
        Vec::new()
    );
    assert_eq!(
        tracker.apply(subagent(HookEvent::PermissionRequest)),
        Vec::new()
    );
    assert_eq!(tracker.state, AgentState::Idle);

    // The rule is enforced twice on purpose — the table refuses the row and the
    // tracker refuses the input, because the tracker's guard also covers the
    // ref, which is not the table's business. Both are asserted, so neither can
    // be deleted as "already handled by the other one".
    for event in [
        HookEvent::UserPromptSubmit,
        HookEvent::PermissionRequest,
        HookEvent::SubagentStop,
    ] {
        for class in [
            CoverageClass::Full,
            CoverageClass::Edges,
            CoverageClass::Identity,
            CoverageClass::None,
        ] {
            assert_eq!(
                precedence(class, &edge(event.clone(), true)),
                EdgeEffect::Ignore,
                "{event:?} on a {class} agent"
            );
        }
    }
}

/// V01 §7 edge case 9: a tool call refused by a `permissions.deny` rule fires
/// `PreToolUse` → `PostToolBatch` → `Stop` — no `PostToolUse` at all.
///
/// "A tracker that expects `PostToolUse` to close every `PreToolUse` will
/// wedge." Nothing here pairs the two: `PreToolUse` asserts `Working` and the
/// screen ends it, exactly as an interrupt does.
#[test]
fn a_rule_denied_tool_call_does_not_wedge_the_tracker() {
    let mut tracker = agent(CoverageClass::Edges);
    drive(
        &mut tracker,
        [hook(HookEvent::PreToolUse), hook(HookEvent::Stop)],
    );
    assert_eq!(tracker.state, AgentState::Working);

    // `skip_state_update` and "nothing matched" assert nothing, so neither
    // serves the confirmation window nor resets it.
    drive(&mut tracker, [screen(AgentState::Idle), hold(), no_match()]);
    assert_eq!(tracker.pending(), Some(AgentState::Idle));
    assert_eq!(tracker.state, AgentState::Working);

    drive(
        &mut tracker,
        [screen(AgentState::Idle), screen(AgentState::Idle)],
    );
    assert_eq!(tracker.state, AgentState::Idle);
    assert_eq!(tracker.cause, StatusCause::Screen);
}

/// V01 §3 M5's second witness, and the sibling that is deliberately not one.
///
/// A `permission_prompt` notification 6 s into an unanswered dialog proves the
/// block is real, so it pushes the staleness deadline out. An `idle_prompt` at
/// 60 s is a genuine contradiction of a held `Working` — and still not an exit:
/// the screen has answered the question forty times over by then, and taking an
/// exit from a hook is what `ExitAuthority::from_hook` refuses an `edges` agent.
#[test]
fn a_permission_notification_corroborates_a_block_and_an_idle_one_never_exits() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));

    let effects = tracker.apply(notification(PERMISSION_PROMPT));
    assert_eq!(status(&effects), None);
    assert_eq!(
        effects,
        vec![Directive::Arm {
            deadline: Deadline::Staleness,
            after: STALENESS,
        }]
    );

    tracker.apply(hook(HookEvent::UserPromptSubmit));
    assert_eq!(tracker.state, AgentState::Working);
    assert_eq!(tracker.apply(notification(IDLE_PROMPT)), Vec::new());
    assert_eq!(tracker.state, AgentState::Working);
}

/// Identity lands, and the screen is not read until the splash has stopped
/// matching nonsense.
///
/// The grace is per-agent because V01 §6 measured the two shipped agents
/// differing: Claude Code is ready about 1.1 s after launch, Codex took
/// 2.8–4.6 s and emits no hook at all until the first prompt.
#[test]
fn identification_reads_the_probe_and_holds_the_screen_for_the_startup_grace() {
    let mut tracker = Tracker::new();
    assert_eq!(tracker.state, AgentState::Quiet);

    // Tier 3 sees output arriving from a program it cannot name yet.
    let effects = tracker.apply(Input::Probe(Activity::Busy));
    assert_eq!(
        status(&effects),
        Some((None, AgentState::Busy, StatusCause::Probe)),
        "a pane's first status has no `from`",
    );

    let effects = tracker.identify(claude(), CoverageClass::Edges, IDENTITY_GRACE);
    assert!(effects.contains(&Directive::Identified { kind: claude() }));
    assert!(effects.contains(&Directive::Arm {
        deadline: Deadline::IdentityGrace,
        after: IDENTITY_GRACE,
    }));
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Busy),
            AgentState::Working,
            StatusCause::Probe
        )),
        "a named program may not keep tier 3's vocabulary",
    );

    // The splash screen matches whatever it matches; nobody is listening.
    assert_eq!(tracker.apply(screen(AgentState::Idle)), Vec::new());
    assert_eq!(tracker.apply(visible_idle(AgentState::Idle)), Vec::new());
    assert_eq!(tracker.pending(), None);

    assert_eq!(
        tracker.apply(Input::Deadline(Deadline::IdentityGrace)),
        Vec::new()
    );
    assert_eq!(
        status(&tracker.apply(visible_idle(AgentState::Idle))),
        Some((
            Some(AgentState::Working),
            AgentState::Idle,
            StatusCause::Screen
        ))
    );

    // And tier 3 has nothing more to say about a pane whose program has a name.
    assert_eq!(tracker.apply(Input::Probe(Activity::Busy)), Vec::new());
    assert_eq!(tracker.state, AgentState::Idle);
}

/// The ref is taken from every report that carries one, because `/clear` mints
/// a new session id inside one process (V01 §3 M8) and a ref captured before it
/// would resume the wrong conversation.
#[test]
fn a_new_session_id_replaces_the_ref_and_a_repeat_of_it_does_not() {
    let mut tracker = agent(CoverageClass::Edges);
    let first = report(
        HookEvent::SessionStart,
        Some("a8e2f3d1"),
        ReportScope::Parent,
    );
    let effects = tracker.apply(Input::Hook(HookEdge::from_report(&first, RefKind::Id)));
    assert_eq!(
        effects,
        vec![Directive::Ref {
            session_ref: session("a8e2f3d1"),
        }]
    );
    assert_eq!(tracker.session_ref(), Some(&session("a8e2f3d1")));

    // The same conversation reported again is not news.
    assert_eq!(
        tracker.apply(Input::Hook(HookEdge::from_report(&first, RefKind::Id))),
        Vec::new()
    );

    // `/clear`: `SessionEnd{clear}` then `SessionStart{clear}`, new id, same
    // process, and the pane is still very much alive.
    let cleared = report(
        HookEvent::SessionStart,
        Some("c9a3c73b"),
        ReportScope::Parent,
    );
    let effects = tracker.apply(Input::Hook(HookEdge::from_report(&cleared, RefKind::Id)));
    assert_eq!(
        effects,
        vec![Directive::Ref {
            session_ref: session("c9a3c73b"),
        }]
    );
    assert_eq!(status(&effects), None, "a ref is identity, not state");
}

/// The reduction, on the payload shapes V01 recorded.
#[test]
fn from_report_tags_subagent_scope_and_takes_no_ref_from_it() {
    let parent = report(HookEvent::Stop, Some("a8e2f3d1"), ReportScope::Parent);
    let reduced = HookEdge::from_report(&parent, RefKind::Id);
    assert!(!reduced.subagent);
    assert_eq!(reduced.session_ref, Some(session("a8e2f3d1")));

    // A subagent payload carries the *parent's* session id, so reading one here
    // would let a subagent write the pane's resume ref — the same mistake as
    // letting it write the pane's state, one size down.
    let child = report(
        HookEvent::SubagentStop,
        Some("a8e2f3d1"),
        ReportScope::Subagent {
            agent_id: "a7d7604b5dfd35b1e".to_owned(),
            agent_type: None,
        },
    );
    let reduced = HookEdge::from_report(&child, RefKind::Id);
    assert!(reduced.subagent);
    assert_eq!(reduced.session_ref, None);

    // A `path` agent's ref is its transcript, not its id.
    let reduced = HookEdge::from_report(&parent, RefKind::Path);
    assert_eq!(
        reduced.session_ref,
        Some(SessionRef::new(RefKind::Path, "/home/u/.claude/a8e2f3d1.jsonl").unwrap())
    );

    // A report with nothing to identify identifies nothing.
    let bare = report(HookEvent::Stop, None, ReportScope::Parent);
    assert_eq!(HookEdge::from_report(&bare, RefKind::Id).session_ref, None);
}

/// A retired tracker publishes its exit and then nothing, ever — including the
/// dequeue and the timers it was holding.
#[test]
fn exit_publishes_once_dequeues_and_disarms_everything() {
    let mut tracker = agent(CoverageClass::Edges);
    tracker.apply(hook(HookEvent::PermissionRequest));
    tracker.apply(screen(AgentState::Idle));
    assert!(tracker.is_armed(Deadline::Confirmation));
    assert!(tracker.is_armed(Deadline::Staleness));

    let effects = tracker.apply(Input::Exited);
    assert_eq!(
        status(&effects),
        Some((
            Some(AgentState::Blocked),
            AgentState::Quiet,
            StatusCause::Exited
        ))
    );
    assert!(effects.contains(&Directive::Dequeue));
    assert!(effects.contains(&Directive::Disarm {
        deadline: Deadline::Confirmation,
    }));
    assert!(effects.contains(&Directive::Disarm {
        deadline: Deadline::Staleness,
    }));
    assert!(tracker.is_exited());

    for input in [
        hook(HookEvent::UserPromptSubmit),
        screen(AgentState::Working),
        Input::Deadline(Deadline::Staleness),
        Input::Exited,
    ] {
        assert_eq!(tracker.apply(input), Vec::new());
    }
}
