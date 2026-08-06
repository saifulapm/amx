//! Checked-in byte goldens for every control message (04 §4).
//!
//! Every method in the table gets one golden file holding an example request
//! and reply; the handshake and envelope shapes that sit outside the table
//! get one each. Regenerate after an intentional wire change with:
//!
//! ```text
//! AMX_UPDATE_GOLDENS=1 cargo test -p amx-proto --test goldens
//! ```
//!
//! Goldens are pretty-printed JSON, one message per file under
//! `tests/goldens/proto/` at the repository root, so a wire change shows up
//! as a readable diff rather than a blob.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use amx_core::{PaneId, SessionId, ShortNumber, WorkspaceId};
use amx_proto::control::{Call, pane, session, workspace};
use amx_proto::hello::{ClientInfo, Feature, Hello, Resume, ServerInfo, Welcome};
use amx_proto::rpc::{Notification, Request, RequestId, Response, RpcError};
use serde::Serialize;

fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/goldens/proto")
}

fn check_golden(name: &str, value: &impl Serialize) {
    let json = format!("{}\n", serde_json::to_string_pretty(value).unwrap());
    let path = goldens_dir().join(format!("{name}.json"));

    if std::env::var_os("AMX_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, &json).unwrap_or_else(|e| panic!("writing golden {path:?}: {e}"));
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("reading golden {path:?}: {e} (run with AMX_UPDATE_GOLDENS=1 to create it)")
    });
    assert_eq!(
        json, expected,
        "golden {name} does not match; run with AMX_UPDATE_GOLDENS=1 to update it after an intentional change"
    );
}

fn pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a1".parse().unwrap()
}

fn other_pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a2".parse().unwrap()
}

fn workspace_id() -> WorkspaceId {
    "00000000-0000-0000-0000-0000000000b1".parse().unwrap()
}

fn session_id() -> SessionId {
    "00000000-0000-0000-0000-0000000000c1".parse().unwrap()
}

/// Golden a method's request envelope alongside an example reply, so one file
/// documents the full round trip for that row of the table.
fn call_golden(name: &str, call: Call, reply: impl Serialize) {
    let request = Request::new(
        RequestId::from(1),
        call.method().wire_name(),
        Some(call.params().unwrap()),
    );
    check_golden(
        name,
        &serde_json::json!({ "request": request, "reply": reply }),
    );
}

#[test]
fn control_goldens_match() {
    check_golden(
        "hello",
        &Hello {
            proto: (1, 1),
            features: BTreeSet::from([Feature::GRID_STREAM, Feature::HISTORY_RANGES]),
            client: ClientInfo {
                name: "amx".into(),
                version: "0.1.0".into(),
                term: Some("xterm-ghostty".into()),
            },
            resume: Some(Resume {
                last_seq: 41,
                generations: vec![],
            }),
        },
    );

    check_golden(
        "welcome",
        &Welcome {
            proto: 1,
            features: BTreeSet::from([Feature::GRID_STREAM]),
            server: ServerInfo {
                name: "amxd".into(),
                version: "0.1.0".into(),
            },
            seq: 41,
            session: session_id(),
        },
    );

    check_golden(
        "notification",
        &Notification::new(
            "pane.exited",
            Some(serde_json::json!({"pane": pane_id(), "status": 0})),
        ),
    );

    check_golden(
        "response_error",
        &Response::err(
            RequestId::from(7),
            RpcError::method_not_found("pane.teleport"),
        ),
    );

    call_golden(
        "method_ping",
        Call::Ping(session::PingParams {}),
        session::PingReply {
            server: ServerInfo {
                name: "amxd".into(),
                version: "0.1.0".into(),
            },
            session: session_id(),
            seq: 41,
        },
    );

    call_golden(
        "method_workspace_create",
        Call::WorkspaceCreate(workspace::CreateParams {
            label: Some("scratch".into()),
            focus: true,
        }),
        workspace::CreateReply {
            workspace: workspace_id(),
            short: ShortNumber::new(1),
            seq: 41,
        },
    );

    call_golden(
        "method_workspace_rename",
        Call::WorkspaceRename(workspace::RenameParams {
            workspace: workspace_id(),
            label: "dev".into(),
        }),
        workspace::RenameReply { seq: 42 },
    );

    call_golden(
        "method_workspace_kill",
        Call::WorkspaceKill(workspace::KillParams {
            workspace: workspace_id(),
        }),
        workspace::KillReply {
            panes: vec![pane_id(), other_pane_id()],
            seq: 43,
        },
    );

    call_golden(
        "method_workspace_switch",
        Call::WorkspaceSwitch(workspace::SwitchParams {
            workspace: workspace_id(),
        }),
        workspace::SwitchReply {
            focused_pane: Some(pane_id()),
            seq: 44,
        },
    );

    call_golden(
        "method_pane_split",
        Call::PaneSplit(pane::SplitParams {
            pane: pane_id(),
            direction: pane::SplitDirection::Vertical,
            command: None,
            cwd: None,
        }),
        pane::SplitReply {
            pane: other_pane_id(),
            short: ShortNumber::new(2),
            seq: 45,
        },
    );

    call_golden(
        "method_pane_zoom",
        Call::PaneZoom(pane::ZoomParams { pane: pane_id() }),
        pane::ZoomReply {
            zoomed: true,
            seq: 46,
        },
    );

    call_golden(
        "method_pane_swap",
        Call::PaneSwap(pane::SwapParams {
            pane: pane_id(),
            with: other_pane_id(),
        }),
        pane::SwapReply { seq: 47 },
    );

    call_golden(
        "method_pane_move",
        Call::PaneMove(pane::MoveParams {
            pane: pane_id(),
            to: workspace_id(),
        }),
        pane::MoveReply { seq: 48 },
    );

    call_golden(
        "method_pane_close",
        Call::PaneClose(pane::CloseParams { pane: pane_id() }),
        pane::CloseReply { seq: 49 },
    );

    call_golden(
        "method_pane_focus",
        Call::PaneFocus(pane::FocusParams {
            workspace: workspace_id(),
            direction: pane::MoveDirection::Left,
        }),
        pane::FocusReply {
            pane: Some(pane_id()),
            seq: 50,
        },
    );

    call_golden(
        "method_pane_resize",
        Call::PaneResize(pane::ResizeParams {
            pane: pane_id(),
            // Exactly representable in an f32, so the golden stays readable
            // rather than picking up a float tail.
            direction: pane::MoveDirection::Right,
            delta: 0.0625,
        }),
        pane::ResizeReply {
            resized: true,
            seq: 51,
        },
    );
}
