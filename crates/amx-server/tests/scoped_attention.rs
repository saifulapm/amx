//! X17: `agent.next --workspace` from the caller's end.
//!
//! `agent_hub/scope.rs` drives the hub's mailbox and asserts what the queue
//! does. This one asserts the other half — that the scope a caller sends
//! *arrives*: through the method table's decode, through `dispatch::agent`, and
//! into the actor that holds the queue. The two halves were separate failures
//! waiting to happen, because the field spent all of wave 1 and wave 2 parsed,
//! typed, wire-frozen and destructured into `_` (`docs/notes/m4-wave-outcomes.md`,
//! X02's hand-off), which is exactly the shape D-M4-10 exists to catch: a field
//! whose reader never came.
//!
//! It borrows `agent_verbs`' harness rather than copying it — a real `Core`
//! spawning real children on real ptys, a real hub, the shipped fusion machine
//! and rule engine, with `/bin/sh` fakes for the agents themselves. The calls go
//! through `amx_server::dispatch::handle`, which is the same decode a socket
//! reaches.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{PaneId, WorkspaceId};
use serde_json::{Value, json};

#[path = "agent_verbs/harness.rs"]
mod harness;
mod support;

use harness::{FAKE, Rig, blocked_from_the_start, plant_script};

/// Read a workspace id out of a reply field.
fn workspace_of(reply: &Value) -> WorkspaceId {
    reply["workspace"]
        .as_str()
        .unwrap_or_else(|| panic!("workspace is a workspace id: {reply}"))
        .parse()
        .expect("a workspace id")
}

/// Start an agent that blocks immediately in `workspace`, and answer its pane.
///
/// Readiness times out on purpose: this agent is never idle, and the pane is
/// left running, which is the semantics `agent.start` already has for an agent
/// that does not come up (V13's `start_timeout_reports_failure_but_leaves_the_
/// pane_running`). What this helper waits for is the thing the test is about —
/// the pane reaching `blocked`, which is what puts it in the queue.
async fn blocked_agent(rig: &mut Rig, name: &str, workspace: WorkspaceId) -> PaneId {
    let reply = rig
        .call(
            "agent.start",
            json!({
                "name": name,
                "kind": FAKE,
                "workspace": workspace.to_string(),
                "timeout_ms": 400,
            }),
        )
        .await;
    let pane: PaneId = reply["pane"]
        .as_str()
        .unwrap_or_else(|| panic!("pane is a pane id: {reply}"))
        .parse()
        .expect("a pane id");
    rig.wait_status(pane, "blocked").await;
    pane
}

/// The scope reaches the queue, and the unscoped call is what it always was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_next_scoped_to_a_workspace_focuses_that_workspaces_block() {
    let mut rig = Rig::start("x17e").await;
    plant_script(rig.scripts(), FAKE, &blocked_from_the_start());

    // Two workspaces made by name, neither of them focused, so both ids are
    // known here rather than dug out of a layout tree.
    let first = workspace_of(
        &rig.call("workspace.create", json!({ "label": "api" }))
            .await,
    );
    let second = workspace_of(
        &rig.call("workspace.create", json!({ "label": "web" }))
            .await,
    );

    // Blocked in this order, so `api`'s agent is the global head and `web`'s is
    // behind it: an unscoped call can only answer `api`.
    let api = blocked_agent(&mut rig, "one", first).await;
    let web = blocked_agent(&mut rig, "two", second).await;

    let scoped = rig
        .call("agent.next", json!({ "workspace": second.to_string() }))
        .await;
    assert_eq!(
        scoped["pane"],
        web.to_string(),
        "the scope never reached the queue: {scoped}",
    );
    assert_eq!(scoped["workspace"], second.to_string(), "{scoped}");
    assert_eq!(scoped["waiting"], 1, "how many wait in `web`: {scoped}");

    // The global queue is untouched by the scoped call above: same head, same
    // count. Clearing one project's queue does not consume another's.
    let global = rig.call("agent.next", json!({})).await;
    assert_eq!(global["pane"], api.to_string(), "{global}");
    assert_eq!(global["workspace"], first.to_string(), "{global}");
    assert_eq!(global["waiting"], 2, "{global}");

    // A scope with nothing blocked in it is an honest empty reply and not an
    // error — the shape the unscoped call already has for an empty queue.
    let idle_scope = rig
        .call(
            "agent.next",
            json!({ "workspace": WorkspaceId::new_v4().to_string() }),
        )
        .await;
    assert_eq!(idle_scope["pane"], Value::Null, "{idle_scope}");
    assert_eq!(idle_scope["waiting"], 0, "{idle_scope}");

    rig.stop().await;
}
