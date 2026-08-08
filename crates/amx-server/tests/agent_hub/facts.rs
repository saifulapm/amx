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

use amx_core::agent::AgentWorkspace;
use amx_core::{Delivery, Event, PaneId, Subscription};
use amx_proto::control::agent::HookEvent;

use crate::fixtures::{self, FakePane, Rig, pane_id, report, token, wait_for, workspace_id};
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
pub async fn enqueued(subscription: &mut Subscription) -> Event {
    awaited(subscription, |event| {
        matches!(event, Event::AttentionEnqueued { .. })
    })
    .await
}

/// The next `attention_dequeued`.
pub async fn dequeued(subscription: &mut Subscription) -> Event {
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
