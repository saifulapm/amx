//! V08: the `AgentHub` actor — what it ingests, what it costs, and how it
//! stops.
//!
//! The claims defended here are `docs/08-m2-plan.md` §3's, and each one is a
//! failure mode somebody has already shipped:
//!
//! - **A hook report is attributed, or it is dropped.** The token is a
//!   misattribution guard, not a security boundary (D-M2-4): a stale hook
//!   config in a nested session must report into the void rather than into the
//!   wrong pane's status. Dropping is silent and counted — a hook must never
//!   break or slow a turn.
//! - **Detection is scheduled by damage and coalesced per pane.** herdr runs a
//!   permanent 300–500 ms scan over every pane; here a burst of fifty damage
//!   batches costs two or three evaluations, and an idle pane costs none.
//! - **The `StatusView` is written before the event that announces it.** The
//!   reverse order hangs a wait forever, and the test that proves it races a
//!   real subscriber against a real hub.
//! - **The attention queue is block-ordered and outlives nothing.** Leaving
//!   `blocked` dequeues; so does the pane exiting, because a queue holding a
//!   dead pane sends `next-attention` somewhere that is not there.
//! - **Shutdown is receive-only.** After cancellation the hub publishes
//!   nothing, evaluates nothing and tells no sibling anything — the discipline
//!   that keeps it out of the undiagnosed drain wedge's way (R-M1-2), asserted
//!   the way `Persist`'s is.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::agent::{AgentState, StatusCause};
use amx_core::{Delivery, Event};
use amx_proto::control::agent::HookEvent;

// A test crate root's module directory is `tests/`, so the fixtures need their
// path spelled out to live beside their one suite rather than next to
// everybody's.
#[path = "agent_hub/facts.rs"]
mod facts;
#[path = "agent_hub/fixtures.rs"]
mod fixtures;
#[path = "agent_hub/scope.rs"]
mod scope;
#[path = "agent_hub/staleness.rs"]
mod staleness;
#[path = "agent_hub/stamps.rs"]
mod stamps;
mod support;

use fixtures::{
    FakePane, IMPATIENT_CLAUDE, Rig, agent_events, kind, pane_id, report, screen, token, wait_for,
    workspace_id,
};
use support::TempDir;

#[tokio::test]
async fn hook_report_with_valid_token_updates_status_and_publishes_once() {
    let root = TempDir::new("hook");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("hook");

    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified from its spawn identity",
    )
    .await;

    // Subscribed *after* identification, so the only transition in the window
    // is the one this report causes.
    let mut subscription = rig.ctx.bus.subscribe();
    let head = rig.ctx.bus.head();
    let reply = rig
        .report(report(
            pane.pane,
            &token,
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
    assert!(reply.accepted, "a report with the pane's own token is ours");
    rig.settle().await;

    let status = rig.view.get(pane.pane).expect("the pane is tracked");
    assert_eq!(status.state, AgentState::Blocked);
    assert_eq!(
        status.cause,
        StatusCause::Hook,
        "`PermissionRequest` is an entry edge and applies instantly (V01 §3 M3)",
    );
    assert_eq!(
        status.kind.as_ref(),
        Some(&kind("claude")),
        "the stanza's id, not the report's spelling of it",
    );
    // One transition, one event, and nothing else on the bus: the hub is the
    // only publisher of agent events (04 §2's rule, read per event kind —
    // `docs/09-m3-plan.md` D-M3-2). The block enqueues too, so the head moves
    // by exactly the two events this transition owes.
    assert_eq!(
        rig.ctx.bus.head(),
        head + 2,
        "one `agent_status` and one `attention_enqueued`, published once each",
    );
    let delivered = tokio::time::timeout(fixtures::PATIENCE, subscription.recv())
        .await
        .expect("the event is already buffered")
        .expect("the bus is live");
    assert!(
        matches!(
            delivered,
            Delivery::Event(envelope)
                if envelope.event == Event::AgentStatus {
                    pane: pane.pane,
                    from: Some(AgentState::Idle),
                    to: AgentState::Blocked,
                    cause: StatusCause::Hook,
                }
        ),
        "the status event says what moved and why",
    );

    // And the slow read model has it too, queue position included, which is
    // what `session.state` and the status line answer from.
    let mirrored = rig.spy.status_of(pane.pane).expect("Core was told");
    assert_eq!(mirrored.state, AgentState::Blocked);
    assert_eq!(mirrored.attention, Some(0));

    let outcome = rig.stop().await;
    assert_eq!((outcome.reports, outcome.dropped), (1, 0));
    pane.stop().await;
}

#[tokio::test]
async fn token_mismatch_is_dropped_and_counted() {
    let root = TempDir::new("token");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("real");

    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;
    let before = rig.view.get(pane.pane).expect("tracked").state;

    // A hook config left behind by a previous process for this pane id, or one
    // belonging to a nested session: the same shape, the wrong token.
    let stale = rig
        .report(report(
            pane.pane,
            &fixtures::token("stale"),
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
    // A report that claims `claude` from a source that is not `claude`'s hook
    // path — D-M2-7's allowlist at the first of its three gates.
    let mut foreign = report(pane.pane, &token, "claude", HookEvent::PermissionRequest);
    foreign.source = amx_core::agent::RefSource::for_agent(&kind("codex"));
    let foreign = rig.report(foreign).await;
    rig.settle().await;

    assert!(!stale.accepted, "a foreign token is not this pane's hook");
    assert!(
        !foreign.accepted,
        "a source may only speak for its own agent"
    );
    assert_eq!(
        rig.view.get(pane.pane).map(|status| status.state),
        Some(before),
        "neither report moved the pane it named",
    );

    let outcome = rig.stop().await;
    assert_eq!(
        (outcome.reports, outcome.dropped),
        (0, 2),
        "dropped and counted, never answered with an error a turn would surface",
    );
    pane.stop().await;
}

#[tokio::test]
async fn damage_driven_detection_coalesces_to_the_minimum_spacing() {
    let root = TempDir::new("coal");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));

    rig.started(&pane, &token("coal"), Some("claude")).await;
    // The real permission dialog, as Claude Code 2.1.224 painted it.
    pane.paint(&screen("claude-blocked-permission.txt")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;

    // A pane streaming an answer produces damage at frame rate. Fifty batches
    // must not cost fifty evaluations.
    let batches = 50;
    for _ in 0..batches {
        rig.damage(pane.pane);
    }
    rig.settle().await;
    let promptly = rig.probe.evaluations();
    assert!(
        (1..=2).contains(&promptly),
        "a burst evaluates once on arrival, not once per batch: {promptly}",
    );

    // The batches that arrived inside the spacing are owed exactly one
    // evaluation between them — not none, or the last frame of a burst would
    // never be read.
    wait_for(
        || rig.probe.evaluations() > promptly,
        "the coalesced batches were never evaluated",
    )
    .await;
    rig.settle().await;
    assert_eq!(
        rig.probe.evaluations(),
        promptly + 1,
        "{batches} batches cost {} evaluations and no more",
        promptly + 1,
    );

    let status = rig.view.get(pane.pane).expect("the pane is tracked");
    assert_eq!(status.state, AgentState::Blocked);
    assert_eq!(
        status.cause,
        StatusCause::Screen,
        "no hook said anything; the dialog on the grid did",
    );
    assert_eq!(
        rig.view.attention(),
        vec![pane.pane],
        "a screen-detected block joins the queue like any other",
    );

    rig.stop().await;
    pane.stop().await;
}

#[tokio::test]
async fn status_view_is_current_before_the_status_event_is_receivable() {
    let root = TempDir::new("order");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("order");

    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;

    // A waiter of exactly the shape `wait --until blocked` has: it subscribes
    // first, and reads *live state* the moment an event wakes it. It never
    // looks at the event's contents — that would make it an event predicate,
    // which 04 §2 forbids precisely because a transition inside a gap would
    // then hang it.
    let mut subscription = rig.ctx.bus.subscribe();
    let view = rig.view.clone();
    let target = pane.pane;
    let woken = tokio::spawn(async move {
        loop {
            match subscription.recv().await {
                Some(Delivery::Event(envelope)) if matches!(envelope.event, Event::AgentStatus { pane, .. } if pane == target) =>
                {
                    return view.get(target);
                }
                Some(_) => continue,
                None => return None,
            }
        }
    });

    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;

    let seen = tokio::time::timeout(fixtures::PATIENCE, woken)
        .await
        .expect("the waiter is woken by the hub's own event")
        .expect("the waiter task");
    assert_eq!(
        seen.map(|status| status.state),
        Some(AgentState::Blocked),
        "a waiter woken by agent_status must not read a view older than it",
    );

    rig.stop().await;
    pane.stop().await;
}

#[tokio::test]
async fn blocked_agents_enqueue_in_block_order_and_dequeue_on_unblock_and_exit() {
    let root = TempDir::new("queue");
    let rig = Rig::under(&root).start();
    let panes: Vec<FakePane> = (0..3)
        .map(|n| FakePane::start(&rig.ctx.bus, pane_id(n)))
        .collect();
    let token = token("queue");

    for pane in &panes {
        rig.started(pane, &token, Some("claude")).await;
    }
    wait_for(
        || rig.view.get(panes[2].pane).is_some(),
        "the panes were never identified",
    )
    .await;

    // Blocked in a deliberate order that is not the order they were created
    // in: the queue is ordered by block time (D-M2-8), not by anything else.
    for n in [1, 0, 2] {
        rig.report(report(
            panes[n].pane,
            &token,
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
    }
    rig.settle().await;
    assert_eq!(
        rig.view.attention(),
        vec![panes[1].pane, panes[0].pane, panes[2].pane],
        "block order, and the queue position each pane reads back",
    );
    assert_eq!(
        rig.view
            .get(panes[0].pane)
            .and_then(|status| status.attention),
        Some(1),
    );

    // Leaving `blocked` dequeues. An `edges` agent cannot leave a held state on
    // a hook *exit* — V01 measured every one of those silent — but an entry
    // edge asserting a different state is not an exit, and a turn resuming is
    // exactly that.
    rig.report(report(
        panes[1].pane,
        &token,
        "claude",
        HookEvent::UserPromptSubmit,
    ))
    .await;
    rig.settle().await;
    assert_eq!(rig.view.attention(), vec![panes[0].pane, panes[2].pane]);
    assert_eq!(
        rig.view.get(panes[1].pane).map(|status| status.state),
        Some(AgentState::Working),
    );

    // And a pane that exits leaves too: a queue holding a dead pane would send
    // `next-attention` somewhere that is not there.
    rig.publish(Event::PaneExited {
        pane: panes[0].pane,
        status: Some(0),
    });
    wait_for(
        || rig.view.attention() == vec![panes[2].pane],
        "an exited pane never left the attention queue",
    )
    .await;
    assert!(
        rig.view.get(panes[0].pane).is_none(),
        "and its tracker retired with it",
    );

    rig.stop().await;
    for pane in panes {
        pane.stop().await;
    }
}

#[tokio::test]
async fn agent_next_focuses_the_head_and_reports_empty_honestly() {
    let root = TempDir::new("next");
    let rig = Rig::under(&root).start();

    // An empty queue is an honest empty reply, never an error: a prefix key
    // that raised a dialog for "nothing is waiting" would be chrome (03 §4).
    let empty = rig.next_attention().await;
    assert_eq!(empty.pane, None);
    assert_eq!(empty.waiting, 0);
    assert!(rig.spy.seen().focused.is_empty(), "and nothing was focused");

    let panes: Vec<FakePane> = (0..2)
        .map(|n| FakePane::start(&rig.ctx.bus, pane_id(n)))
        .collect();
    let token = token("next");
    for pane in &panes {
        rig.publish(Event::PaneCreated {
            pane: pane.pane,
            workspace: workspace_id(),
        });
        rig.started(pane, &token, Some("claude")).await;
    }
    for pane in &panes {
        rig.report(report(
            pane.pane,
            &token,
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
    }
    rig.settle().await;

    let next = rig.next_attention().await;
    assert_eq!(next.pane, Some(panes[0].pane), "the head, in block order");
    assert_eq!(next.workspace, Some(workspace_id()));
    assert_eq!(next.waiting, 2, "this one included");
    wait_for(
        || rig.spy.seen().focused == vec![panes[0].pane],
        "Core was never asked to focus the head",
    )
    .await;

    rig.stop().await;
    for pane in panes {
        pane.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn idle_session_arms_no_timer() {
    let root = TempDir::new("idle");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));

    // A pane running a plain shell: tracked, unidentified, and owing the wheel
    // nothing at all.
    rig.started(&pane, &token("idle"), None).await;
    rig.settle().await;

    // Ten minutes of virtual time. With no deadline pending the timer branch
    // of the select is disabled outright, so there is nothing for the clock to
    // advance *to* — which is 03 §5's promise that a session of idle agents
    // costs zero wakeups, as opposed to herdr's 300–500 ms scan over every
    // pane, forever.
    tokio::time::advance(std::time::Duration::from_secs(600)).await;
    rig.settle().await;
    assert_eq!(rig.probe.wakeups(), 0, "an idle session woke the hub up");
    assert_eq!(rig.probe.evaluations(), 0, "and evaluated nothing");

    let outcome = rig.stop().await;
    assert_eq!(outcome.wakeups, 0);
    pane.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_identified_pane_arms_its_startup_grace_and_fires_once() {
    let root = TempDir::new("grace");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));

    // The other half of the claim above: the wheel is armed when — and only
    // when — something asked for it. `claude`'s stanza asks for a 3 s startup
    // grace, because a booting TUI's splash matches nonsense (V01 §6).
    rig.started(&pane, &token("grace"), Some("claude")).await;
    rig.settle().await;
    assert_eq!(rig.probe.wakeups(), 0, "not before its deadline");

    tokio::time::advance(std::time::Duration::from_secs(4)).await;
    rig.settle().await;
    assert_eq!(
        rig.probe.wakeups(),
        1,
        "one wakeup for one deadline, and none after it",
    );

    rig.stop().await;
    pane.stop().await;
}

#[tokio::test]
async fn shutdown_after_cancel_sends_no_sibling_request() {
    let root = TempDir::new("quiet");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("quiet");

    // Warm first, so both the mirror and the bus have a baseline that a
    // post-cancellation message would move.
    rig.started(&pane, &token, Some("claude")).await;
    pane.paint(&screen("claude-blocked-permission.txt")).await;
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;
    let before = rig.spy.seen();
    // Everything published from here on, kept rather than counted.
    //
    // The count this used to take — `bus.head()`, before and after, expected to
    // move by exactly the test's own two publishes — is not a measurement of
    // this actor. The pane is a publisher too: `PaneHost` publishes its own
    // `PaneDamage` off the parser's `frames.changed()`
    // (`actor/pane_host/actor.rs:86,132`), and `paint` returns when the
    // snapshot command answers, not when that wakeup has been folded. Under
    // load the second one lands *after* the hub has been joined and the head
    // is one higher than the arithmetic allows — a failure whose message
    // accuses the hub of publishing after cancellation, in a run where the hub
    // published nothing at all and was already gone.
    //
    // So the assertion below is about the events the hub is the only publisher
    // of (04 §2's rule, read per kind), which is the property this test exists
    // to defend and is not a hostage to what else shares the bus.
    let mut published = rig.ctx.bus.subscribe();
    let evaluations = rig.probe.evaluations();
    let transitions = rig.probe.transitions();
    assert!(!before.statuses.is_empty(), "the mirror was filled live");
    assert!(transitions > 0, "and so was the fast one");

    // Now cancel, and then do every tempting thing: damage that would schedule
    // an evaluation, a report that would move a status, a close that would
    // publish a dequeue, and a queue query that would ask `Core` to focus.
    rig.ctx.cancel.cancel();
    rig.settle().await;
    rig.damage(pane.pane);
    rig.publish(Event::PaneExited {
        pane: pane.pane,
        status: Some(0),
    });
    let refused = rig
        .report(report(pane.pane, &token, "claude", HookEvent::Stop))
        .await;
    assert!(
        !refused.accepted,
        "a report after cancellation is unaccepted, and silently so",
    );

    let spy = rig.spy.clone();
    let outcome = tokio::time::timeout(fixtures::PATIENCE, rig.stop())
        .await
        .expect("the hub returns promptly once its mailbox closes");
    let after = spy.seen();
    assert_eq!(
        (after.statuses.len(), after.focused.len(), after.others),
        (before.statuses.len(), 0, 0),
        "a cancelled hub tells Core nothing — Core is draining too (R-M1-2)",
    );
    let hub_events = agent_events(&mut published).await;
    assert!(
        hub_events.is_empty(),
        "a cancelled hub publishes nothing of its own: {hub_events:#?}",
    );
    assert_eq!(
        outcome.transitions, transitions,
        "and moves no status: the exit it saw published no `agent_status`",
    );
    assert_eq!(
        outcome.evaluations, evaluations,
        "and evaluates nothing: the panes are on their way down",
    );
    pane.stop().await;
}

/// The same discipline where the test above cannot reach: a wakeup that was
/// ready in the *same poll* as the cancellation.
///
/// The test above cancels and then settles, so the hub has already left its
/// loop before anything else arrives — which is the ordinary case and is why
/// it passes. It is not the only case. The select is deliberately not
/// `biased`, so when a delivery and the cancellation are both ready tokio picks
/// between them at random, and the discipline would hold only as often as the
/// coin came down right.
///
/// Forcing it is a matter of not yielding: `publish` and `cancel` are both
/// synchronous, so on a current-thread runtime the hub cannot run between them
/// and its next poll has both branches ready. The delivery is a `PaneExited`
/// for a blocked pane, which is the most expensive thing the bus can hand this
/// actor — `retire` publishes an `agent_status` and an `attention_dequeued` and
/// tells `Core` about both.
///
/// Twenty-four rounds because one proves nothing about a coin: a loop that
/// takes the wrong branch half the time survives a single round with
/// probability 1/2 and this run with probability 2⁻²⁴.
#[tokio::test]
async fn a_cancelled_hub_ignores_a_wakeup_that_was_ready_in_the_same_poll() {
    for round in 0..24 {
        let root = TempDir::new("samepoll");
        let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
        let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
        let token = token("samepoll");

        rig.started(&pane, &token, Some("claude")).await;
        rig.report(report(
            pane.pane,
            &token,
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
        rig.settle().await;
        assert_eq!(
            rig.view.get(pane.pane).map(|status| status.state),
            Some(AgentState::Blocked),
            "round {round}: the pane never blocked, so there is no race to lose",
        );
        let mut published = rig.ctx.bus.subscribe();
        let mirrored = rig.spy.seen().statuses.len();
        let transitions = rig.probe.transitions();

        // No `.await` between these two, which is the whole of the setup.
        rig.publish(Event::PaneExited {
            pane: pane.pane,
            status: Some(0),
        });
        rig.ctx.cancel.cancel();
        rig.settle().await;

        let spy = rig.spy.clone();
        let outcome = rig.stop().await;
        let hub_events = agent_events(&mut published).await;
        assert!(
            hub_events.is_empty(),
            "round {round}: a cancelled hub folded the exit it raced: {hub_events:#?}",
        );
        assert_eq!(
            spy.seen().statuses.len(),
            mirrored,
            "round {round}: and told Core about it",
        );
        assert_eq!(
            outcome.transitions, transitions,
            "round {round}: and moved a status on the way out",
        );
        pane.stop().await;
    }
}
