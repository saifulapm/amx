//! X06: what the hub can say about a pane besides its state.
//!
//! Three facts, one actor, and one rule holding them together — nothing here is
//! asked of a sibling. `docs/11-m4-plan.md` D-M4-6 is explicit that the hub may
//! not query `Core` for a label ("parking on a sibling is what its shutdown
//! discipline forbids"), so every name in an attention event was folded off the
//! bus the hub already reads, or handed over by an import that publishes
//! nothing.
//!
//! What each scenario is defending:
//!
//! - **A notifier needs no follow-up query.** D15's `attention_enqueued` says
//!   `api/backend blocked (permission_dialog) 4m` from the event alone. Four
//!   optional fields, and the reason each is optional is that a fact the hub
//!   has not been told is absent rather than invented.
//! - **`since` is an observation.** It moves when the pane moves and at no
//!   other time — not on a re-assertion, not on a re-evaluation that agreed,
//!   and not on a handoff, where the exporter's instant crosses intact because
//!   an agent blocked all night has been blocked all night (R-M4-4).
//! - **`reason` is the shipped rule's own name.** The assertion below is
//!   `permission_dialog` because that is the string in
//!   `crates/amx-server/assets/manifests/claude.toml`, matched against a real
//!   recorded Claude Code screen — not a vocabulary this suite agreed with the
//!   hub.

use amx_core::agent::{AgentSnapshot, AgentState, AgentWorkspace, StatusCause};
use amx_core::{Delivery, Event, PaneId, Subscription};
use amx_proto::control::agent::HookEvent;
use amx_server::actor::agent_hub::inherit::InheritedPane;

use crate::fixtures::{
    self, FakePane, IMPATIENT_CLAUDE, Rig, kind, pane_id, report, screen, token, wait_for,
    workspace_id,
};
use crate::support::TempDir;

/// Read `subscription` until an event `want` accepts, or fail at [`PATIENCE`].
///
/// A deadline and never a nap: a fixed wait would be a wall clock against the
/// machine, which is exactly the shape X04 spent a task removing from four
/// suites.
///
/// [`PATIENCE`]: fixtures::PATIENCE
async fn awaited(subscription: &mut Subscription, want: fn(&Event) -> bool) -> Event {
    let deadline = tokio::time::Instant::now() + fixtures::PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let delivery = tokio::time::timeout(left, subscription.recv())
            .await
            .expect("the event never arrived")
            .expect("the bus is live");
        if let Delivery::Event(envelope) = delivery
            && want(&envelope.event)
        {
            return envelope.event;
        }
    }
}

/// The next `attention_enqueued`.
async fn enqueued(subscription: &mut Subscription) -> Event {
    awaited(subscription, |event| {
        matches!(event, Event::AttentionEnqueued { .. })
    })
    .await
}

/// The next `attention_dequeued`.
async fn dequeued(subscription: &mut Subscription) -> Event {
    awaited(subscription, |event| {
        matches!(event, Event::AttentionDequeued { .. })
    })
    .await
}

/// D15's requirement, literally: a subscriber renders `api/backend blocked
/// (PermissionRequest)` with no follow-up call.
#[tokio::test]
async fn an_attention_event_carries_the_whole_identity_block() {
    let root = TempDir::new("ident");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("ident");

    // The three bus facts the hub folds, in the order a live session produces
    // them: the pane's workspace, then the labels a person gave both.
    rig.publish(Event::PaneCreated {
        pane: pane.pane,
        workspace: workspace_id(),
    });
    rig.publish(Event::WorkspaceRenamed {
        workspace: workspace_id(),
        label: "api".to_owned(),
    });
    rig.publish(Event::PaneRenamed {
        pane: pane.pane,
        label: "backend".to_owned(),
    });
    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;

    let mut subscription = rig.ctx.bus.subscribe();
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;

    let Event::AttentionEnqueued {
        pane: blocked,
        workspace,
        name,
        reason,
        since,
    } = enqueued(&mut subscription).await
    else {
        unreachable!("matched above")
    };
    assert_eq!(blocked, pane.pane);
    assert_eq!(
        workspace.as_ref(),
        Some(&AgentWorkspace {
            id: workspace_id(),
            name: Some("api".to_owned()),
        }),
        "the id a consumer acts on, and the label a person reads",
    );
    assert_eq!(
        name.as_deref(),
        Some("backend"),
        "the agent's name is the pane's label (D-M2-9)"
    );
    assert_eq!(reason.as_deref(), Some("PermissionRequest"));
    let blocked_at = since.expect("the hub watched the pane enter blocked");

    // The dequeue carries the same block, so a notification can be matched to
    // the one it clears — with the *new* state's provenance, which is what
    // ended it.
    let mut subscription = rig.ctx.bus.subscribe();
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::UserPromptSubmit,
    ))
    .await;
    rig.settle().await;

    let Event::AttentionDequeued {
        workspace,
        name,
        reason,
        since,
        ..
    } = dequeued(&mut subscription).await
    else {
        unreachable!("matched above")
    };
    assert_eq!(
        workspace
            .as_ref()
            .and_then(|workspace| workspace.name.as_deref()),
        Some("api"),
    );
    assert_eq!(name.as_deref(), Some("backend"));
    assert_eq!(reason.as_deref(), Some("UserPromptSubmit"));
    assert!(
        since.is_some_and(|left| left >= blocked_at),
        "the resumed turn started no earlier than the block it ended",
    );

    rig.stop().await;
    pane.stop().await;
}

/// A label the hub was never told is absent, and the event is still usable.
///
/// The absence is the design (D-M4-6): the alternative to "no name" is asking
/// `Core`, and the hub does not ask siblings for anything.
#[tokio::test]
async fn an_unnamed_pane_reports_absences_rather_than_inventions() {
    let root = TempDir::new("bare");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("bare");

    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;

    let mut subscription = rig.ctx.bus.subscribe();
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;

    let Event::AttentionEnqueued {
        workspace,
        name,
        reason,
        ..
    } = enqueued(&mut subscription).await
    else {
        unreachable!("matched above")
    };
    assert_eq!(workspace, None, "no PaneCreated reached this hub");
    assert_eq!(name, None, "and nobody has named the pane");
    assert_eq!(
        reason.as_deref(),
        Some("PermissionRequest"),
        "the reason is the hub's own knowledge and is there either way",
    );

    rig.stop().await;
    pane.stop().await;
}

/// `since` marks the edge, and holds through everything that is not one.
#[tokio::test]
async fn since_moves_on_a_transition_and_on_nothing_else() {
    let root = TempDir::new("since");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("since");

    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;

    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;
    let blocked = rig.view.get(pane.pane).expect("tracked");
    assert_eq!(blocked.state, AgentState::Blocked);
    let entered = blocked.since.expect("the entry edge was observed here");

    // The same edge again: a permission dialog that re-announces itself
    // corroborates the block. The pane has not moved, so neither has the
    // instant it moved at — a status line showing `4m` must not restart at
    // `0m` because a hook repeated itself.
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;
    let held = rig.view.get(pane.pane).expect("tracked");
    assert_eq!(
        held.since,
        Some(entered),
        "a re-assertion is not a transition"
    );
    assert_eq!(held.reason.as_deref(), Some("PermissionRequest"));

    // The sharper version of the same claim, and the one a view read can
    // actually see: a `/clear` mid-block mints a new conversation id, which is
    // a fact about the pane worth writing and *not* a transition. The fold
    // writes both read models for it — so if `since` were stamped per fold
    // rather than per transition, this is where the block's age would silently
    // restart. It is the same guard `transition_seq` has carried since M2.
    let mut cleared = report(pane.pane, &token, "claude", HookEvent::SessionStart);
    cleared.session_id = Some("6f1d2a70-0000-4000-8000-00000000beef".to_owned());
    rig.report(cleared).await;
    rig.settle().await;
    let refreshed = rig.view.get(pane.pane).expect("tracked");
    assert_eq!(
        refreshed
            .session_ref
            .as_ref()
            .map(|session| session.value()),
        Some("6f1d2a70-0000-4000-8000-00000000beef"),
        "the new conversation was written, so this fold did reach the view",
    );
    assert_eq!(
        refreshed.since,
        Some(entered),
        "and the block is still as old as it was",
    );
    assert_eq!(refreshed.transition_seq, held.transition_seq);

    // A real move restamps it, and the two halves stay in step.
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::UserPromptSubmit,
    ))
    .await;
    rig.settle().await;
    let working = rig.view.get(pane.pane).expect("tracked");
    assert_eq!(working.state, AgentState::Working);
    assert!(
        working.since.is_some_and(|moved| moved >= entered),
        "the transition's own instant, not the block's",
    );
    assert_eq!(working.reason.as_deref(), Some("UserPromptSubmit"));

    // And `Core`'s mirror carries both, because `session.state` and the status
    // line answer from it and must not need a second call for the breakdown.
    let mirrored = rig.spy.status_of(pane.pane).expect("Core was told");
    assert_eq!(mirrored.since, working.since);
    assert_eq!(mirrored.reason, working.reason);

    rig.stop().await;
    pane.stop().await;
}

/// The reason on the wire is the string in the shipped manifest.
///
/// A real recorded permission dialog, matched by the real `claude.toml`. If
/// somebody renames that rule this fails, which is the point: the wire carries
/// the detector's identifier, so the identifier is the contract (D-M4-3).
#[tokio::test]
async fn a_screen_detected_block_is_named_by_the_shipped_rule() {
    let root = TempDir::new("rule");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));

    rig.started(&pane, &token("rule"), Some("claude")).await;
    pane.paint(&screen("claude-blocked-permission.txt")).await;
    wait_for(
        || {
            rig.view
                .get(pane.pane)
                .is_some_and(|status| status.state == AgentState::Blocked)
        },
        "the dialog on the grid never blocked the pane",
    )
    .await;

    let status = rig.view.get(pane.pane).expect("tracked");
    assert_eq!(status.cause, StatusCause::Screen);
    assert_eq!(
        status.reason.as_deref(),
        Some("permission_dialog"),
        "the rule's own name, straight from assets/manifests/claude.toml",
    );

    rig.stop().await;
    pane.stop().await;
}

/// R-M4-4's handoff half: an inherited pane is named, and its age is the
/// exporter's.
///
/// The import path publishes nothing at all — the swap is invisible on the bus
/// (`docs/09-m3-plan.md` §4) — so a names mirror fed only by `PaneRenamed`
/// would leave every pane that crossed an upgrade anonymous, and a `since`
/// stamped on arrival would tell the user an agent blocked all night had been
/// waiting four seconds.
#[tokio::test]
async fn an_inherited_pane_keeps_its_name_and_the_exporters_instant() {
    let root = TempDir::new("inherit");
    let pane_id = pane_id(0);
    // Long enough ago that a hub restamping it could not possibly produce this
    // number: 2025-08-08, in epoch milliseconds.
    let blocked_at = 1_754_650_000_000;
    let carried = AgentSnapshot {
        kind: Some(kind("claude")),
        state: AgentState::Blocked,
        cause: StatusCause::Screen,
        transition_seq: 41,
        attention: Some(0),
        session_ref: None,
        reason: Some("permission_dialog".to_owned()),
        since: Some(blocked_at),
    };
    let rig = Rig::under(&root)
        .registry(IMPATIENT_CLAUDE)
        .inherit(
            vec![InheritedPane {
                pane: pane_id,
                workspace: Some(workspace_id()),
                label: Some("backend".to_owned()),
                workspace_label: Some("api".to_owned()),
                status: Some(carried.clone()),
            }],
            vec![pane_id],
        )
        .start();

    // The view carries the exporter's status before any pane arrives, which is
    // what a `wait --until blocked` reconnecting mid-swap reads.
    let seeded = rig.view.get(pane_id).expect("inherit wrote the view");
    assert_eq!(seeded.since, Some(blocked_at));
    assert_eq!(seeded.reason.as_deref(), Some("permission_dialog"));

    let pane = FakePane::start(&rig.ctx.bus, pane_id);
    rig.started(&pane, &token("inherit"), None).await;
    rig.settle().await;
    // A fact about the pane that is *not* a transition, so the fold writes both
    // read models without moving anything: `/clear` mid-block mints a new
    // conversation id. This is the sharp version of "since moves only on a
    // transition" — the exporter's instant is from last August, so a stamp
    // taken per fold instead of per transition shows up as a four-hundred-day
    // difference rather than as a millisecond nobody can measure.
    let mut cleared = report(
        pane_id,
        &token("inherit"),
        "claude",
        HookEvent::SessionStart,
    );
    cleared.session_id = Some("6f1d2a70-0000-4000-8000-00000000beef".to_owned());
    rig.report(cleared).await;
    rig.settle().await;
    let adopted = rig.view.get(pane_id).expect("tracked");
    assert_eq!(
        adopted.session_ref.as_ref().map(|session| session.value()),
        Some("6f1d2a70-0000-4000-8000-00000000beef"),
        "the new conversation was written, so this fold did reach the view",
    );
    assert_eq!(
        adopted.since,
        Some(blocked_at),
        "neither the adoption nor a ref refresh is a transition",
    );
    assert_eq!(adopted.reason.as_deref(), Some("permission_dialog"));

    // And the labels are there without a single rename event on this bus.
    let mut subscription = rig.ctx.bus.subscribe();
    rig.publish(Event::PaneExited {
        pane: pane_id,
        status: Some(0),
    });
    wait_for(
        || rig.view.get(pane_id).is_none(),
        "the exited pane never retired",
    )
    .await;

    let Event::AttentionDequeued {
        workspace, name, ..
    } = dequeued(&mut subscription).await
    else {
        unreachable!("matched above")
    };
    assert_eq!(name.as_deref(), Some("backend"));
    assert_eq!(
        workspace
            .as_ref()
            .and_then(|workspace| workspace.name.as_deref()),
        Some("api"),
        "seeded by the import, since the import publishes no rename to fold",
    );

    rig.stop().await;
    pane.stop().await;
}

/// A rename after the pane is already tracked moves the name with it.
#[tokio::test]
async fn a_rename_moves_the_name_the_next_event_carries() {
    let root = TempDir::new("rename");
    let rig = Rig::under(&root).start();
    let pane = FakePane::start(&rig.ctx.bus, pane_id(0));
    let token = token("rename");

    rig.publish(Event::PaneRenamed {
        pane: pane.pane,
        label: "backend".to_owned(),
    });
    rig.started(&pane, &token, Some("claude")).await;
    wait_for(
        || rig.view.get(pane.pane).is_some(),
        "the pane was never identified",
    )
    .await;
    rig.publish(Event::PaneRenamed {
        pane: pane.pane,
        label: "api-backend".to_owned(),
    });
    rig.settle().await;

    let mut subscription = rig.ctx.bus.subscribe();
    rig.report(report(
        pane.pane,
        &token,
        "claude",
        HookEvent::PermissionRequest,
    ))
    .await;
    rig.settle().await;

    let Event::AttentionEnqueued { name, .. } = enqueued(&mut subscription).await else {
        unreachable!("matched above")
    };
    assert_eq!(name.as_deref(), Some("api-backend"));

    rig.stop().await;
    pane.stop().await;
}

/// Nothing above changed what the queue is or the order it keeps.
#[tokio::test]
async fn the_queue_is_still_the_queue() {
    let root = TempDir::new("order");
    let rig = Rig::under(&root).start();
    let panes: Vec<FakePane> = (0..2)
        .map(|n| FakePane::start(&rig.ctx.bus, pane_id(n)))
        .collect();
    let token = token("order");

    for pane in &panes {
        rig.started(pane, &token, Some("claude")).await;
    }
    wait_for(
        || rig.view.get(panes[1].pane).is_some(),
        "the panes were never identified",
    )
    .await;
    for n in [1, 0] {
        rig.report(report(
            panes[n].pane,
            &token,
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
    }
    rig.settle().await;

    let queue: Vec<PaneId> = rig.view.attention();
    assert_eq!(
        queue,
        vec![panes[1].pane, panes[0].pane],
        "block order, unchanged by anything the identity block added",
    );

    rig.stop().await;
    for pane in panes {
        pane.stop().await;
    }
}
