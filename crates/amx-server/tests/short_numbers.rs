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

use amx_core::agent::{AgentKind, AgentSnapshot, AgentState};
use amx_core::{PaneId, RowId, ShortNumber};
use amx_proto::control::pane::PaneTarget;
use amx_proto::control::{pane as pane_proto, session, workspace as workspace_proto};
use amx_server::agent::address::{self, AddressError, Scope};
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

/// A pane as `session.state` would report it, for the resolution rules — the
/// module they live in is a pure function over exactly this list.
fn pane_state(pane: PaneId, short: u32, label: Option<&str>, agent: bool) -> session::PaneState {
    session::PaneState {
        pane,
        short: ShortNumber::new(short),
        label: label.map(str::to_owned),
        cwd: None,
        rows: 24,
        cols: 80,
        history_head: RowId::from_raw(0),
        history_floor: RowId::from_raw(0),
        agent: agent.then(|| AgentSnapshot {
            kind: Some(AgentKind::new("claude").expect("a valid kind")),
            state: AgentState::Quiet,
            ..AgentSnapshot::unidentified(1)
        }),
    }
}

#[test]
fn a_short_number_resolves_before_a_label_that_spells_one() {
    let (numbered, mischief) = (PaneId::new_v4(), PaneId::new_v4());
    let panes = vec![
        pane_state(numbered, 2, None, true),
        pane_state(mischief, 3, Some("2"), true),
    ];

    assert_eq!(
        address::resolve(&PaneTarget::new("2"), &panes, Scope::Agent),
        Ok(numbered),
        "the pane that *is* 2 wins over the pane merely called 2",
    );
    assert_eq!(
        address::resolve(&PaneTarget::new("3"), &panes, Scope::Agent),
        Ok(mischief),
        "and the number the label was hiding behind still names its own pane",
    );
}

#[test]
fn a_number_no_pane_holds_says_so_rather_than_looking_for_a_label() {
    let panes = vec![pane_state(PaneId::new_v4(), 1, Some("7"), true)];
    let err = address::resolve(&PaneTarget::new("7"), &panes, Scope::Agent)
        .expect_err("no pane is numbered 7");
    assert_eq!(
        err,
        AddressError::UnknownNumber {
            number: ShortNumber::new(7),
        },
        "a digits-only target is a number whatever is in the tree, or it would \
         mean one thing today and another tomorrow",
    );
    assert!(err.to_string().contains('7'), "{err}");
}

#[test]
fn a_numbered_pane_outside_the_scope_names_itself_in_the_refusal() {
    // The agent verbs' narrower rule, reached by number: the pane exists and
    // the caller may still not drive it as an agent, and the error says which
    // pane it was rather than "no pane is numbered 2".
    let shell = PaneId::new_v4();
    let panes = vec![pane_state(shell, 2, None, false)];

    assert_eq!(
        address::resolve(&PaneTarget::new("2"), &panes, Scope::AnyPane),
        Ok(shell),
        "the driving verbs take any pane, by number as by label",
    );
    assert_eq!(
        address::resolve(&PaneTarget::new("2"), &panes, Scope::Agent),
        Err(AddressError::NoAgent {
            name: "2".to_owned(),
            panes: vec![shell],
        }),
    );
}

#[test]
fn a_new_agent_cannot_be_named_after_a_short_number() {
    // The same argument the UUID-shaped name is refused on: rule 2 is checked
    // first, so such a label could never win, and a name that cannot be
    // resolved back is a pane that cannot be addressed.
    assert_eq!(
        address::check_new_name("3", &[]),
        Err(AddressError::NumberName {
            name: "3".to_owned(),
        }),
    );
    assert_eq!(
        address::check_new_name("3a", &[]),
        Ok(()),
        "a name that merely starts with a digit is a name",
    );
}

#[tokio::test]
async fn a_pane_can_be_driven_by_the_number_the_status_line_shows() {
    let server = Server::start("short-address").await;
    let mut client = server.connect().await;
    client.hello_as_attach(amx_proto::version::window()).await;

    let root = state_of(&mut client)
        .await
        .panes
        .first()
        .expect("the seeded pane")
        .pane;
    let (second, second_short) = split(&mut client, root).await;
    assert_eq!(second_short, 2);

    // The root pane takes the *other* pane's number as its label, which is the
    // reason the order is fixed rather than convenient.
    let renamed = client
        .request(
            50,
            "pane.rename",
            json!({ "pane": root.to_string(), "label": "2" }),
        )
        .await;
    assert!(result_of(&renamed).is_object(), "pane.rename answered");

    let read = client
        .request(51, "pane.read", json!({ "target": "2", "lines": 1 }))
        .await;
    let read: pane_proto::ReadReply =
        serde_json::from_value(result_of(&read).clone()).expect("decode pane.read");
    assert_eq!(
        read.pane, second,
        "`2` reached the pane numbered 2, not the pane labelled 2",
    );

    let missing = client
        .request(52, "pane.read", json!({ "target": "9" }))
        .await;
    let amx_proto::RpcOutcome::Error(err) = &missing.outcome else {
        panic!("no pane is numbered 9, so the read must be refused: {missing:?}");
    };
    assert!(
        err.message.contains("numbered 9"),
        "the refusal names the number: {}",
        err.message,
    );

    server.shutdown().await;
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
    assert_ne!(
        fourth, second,
        "a fresh pane, with the departed pane's number"
    );
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
    assert_eq!(
        killed.panes.len(),
        2,
        "the workspace took both panes with it"
    );

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
