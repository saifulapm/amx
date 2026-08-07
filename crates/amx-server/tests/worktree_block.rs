//! The workspace `worktree` block, across both surfaces that carry it
//! (D-M3-10).
//!
//! Membership, not machinery: the server learns no git. What it has to do is
//! remember, on two surfaces at once — `session.state` on the wire and
//! `session.json` on disk — because `amx work done` resolves a workspace by
//! branch and restore has to notice a tree that is no longer there. A block
//! that survived one surface and not the other would be a workspace that
//! forgets what it is over a restart, which is exactly the failure mode
//! UUID-keyed persistence exists to delete.
//!
//! W12 fills the verbs; this is the field's own contract, frozen ahead of them.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{Direction, Layout, PaneId, ShortNumber, Workspace, WorkspaceId, Worktree};
use amx_proto::control::session::WorkspaceState;
use amx_server::persist::snapshot::{decode, encode};
use amx_server::persist::{Snapshot, VERSION, WorkspaceSnapshot};
use serde_json::json;

fn pane() -> PaneId {
    "00000000-0000-0000-0000-0000000000a1".parse().unwrap()
}

fn other_pane() -> PaneId {
    "00000000-0000-0000-0000-0000000000a2".parse().unwrap()
}

fn workspace() -> WorkspaceId {
    "00000000-0000-0000-0000-0000000000b1".parse().unwrap()
}

fn membership() -> Worktree {
    Worktree {
        repo: "/home/s/amx".into(),
        branch: "reflow-fix".to_owned(),
        path: "/home/s/amx--reflow-fix".into(),
    }
}

fn layout() -> Layout {
    let mut layout = Layout::with_root(pane());
    layout
        .split(pane(), Direction::Right, other_pane(), 0.5)
        .expect("split the fixture layout");
    layout
}

#[test]
fn workspace_worktree_block_round_trips_snapshot_and_state() {
    // On disk. The version does not move for an additive optional field — the
    // R-M1-8 precedent — so this is a v1 file with a v1 reader.
    let saved = Snapshot {
        version: VERSION,
        focused_workspace: Some(workspace()),
        workspaces: vec![WorkspaceSnapshot {
            id: workspace(),
            short: ShortNumber::new(1),
            label: Some("reflow".to_owned()),
            layout: layout(),
            focus: Some(other_pane()),
            worktree: Some(membership()),
        }],
        panes: Vec::new(),
    };
    let read_back = decode(&encode(&saved).expect("encode")).expect("decode");
    assert_eq!(read_back, saved, "the block did not survive the snapshot");
    assert_eq!(
        read_back.workspaces[0].worktree.as_ref(),
        Some(&membership()),
        "and it is the same three facts, not a lossy shadow of them",
    );

    // On the wire. The same three facts, spelled the same way, because a
    // client reading state and a restore reading the file must agree about
    // what workspace this is.
    let reported = WorkspaceState {
        workspace: workspace(),
        short: ShortNumber::new(1),
        label: Some("reflow".to_owned()),
        layout: layout(),
        focus: Some(other_pane()),
        worktree: Some(membership()),
    };
    let bytes = serde_json::to_value(&reported).expect("encode");
    assert_eq!(
        bytes["worktree"],
        json!({
            "repo": "/home/s/amx",
            "branch": "reflow-fix",
            "path": "/home/s/amx--reflow-fix",
        }),
        "the wire spelling must match the file's, or two readers disagree",
    );
    let round_tripped: WorkspaceState = serde_json::from_value(bytes).expect("decode");
    assert_eq!(round_tripped, reported);
}

/// A workspace nobody created with `amx work` writes exactly the bytes it
/// always did, on both surfaces.
///
/// The other half of "additive": most workspaces have no worktree, and their
/// snapshot and their state row must be byte-identical to what a build without
/// this field produced. Otherwise the field is a wire change wearing an
/// optional's clothes.
#[test]
fn a_workspace_without_a_worktree_is_unchanged_on_both_surfaces() {
    let plain = WorkspaceSnapshot {
        id: workspace(),
        short: ShortNumber::new(1),
        label: None,
        layout: Layout::with_root(pane()),
        focus: Some(pane()),
        worktree: None,
    };
    let bytes = serde_json::to_value(&plain).expect("encode");
    assert!(
        bytes.get("worktree").is_none(),
        "an absent block must serialize to nothing at all: {bytes}",
    );

    let reported = WorkspaceState {
        workspace: workspace(),
        short: ShortNumber::new(1),
        label: None,
        layout: Layout::with_root(pane()),
        focus: Some(pane()),
        worktree: None,
    };
    let bytes = serde_json::to_value(&reported).expect("encode");
    assert!(
        bytes.get("worktree").is_none(),
        "and on the wire too: {bytes}"
    );

    // And a file written before the field existed reads as one without it,
    // rather than as a malformed snapshot.
    let older = json!({
        "version": VERSION,
        "workspaces": [{
            "id": workspace(),
            "short": 1,
            "layout": serde_json::to_value(Layout::with_root(pane())).expect("encode"),
        }],
        "panes": [],
    });
    let read_back =
        decode(&serde_json::to_vec(&older).expect("encode")).expect("a pre-M3 snapshot is v1");
    assert_eq!(read_back.workspaces[0].worktree, None);
}

/// `SessionState` carries the block through the live tree, not just through
/// serde.
///
/// Two surfaces agreeing on paper is not the same as one workspace carrying the
/// association while the server is running — which is what `amx work done`
/// reads and what capture writes into the snapshot. W12 sets it from the
/// worktree it just created; this pins that setting it and reading it back are
/// the same fact.
#[test]
fn a_live_workspace_carries_and_clears_its_membership() {
    let mut state = amx_core::SessionState::new();
    state
        .adopt_workspace(workspace(), Some("reflow".to_owned()), layout(), None)
        .expect("adopt the workspace");

    assert_eq!(
        state.workspace(workspace()).and_then(Workspace::worktree),
        None,
        "a workspace starts with no membership",
    );
    state
        .set_worktree(workspace(), Some(membership()))
        .expect("the workspace is here");
    assert_eq!(
        state.workspace(workspace()).and_then(Workspace::worktree),
        Some(&membership()),
    );

    // Cleared rather than removed when the tree vanishes: restore degrades such
    // a workspace to a plain one and reports the loss, and a workspace with no
    // membership is exactly what a plain one is.
    state
        .set_worktree(workspace(), None)
        .expect("the workspace is here");
    assert_eq!(
        state.workspace(workspace()).and_then(Workspace::worktree),
        None,
    );

    // And a workspace that is not here is a named error, not a silent no-op:
    // `work done` resolving the wrong id must fail loudly.
    assert!(
        state
            .set_worktree(WorkspaceId::new_v4(), Some(membership()))
            .is_err(),
    );
}
