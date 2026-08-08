//! X06's other half: what the pane's own status now carries, read off the view.
//!
//! [`super::facts`] is about the *events*; this is about the two fields
//! `AgentSnapshot` grew, seen the way `session.state` and a `wait` predicate
//! see them — through the `StatusView` and `Core`'s mirror rather than through
//! a subscription.
//!
//! One claim holds them together, and it is the one D-M4-4 exists for: `since`
//! is an **observation**, so it moves when the pane moves and never otherwise.
//! A hook that re-announced itself, a screen verdict that agreed, a `/clear`
//! that replaced a conversation id and an adoption across a live upgrade are
//! all things that happen to a pane without moving it, and every one of them
//! would restart a rendered age if the stamp were taken per fold instead of
//! per transition.

use amx_core::Event;
use amx_core::agent::{AgentSnapshot, AgentState, StatusCause};
use amx_proto::control::agent::HookEvent;
use amx_server::actor::agent_hub::inherit::InheritedPane;

use crate::facts::dequeued;
use crate::fixtures::{
    FakePane, IMPATIENT_CLAUDE, Rig, kind, pane_id, report, screen, token, wait_for, workspace_id,
};
use crate::support::TempDir;

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
