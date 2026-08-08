//! X17: `agent.next` asked about one workspace.
//!
//! D15's scoped cycling exists for one workflow: a person clearing the blocks
//! in the project they are working on, without focus being thrown into another
//! project because something there blocked earlier. So the promise has two
//! halves and this file asserts both.
//!
//! **The queue stays global.** Fairness is the hub's, block-time order is the
//! hub's, and a caller asking about `api` must not be able to reorder `web`'s
//! share of the queue by asking. Every scenario below reads
//! `StatusView::attention` before and after and asserts the order did not
//! move — a scoped call selects, it does not mutate.
//!
//! **A scope answers about itself.** `waiting` is the count in the scope asked
//! about, which is the number a caller watches fall to zero, and an empty scope
//! is an honest empty reply rather than the next best pane from somewhere else.
//! The second half is the one that would make the feature worse than useless if
//! it fell through: a `next-attention` scoped to `api` that focused a `web`
//! pane because `api` had nothing left is exactly the yank the scope is for.

use amx_core::Event;
use amx_proto::control::agent::HookEvent;

use crate::fixtures::{
    FakePane, IMPATIENT_CLAUDE, Rig, pane_id, report, token, wait_for, workspace_id_n,
};
use crate::support::TempDir;

/// Three blocked panes, interleaved across two workspaces.
///
/// The interleaving is the point: workspace 2's only blocked pane sits in the
/// *middle* of the global queue, so a scoped call that answered with the head
/// and filtered afterwards, or that filtered and then took the global head,
/// would both be visible here.
async fn three_blocked(rig: &Rig) -> Vec<FakePane> {
    let panes: Vec<FakePane> = (0..3)
        .map(|n| FakePane::start(&rig.ctx.bus, pane_id(n)))
        .collect();
    let token = token("scope");
    for (n, pane) in panes.iter().enumerate() {
        rig.publish(Event::PaneCreated {
            pane: pane.pane,
            // 1, 2, 1: the second pane is the odd one out.
            workspace: workspace_id_n(if n == 1 { 2 } else { 1 }),
        });
        rig.started(pane, &token, Some("claude")).await;
    }
    // One at a time, each acknowledged before the next: the queue is in block
    // order and this is what makes that order a fact rather than a race.
    for pane in &panes {
        rig.report(report(
            pane.pane,
            &token,
            "claude",
            HookEvent::PermissionRequest,
        ))
        .await;
    }
    let want: Vec<_> = panes.iter().map(|pane| pane.pane).collect();
    wait_for(
        || rig.view.attention() == want,
        "the three panes never reached the queue in block order",
    )
    .await;
    panes
}

#[tokio::test]
async fn a_scoped_call_focuses_that_workspaces_oldest_block_and_counts_only_it() {
    let root = TempDir::new("scope");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let panes = three_blocked(&rig).await;
    let queue: Vec<_> = panes.iter().map(|pane| pane.pane).collect();

    let second = rig.next_attention_in(Some(workspace_id_n(2))).await;
    assert_eq!(
        second.pane,
        Some(panes[1].pane),
        "workspace 2's oldest block, not the global head",
    );
    assert_eq!(second.workspace, Some(workspace_id_n(2)));
    assert_eq!(
        second.waiting, 1,
        "how many are waiting *in that workspace*"
    );
    wait_for(
        || rig.spy.seen().focused == vec![panes[1].pane],
        "Core was never asked to focus the scoped answer",
    )
    .await;

    // The queue itself is untouched: same panes, same block order. A scoped
    // call selects an entry, it does not promote, demote or drop one.
    assert_eq!(
        rig.view.attention(),
        queue,
        "a scoped call reordered nothing"
    );

    let first = rig.next_attention_in(Some(workspace_id_n(1))).await;
    assert_eq!(
        first.pane,
        Some(panes[0].pane),
        "workspace 1's oldest block"
    );
    assert_eq!(first.waiting, 2, "both of workspace 1's, this one included");

    // And the unscoped call answers exactly as it did before the field
    // existed — the same head, and the whole queue's count.
    let global = rig.next_attention().await;
    assert_eq!(global.pane, Some(panes[0].pane), "the global head");
    assert_eq!(global.waiting, 3, "the whole queue, this one included");
    assert_eq!(rig.view.attention(), queue);

    rig.stop().await;
    for pane in panes {
        pane.stop().await;
    }
}

#[tokio::test]
async fn a_scope_with_nothing_blocked_is_an_honest_empty_and_never_crosses_out_of_itself() {
    let root = TempDir::new("empty-scope");
    let rig = Rig::under(&root).registry(IMPATIENT_CLAUDE).start();
    let panes = three_blocked(&rig).await;
    let queue: Vec<_> = panes.iter().map(|pane| pane.pane).collect();

    // A workspace this session has no blocked pane in — and, indistinguishably
    // from here, one it has never heard of. Neither is an error: the hub holds
    // no workspace registry to check an id against, and 03 §4 has no chrome for
    // "nothing is waiting" either way.
    let unknown = rig.next_attention_in(Some(workspace_id_n(9))).await;
    assert_eq!(unknown.pane, None);
    assert_eq!(unknown.workspace, None);
    assert_eq!(unknown.waiting, 0);
    assert!(
        rig.spy.seen().focused.is_empty(),
        "an empty scope focused something",
    );

    // Workspace 2's one blocked pane leaves the queue. Two panes are still
    // blocked and both are somebody else's project, so the honest answer to
    // "next in workspace 2" is that there is none — a call that fell through to
    // the global head here is the cross-project yank the scope exists to
    // prevent.
    rig.publish(Event::PaneExited {
        pane: panes[1].pane,
        status: Some(0),
    });
    wait_for(
        || rig.view.attention() == vec![panes[0].pane, panes[2].pane],
        "the exited pane never left the attention queue",
    )
    .await;

    let cleared = rig.next_attention_in(Some(workspace_id_n(2))).await;
    assert_eq!(
        cleared.pane, None,
        "a cleared scope stays in its own project"
    );
    assert_eq!(cleared.waiting, 0);
    assert!(
        rig.spy.seen().focused.is_empty(),
        "a cleared scope focused a pane in another workspace",
    );

    // The rest of the queue is exactly where it was, still waiting for whoever
    // asks about *their* project.
    let global = rig.next_attention().await;
    assert_eq!(global.pane, Some(panes[0].pane));
    assert_eq!(global.waiting, 2);
    assert_eq!(rig.view.attention(), vec![queue[0], queue[2]]);

    rig.stop().await;
    for pane in panes {
        pane.stop().await;
    }
}
