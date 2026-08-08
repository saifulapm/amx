//! Short numbers over a real socket: what a user types, and what it names
//! (04 §6, DR-6).
//!
//! Driven through `Gateway` and `Core` rather than at `Core`'s mailbox,
//! because the number is a *reply field* and a `session.state` field before it
//! is anything else: the assignment, the release and the two projections are
//! separate pieces and only the socket exercises all of them at once.
//!
//! The reuse assertions are what the mapping is for. `Core` ran a monotonic
//! stand-in until this landed, so a session that had opened and closed forty
//! panes over a day offered `37`–`40` and never `1` again.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::PaneId;
use amx_proto::control::{pane as pane_proto, session, workspace as workspace_proto};
use serde_json::json;

mod support;

use support::{Server, result_of};

/// Ask for the session's state and decode it.
async fn state_of(client: &mut support::Client) -> session::StateReply {
    let response = client.request(90, "session.state", json!({})).await;
    serde_json::from_value(result_of(&response).clone()).expect("decode session.state")
}

/// Every pane's number, lowest first.
fn pane_shorts(state: &session::StateReply) -> Vec<u32> {
    let mut shorts: Vec<u32> = state.panes.iter().map(|pane| pane.short.get()).collect();
    shorts.sort_unstable();
    shorts
}

/// Split `pane`, and answer with the new pane and the number it was given.
async fn split(client: &mut support::Client, pane: PaneId) -> (PaneId, u32) {
    let response = client
        .request(
            10,
            "pane.split",
            json!({ "pane": pane.to_string(), "direction": "vertical" }),
        )
        .await;
    let reply: pane_proto::SplitReply =
        serde_json::from_value(result_of(&response).clone()).expect("decode pane.split");
    (reply.pane, reply.short.get())
}

#[tokio::test]
async fn a_closed_pane_gives_its_number_back_to_the_next_split() {
    let server = Server::start("short-reuse").await;
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let root = state_of(&mut client)
        .await
        .panes
        .first()
        .expect("the seeded pane")
        .pane;
    let (second, second_short) = split(&mut client, root).await;
    let (_third, third_short) = split(&mut client, root).await;
    assert_eq!(
        (second_short, third_short),
        (2, 3),
        "the seeded pane holds 1, so the two splits take 2 and 3",
    );

    let closed = client
        .request(11, "pane.close", json!({ "pane": second.to_string() }))
        .await;
    let _: pane_proto::CloseReply =
        serde_json::from_value(result_of(&closed).clone()).expect("decode pane.close");

    let (fourth, fourth_short) = split(&mut client, root).await;
    assert_ne!(fourth, second, "a fresh pane, with the departed pane's number");
    assert_eq!(
        fourth_short, 2,
        "the closed pane's number is the lowest free one, so the next split takes it",
    );
    assert_eq!(
        pane_shorts(&state_of(&mut client).await),
        vec![1, 2, 3],
        "and no two panes hold one number",
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_killed_workspace_gives_back_its_number_and_its_panes_numbers() {
    let server = Server::start("short-kill").await;
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let seeded = state_of(&mut client).await;
    let first_workspace = seeded.workspaces.first().expect("the seeded workspace");
    assert_eq!(first_workspace.short.get(), 1);
    let workspace = first_workspace.workspace;
    let root = seeded.panes.first().expect("the seeded pane").pane;
    let _ = split(&mut client, root).await;

    let killed = client
        .request(
            20,
            "workspace.kill",
            json!({ "workspace": workspace.to_string() }),
        )
        .await;
    let killed: workspace_proto::KillReply =
        serde_json::from_value(result_of(&killed).clone()).expect("decode workspace.kill");
    assert_eq!(killed.panes.len(), 2, "the workspace took both panes with it");

    let created = client.request(21, "workspace.create", json!({})).await;
    let created: workspace_proto::CreateReply =
        serde_json::from_value(result_of(&created).clone()).expect("decode workspace.create");
    assert_eq!(
        created.short.get(),
        1,
        "the killed workspace's number is free again",
    );

    let state = state_of(&mut client).await;
    assert_eq!(
        state.workspaces.len(),
        1,
        "one workspace, so one workspace number",
    );
    assert_eq!(
        pane_shorts(&state),
        vec![1],
        "and its root pane took the lowest free pane number, both of the old ones \
         having gone with the workspace",
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_number_is_never_held_by_two_panes_at_once() {
    // The property behind both tests above, stated on its own: reuse is only
    // ever reuse of a number nobody holds. A release that ran while the pane
    // was still in the layout would show up here as a duplicate.
    let server = Server::start("short-unique").await;
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let root = state_of(&mut client)
        .await
        .panes
        .first()
        .expect("the seeded pane")
        .pane;
    let mut made = Vec::new();
    for _ in 0..4 {
        made.push(split(&mut client, root).await);
    }

    // Close two out of the middle, then refill: the numbers handed back are
    // exactly the two that were freed, in ascending order.
    for (pane, _) in &made[..2] {
        let closed = client
            .request(30, "pane.close", json!({ "pane": pane.to_string() }))
            .await;
        assert!(result_of(&closed).is_object(), "pane.close answered");
    }
    let refilled: Vec<u32> = vec![
        split(&mut client, root).await.1,
        split(&mut client, root).await.1,
    ];
    assert_eq!(refilled, vec![2, 3], "the two freed numbers, lowest first");

    let shorts = pane_shorts(&state_of(&mut client).await);
    let mut unique = shorts.clone();
    unique.dedup();
    assert_eq!(shorts, unique, "every pane holds a number of its own");
    assert_eq!(shorts, vec![1, 2, 3, 4, 5]);

    server.shutdown().await;
}
