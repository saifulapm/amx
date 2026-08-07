//! U07 acceptance: `pane.rename` end to end, and the labels a snapshot keeps.
//!
//! The rename verb is driven over a real socket against a real `Gateway` and
//! `Core` — not at `Core`'s mailbox — because the value of this task is that a
//! *client* can name a pane and every other client sees it: the wire row, the
//! dispatch arm, the state mutation and the `session.state` field are four
//! separate pieces, and only the socket exercises all four at once.
//!
//! The snapshot half is deliberately narrower than its name suggests today.
//! `Core::restore` is U06's (`docs/07-m1-plan.md` §4) and does not exist on
//! this branch, so what is proven here is the leg U07 owns: the labels a
//! rename puts into `session.state` are exactly what the real snapshot codec
//! writes and reads back, keyed by the same UUIDs. The restart leg — a second
//! server picking that file up — is U06's `restore.rs` and U09's reboot test.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{Delivery, Event, PaneId, WorkspaceId};
use amx_proto::control::{pane as pane_proto, session};
use amx_server::persist::io::SyncAll;
use amx_server::persist::{PaneSnapshot, Snapshot, WorkspaceSnapshot, snapshot};
use serde_json::json;

mod support;

use support::{Server, result_of};

/// Ask for the session's state and decode it.
async fn state_of(client: &mut support::Client) -> session::StateReply {
    let response = client.request(90, "session.state", json!({})).await;
    serde_json::from_value(result_of(&response).clone()).expect("decode session.state")
}

/// The label `session.state` reports for `pane`, if any.
fn pane_label(state: &session::StateReply, pane: PaneId) -> Option<&str> {
    state
        .panes
        .iter()
        .find(|p| p.pane == pane)
        .expect("the pane is in the state reply")
        .label
        .as_deref()
}

/// The label `session.state` reports for `workspace`, if any.
fn workspace_label(state: &session::StateReply, workspace: WorkspaceId) -> Option<&str> {
    state
        .workspaces
        .iter()
        .find(|w| w.workspace == workspace)
        .expect("the workspace is in the state reply")
        .label
        .as_deref()
}

#[tokio::test]
async fn pane_rename_persists_across_the_wire_and_republishes_state() {
    let server = Server::start("rename").await;
    // Attaching seeds the session's first workspace and its root pane, so
    // there is a real pane — with a real shell behind it — to name.
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let before = state_of(&mut client).await;
    let pane = before.panes.first().expect("the seeded pane").pane;
    assert_eq!(pane_label(&before, pane), None, "a fresh pane has no label");

    let mut events = server.ctx.bus.subscribe();

    let response = client
        .request(1, "pane.rename", json!({ "pane": pane, "label": "editor" }))
        .await;
    let reply: pane_proto::RenameReply =
        serde_json::from_value(result_of(&response).clone()).expect("decode pane.rename");

    // The transition went on the bus, at the sequence the reply names: that
    // is what makes the rename visible to the Persist actor's dirtiness
    // filter and to every future event consumer, not just to the caller.
    let delivery = tokio::time::timeout(support::PATIENCE, events.recv())
        .await
        .expect("an event arrived")
        .expect("the bus is open");
    match delivery {
        Delivery::Event(envelope) => {
            assert_eq!(envelope.seq, reply.seq, "the reply names the event's seq");
            assert_eq!(
                envelope.event,
                Event::PaneRenamed {
                    pane,
                    label: "editor".to_owned(),
                },
            );
        }
        Delivery::Gap { from, to } => panic!("unexpected gap {from}..={to}"),
    }

    // And it is state, not a notification: a client that asks afterwards —
    // this one, or any other — is told the label.
    let after = state_of(&mut client).await;
    assert_eq!(pane_label(&after, pane), Some("editor"));

    let mut second = server.attach().await;
    assert_eq!(
        pane_label(&state_of(&mut second).await, pane),
        Some("editor"),
        "a client that was not there for the rename still sees the label",
    );

    // Renaming to the same label is a legal no-op: nothing to publish, and the
    // reply reports the sequence the label already held at.
    let repeat = client
        .request(2, "pane.rename", json!({ "pane": pane, "label": "editor" }))
        .await;
    let repeat: pane_proto::RenameReply =
        serde_json::from_value(result_of(&repeat).clone()).expect("decode pane.rename");
    assert_eq!(repeat.seq, server.ctx.bus.head());

    drop(client);
    drop(second);
    server.shutdown().await;
}

#[tokio::test]
async fn renaming_a_pane_that_does_not_exist_is_a_typed_refusal() {
    let server = Server::start("rename-gone").await;
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let response = client
        .request(
            1,
            "pane.rename",
            json!({ "pane": PaneId::new_v4(), "label": "ghost" }),
        )
        .await;
    match &response.outcome {
        amx_proto::RpcOutcome::Error(err) => {
            assert_eq!(err.code, amx_proto::RpcError::INVALID_PARAMS);
        }
        amx_proto::RpcOutcome::Result(value) => panic!("renaming a ghost succeeded: {value}"),
    }

    drop(client);
    server.shutdown().await;
}

#[tokio::test]
async fn workspace_and_pane_labels_survive_snapshot_and_restore() {
    let server = Server::start("labels").await;
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let state = state_of(&mut client).await;
    let workspace = state.workspaces.first().expect("the seeded workspace");
    let workspace_id = workspace.workspace;
    let pane = state.panes.first().expect("the seeded pane").pane;

    let _ = client
        .request(
            1,
            "workspace.rename",
            json!({ "workspace": workspace_id, "label": "work" }),
        )
        .await;
    let _ = client
        .request(2, "pane.rename", json!({ "pane": pane, "label": "editor" }))
        .await;

    let labelled = state_of(&mut client).await;
    assert_eq!(workspace_label(&labelled, workspace_id), Some("work"));
    assert_eq!(pane_label(&labelled, pane), Some("editor"));

    // The snapshot a capture assembles out of that state, through the real
    // codec and the real durable write (U02's), into the session's own state
    // directory.
    let captured = Snapshot {
        version: amx_server::persist::VERSION,
        focused_workspace: labelled.focused_workspace,
        workspaces: labelled
            .workspaces
            .iter()
            .map(|ws| WorkspaceSnapshot {
                id: ws.workspace,
                short: ws.short,
                label: ws.label.clone(),
                layout: ws.layout.clone(),
                focus: ws.focus,
                worktree: ws.worktree.clone(),
            })
            .collect(),
        panes: labelled
            .panes
            .iter()
            .map(|p| PaneSnapshot {
                id: p.pane,
                short: p.short,
                label: p.label.clone(),
                cwd: None,
                argv: None,
                agent: None,
            })
            .collect(),
    };
    snapshot::save(&server.ctx.state_dir, &captured, &SyncAll).expect("save the snapshot");

    let read_back = snapshot::load(&server.ctx.state_dir)
        .expect("load the snapshot")
        .expect("a snapshot was written");
    assert_eq!(read_back, captured, "the file round-trips exactly");

    // The identities are what join a restored label back to a restored pane:
    // labels keyed by UUID, not by position (04 §6).
    let restored_workspace = read_back
        .workspaces
        .iter()
        .find(|ws| ws.id == workspace_id)
        .expect("the workspace survived by UUID");
    assert_eq!(restored_workspace.label.as_deref(), Some("work"));
    let restored_pane = read_back
        .panes
        .iter()
        .find(|p| p.id == pane)
        .expect("the pane survived by UUID");
    assert_eq!(restored_pane.label.as_deref(), Some("editor"));

    drop(client);
    server.shutdown().await;
}
