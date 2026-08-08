//! The layout file, and the two translations either side of it (D-M3-11).
//!
//! Everything here is a value: a `session.state` reply projected into a file,
//! a file planned into the calls that rebuild it. No socket, no server, no
//! agent binary — which is what lets the agent path be tested at all, since a
//! real agent does not exist on a runner (R-M2-8) and the thing worth pinning
//! is the *plan* apply makes for one.
//!
//! Its sibling [`layout`](../layout.rs) drives the same code through the real
//! binary; the acceptance names split across the two by what they need.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::Path;

use amx::layout::build::{self, Step, Taken};
use amx::layout::schema;
use amx_core::agent::{AgentKind, AgentSnapshot, AgentState, RefKind, SessionRef, StatusCause};
use amx_core::{Direction, Layout, PaneId, RowId, ShortNumber, WorkspaceId};
use amx_proto::control::pane::SplitDirection;
use amx_proto::control::session::{PaneState, StateReply, WorkspaceState};

/// A `session.state` reply over one workspace.
fn reply(layout: Layout, panes: Vec<PaneState>) -> StateReply {
    StateReply {
        seq: 12,
        focused_workspace: None,
        workspaces: vec![WorkspaceState {
            workspace: WorkspaceId::new_v4(),
            short: ShortNumber::FIRST,
            label: Some("dev".to_owned()),
            layout,
            focus: None,
            worktree: None,
        }],
        panes,
        attention: Vec::new(),
        restore: None,
    }
}

/// One pane row, labelled, running `agent` if it runs one.
fn pane_row(pane: PaneId, label: &str, agent: Option<AgentSnapshot>) -> PaneState {
    PaneState {
        pane,
        short: ShortNumber::FIRST,
        label: Some(label.to_owned()),
        rows: 24,
        cols: 80,
        history_head: RowId::FIRST,
        history_floor: RowId::FIRST,
        agent,
    }
}

#[test]
fn agent_kinds_apply_but_session_refs_never_export() {
    // Export: a pane running a tracked agent, with a ref the hub captured.
    let pane = PaneId::new_v4();
    let secret = SessionRef::new(RefKind::Id, "conversation-7f3a").expect("a well-formed ref");
    let state = reply(
        Layout::with_root(pane),
        vec![pane_row(
            pane,
            "api",
            Some(AgentSnapshot {
                kind: Some(AgentKind::new("claude").expect("a well-formed kind")),
                state: AgentState::Idle,
                cause: StatusCause::Probe,
                transition_seq: 11,
                attention: None,
                session_ref: Some(secret),
            }),
        )],
    );
    let exported = build::project(&state)
        .expect("project the state")
        .to_string();

    assert!(
        exported.contains(r#"agent = "claude""#),
        "the kind is the shape, and it is exported:\n{exported}"
    );
    assert!(
        !exported.contains("conversation-7f3a") && !exported.contains("ref"),
        "a session ref is a conversation, not a shape, and never leaves:\n{exported}"
    );

    // Apply: the kind becomes an `agent.start`, in the slot the file gave it.
    let file = schema::parse(
        "version = 1\n\n[[workspace]]\n\n[[workspace.pane]]\n\n[[workspace.pane]]\nfrom = 0\n\
         direction = \"vertical\"\nagent = \"claude\"\nlabel = \"api\"\n",
    )
    .expect("the fixture parses");
    let steps = build::plan(&file, &Taken::default());
    assert!(
        matches!(
            steps.iter().find(|step| matches!(step, Step::StartAgent { .. })),
            Some(Step::StartAgent { kind, name, into, .. })
                if kind.as_str() == "claude" && name == "api" && *into == 2
        ),
        "the agent is started and named: {steps:?}"
    );
    // Started, then put where the file wanted it, then the placeholder closed:
    // `agent.start` picks its own slot, so the shape is restored around it.
    assert!(
        steps
            .iter()
            .any(|step| matches!(step, Step::Swap { pane: 2, with: 1 }))
            && steps
                .iter()
                .any(|step| matches!(step, Step::Close { pane: 1 })),
        "the started agent lands in the slot the layout gave it: {steps:?}"
    );

    // And there is no key for a ref, so a file carrying one is refused rather
    // than quietly ignored.
    let refused = schema::parse(
        "version = 1\n\n[[workspace]]\n\n[[workspace.pane]]\nsession_ref = \"conversation-7f3a\"\n",
    )
    .expect_err("a ref has no place in a layout");
    assert_eq!(
        refused.line,
        Some(6),
        "the refusal names the line: {refused}"
    );
    assert!(
        refused.message.starts_with("unknown field `session_ref`"),
        "and says what it objected to: {refused}"
    );
}

#[test]
fn a_ratio_survives_the_nudge_it_is_rebuilt_with() {
    // Apply reaches a ratio by nudging a fresh half-split, so the delta and
    // the direction have to agree with the file in both directions.
    for (direction, ratio, expected) in [
        (SplitDirection::Vertical, 0.7_f32, "right"),
        (SplitDirection::Vertical, 0.3, "left"),
        (SplitDirection::Horizontal, 0.7, "down"),
        (SplitDirection::Horizontal, 0.3, "up"),
    ] {
        let (way, delta) = build::nudge(direction, ratio);
        assert_eq!(
            serde_json::to_value(way).expect("a wire name"),
            serde_json::Value::String(expected.to_owned())
        );
        assert!(
            (delta - 0.2).abs() < 0.001,
            "{direction:?} {ratio}: {delta}"
        );
    }

    // And a tree that came from a resize renders the ratio it holds.
    let root = PaneId::new_v4();
    let mut layout = Layout::with_root(root);
    layout
        .split(root, Direction::Right, PaneId::new_v4(), 0.5)
        .expect("split");
    layout.resize(root, 0.12).expect("resize");
    let file = build::project(&reply(layout, Vec::new())).expect("project the state");
    assert!(
        file.to_string().contains("ratio = 0.620"),
        "{}",
        file.to_string()
    );
}

#[test]
fn a_cwd_reaches_every_pane_the_file_gives_one() {
    // `workspace.create` takes no cwd, so the first pane of a workspace is
    // made beside the root and the root closed under it. Everything else is
    // an ordinary split parameter.
    let dir = Path::new("/srv/app");
    let file = schema::parse(
        "version = 1\n\n[[workspace]]\n\n[[workspace.pane]]\ncwd = \"/srv/app\"\n\n\
         [[workspace.pane]]\nfrom = 0\ndirection = \"vertical\"\ncwd = \"/srv/app\"\n",
    )
    .expect("the fixture parses");
    let steps = build::plan(&file, &Taken::default());
    assert_eq!(
        steps,
        vec![
            Step::CreateWorkspace {
                label: None,
                workspace: 0,
                root: 0,
            },
            Step::Split {
                from: 0,
                into: 1,
                direction: SplitDirection::Vertical,
                cwd: Some(dir.to_path_buf()),
            },
            Step::Close { pane: 0 },
            Step::Split {
                from: 1,
                into: 2,
                direction: SplitDirection::Vertical,
                cwd: Some(dir.to_path_buf()),
            },
        ],
        "the root is replaced by the pane the file asked for"
    );
}
