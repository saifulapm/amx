//! X10: `agent.list`, D15's one data source, answered from `Core`.
//!
//! Four claims, and the tests are grouped by them.
//!
//! **The reply is complete.** Every field
//! `docs/11-m4-plan.md` §3 tabulates arrives in one call: the workspace with
//! its label, the pane, the agent's name, its kind, its status, the detector's
//! own `reason`, the wall-clock `since` and the literal `last_line`. A surface
//! that had to follow up for any of them would be the 161 ms table X00's
//! baseline measured (`docs/notes/m4-live-smoke.md` §1.4).
//!
//! **`last_line` is `pane.read`'s bottom line and nothing else.** Same rows,
//! same `Row::line().trim_end()`, so a pane's screen has one bottom line rather
//! than two answers. The empty string for a screen with nothing on it, because
//! "this pane has printed nothing" and "this build does not report screen
//! contents" must not be the same bytes.
//!
//! **The filter narrows the rows and never the queue.** `attention` is the
//! global, block-time-ordered queue `agent.next` acts on; a scoped `agent.list`
//! that returned a scoped queue would give a status line and a jump key two
//! different ideas of who has waited longest.
//!
//! **It is one round trip, whatever the pane count.** That is [`direct`]'s,
//! and it is proved by taking the round trips away rather than by timing them.
//!
//! The rig is `agent_verbs`', included by `#[path]` rather than copied — the
//! shape X09 used for `wait_retry/harness.rs`. It is a real `Core` spawning
//! real children on real ptys with a real hub and the shipped fusion machine
//! above them; only the agents are fakes.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::PaneId;
use serde_json::{Value, json};

#[path = "agent_list/direct.rs"]
mod direct;
#[path = "agent_verbs/harness.rs"]
mod harness;
mod support;

use harness::{BLOCKED_MARK, FAKE, IDLE_MARK, Rig, blocked_from_the_start, idles, plant_script};
use support::{PATIENCE, TICK};

/// The rows of one `agent.list` reply.
fn rows(reply: &Value) -> &Vec<Value> {
    reply["agents"].as_array().expect("agents is a list")
}

/// The row for `pane`, or a panic naming what was there instead.
fn row_of(reply: &Value, pane: PaneId) -> &Value {
    rows(reply)
        .iter()
        .find(|row| row["pane"] == pane.to_string())
        .unwrap_or_else(|| panic!("no row for pane {pane} in {reply}"))
}

/// The queue, as pane-id strings.
fn queue(reply: &Value) -> Vec<String> {
    reply["attention"]
        .as_array()
        .map(|queued| {
            queued
                .iter()
                .map(|pane| pane.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------ the reply is complete

/// Every field D15's table renders, from one call.
///
/// The row this asserts is the one 10 §D15 draws in prose — `api/backend ⚑
/// blocked permission 4m │ …` — with the shipped detector name in `reason`
/// rather than a translation of it (D-M4-3), so a renderer built against this
/// reply needs nothing else to draw a line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_call_carries_every_field_the_agents_view_renders() {
    let mut rig = Rig::start("aglist-fields").await;
    plant_script(rig.scripts(), FAKE, &blocked_from_the_start());
    let created = rig
        .call("workspace.create", json!({ "label": "api", "focus": true }))
        .await;
    let workspace = created["workspace"].as_str().expect("a workspace id");
    let (_, pane) = rig.start_agent("backend", FAKE).await;
    rig.wait_status(pane, "blocked").await;
    rig.wait_screen(pane, BLOCKED_MARK).await;
    // The mirror `Core` answers from is posted with an un-awaited `try_send`,
    // so the queue arrives after the status does.
    wait_queued(&mut rig, pane).await;

    let reply = rig.call("agent.list", json!({})).await;
    let row = row_of(&reply, pane);

    assert_eq!(row["workspace"]["id"], *workspace);
    assert_eq!(
        row["workspace"]["name"], "api",
        "the label a person reads rides the row: {row}"
    );
    assert_eq!(
        row["name"], "backend",
        "the agent's name is its pane's label"
    );
    assert_eq!(row["kind"], FAKE);
    assert_eq!(row["status"], "blocked");
    assert_eq!(
        row["reason"], "blocked_marker",
        "reason is the winning manifest rule's own name, never a translation: {row}"
    );
    assert!(
        row["last_line"]
            .as_str()
            .expect("a last line")
            .contains(BLOCKED_MARK),
        "the bottom of the pane's screen is on the row: {row}"
    );

    let since = row["since"].as_u64().expect("a wall-clock stamp");
    let now = reply["now"].as_u64().expect("the server's own now");
    assert!(
        since >= 1_700_000_000_000,
        "since is epoch milliseconds, not seconds and not a bus sequence: {since}"
    );
    assert!(
        now >= since,
        "an age is now - since from inside one reply, so now cannot precede it: \
         now {now}, since {since}"
    );
    assert!(
        reply["seq"].is_u64(),
        "the reply names the seq it was captured at"
    );
    assert_eq!(queue(&reply), vec![pane.to_string()]);

    rig.stop().await;
}

/// A pane the hub has never committed a status for has no row.
///
/// The workspace root panes are plain shells started by `workspace.create`.
/// They are panes of the session, they are in the layout, and they are not
/// agents — so the surfaces that answer "which agents need me" must not have to
/// filter them out three separate times.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pane_with_no_tracked_agent_is_absent_rather_than_listed_as_unknown() {
    let mut rig = Rig::start("aglist-untracked").await;
    plant_script(rig.scripts(), FAKE, &idles());
    let (_, pane) = rig.start_agent("solo", FAKE).await;
    rig.wait_screen(pane, IDLE_MARK).await;

    let state = rig.state().await;
    let all: Vec<String> = state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .map(|entry| entry["pane"].as_str().unwrap_or_default().to_owned())
        .collect();
    let untracked: Vec<String> = state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .filter(|entry| entry["agent"].is_null())
        .map(|entry| entry["pane"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        !untracked.is_empty(),
        "the session's root shell is the untracked pane this test is about; \
         session.state lists {all:?}"
    );

    let reply = rig.call("agent.list", json!({})).await;
    let listed: Vec<String> = rows(&reply)
        .iter()
        .map(|row| row["pane"].as_str().unwrap_or_default().to_owned())
        .collect();
    for pane in &untracked {
        assert!(
            !listed.contains(pane),
            "pane {pane} carries no agent in session.state and must not be a row: {reply}"
        );
    }
    assert!(
        listed.contains(&pane.to_string()),
        "the agent itself is a row: {reply}"
    );

    rig.stop().await;
}

// ------------------------------------------------------------ the last line

/// `last_line` is the bottom of what `pane.read` serves, exactly.
///
/// Read off the same published frame through the same trim, which is what
/// [§7](../../../docs/11-m4-plan.md)'s exit item 1 asks for: `last_line`
/// matching what `pane.read` says the bottom of each pane is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_line_is_the_bottom_non_empty_row_pane_read_serves() {
    let mut rig = Rig::start("aglist-bottom").await;
    plant_script(rig.scripts(), FAKE, &idles());
    let (_, pane) = rig.start_agent("reader", FAKE).await;
    rig.wait_screen(pane, IDLE_MARK).await;

    let screen = rig.screen(pane).await;
    let bottom = screen
        .iter()
        .rev()
        .find(|row| !row.is_empty())
        .cloned()
        .unwrap_or_default();
    assert!(
        bottom.contains(IDLE_MARK),
        "the fake agent's own marker is the bottom line here: {screen:?}"
    );

    let reply = rig.call("agent.list", json!({})).await;
    assert_eq!(
        row_of(&reply, pane)["last_line"],
        bottom,
        "one screen, one bottom line: pane.read reads {screen:?}"
    );

    rig.stop().await;
}

/// A pane with nothing on its screen reports the empty string, not no field.
///
/// The distinction the proto's own doc comment insists on: a row that dropped
/// the key for an empty screen would make "this pane has printed nothing" and
/// "this build does not report screen contents" the same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blank_screen_reports_an_empty_last_line_and_not_a_missing_one() {
    let mut rig = Rig::start("aglist-blank").await;
    // Paints its marker, is identified from it, then wipes the screen: the
    // pane stays tracked and has nothing left to report a bottom line from.
    plant_script(
        rig.scripts(),
        FAKE,
        &format!(
            "stty raw -echo; printf '{IDLE_MARK}\\r\\n'; sleep 1; printf '\\033[2J\\033[H'; sleep 300"
        ),
    );
    let (_, pane) = rig.start_agent("ghost", FAKE).await;
    rig.wait_screen(pane, IDLE_MARK).await;

    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let screen = rig.screen(pane).await;
        if screen.iter().all(String::is_empty) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the fake agent never cleared its screen; it reads {screen:?}"
        );
        tokio::time::sleep(TICK).await;
    }

    let reply = rig.call("agent.list", json!({})).await;
    let row = row_of(&reply, pane);
    assert_eq!(
        row["last_line"], "",
        "a blank screen is an empty line: {row}"
    );
    assert!(
        row.get("last_line").is_some_and(|line| line.is_string()),
        "and the key is present rather than skipped: {row}"
    );

    rig.stop().await;
}

// ------------------------------------------------- the filter and the queue

/// `workspace` narrows the rows; the queue stays global and in its own order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_workspace_filter_narrows_the_rows_and_leaves_the_queue_global() {
    let mut rig = Rig::start("aglist-scope").await;
    plant_script(rig.scripts(), FAKE, &blocked_from_the_start());

    let first = rig
        .call("workspace.create", json!({ "label": "api", "focus": true }))
        .await;
    let api = first["workspace"]
        .as_str()
        .expect("a workspace id")
        .to_owned();
    let (_, api_pane) = rig.start_agent("backend", FAKE).await;
    rig.wait_status(api_pane, "blocked").await;
    wait_queued(&mut rig, api_pane).await;

    let second = rig
        .call("workspace.create", json!({ "label": "web", "focus": true }))
        .await;
    let web = second["workspace"]
        .as_str()
        .expect("a workspace id")
        .to_owned();
    let (_, web_pane) = rig.start_agent("frontend", FAKE).await;
    rig.wait_status(web_pane, "blocked").await;
    wait_queued(&mut rig, web_pane).await;

    let unscoped = rig.call("agent.list", json!({})).await;
    let global = queue(&unscoped);
    assert_eq!(
        global,
        vec![api_pane.to_string(), web_pane.to_string()],
        "the queue is block-time ordered, oldest first: {unscoped}"
    );

    let scoped = rig.call("agent.list", json!({ "workspace": web })).await;
    let listed: Vec<String> = rows(&scoped)
        .iter()
        .map(|row| row["pane"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        listed,
        vec![web_pane.to_string()],
        "a scoped call answers with that workspace's agents and no others: {scoped}"
    );
    assert_eq!(
        queue(&scoped),
        global,
        "and with the same global queue, in the same order — a scoped queue \
         would disagree with agent.next: {scoped}"
    );

    let other = rig.call("agent.list", json!({ "workspace": api })).await;
    let listed: Vec<String> = rows(&other)
        .iter()
        .map(|row| row["pane"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(listed, vec![api_pane.to_string()]);

    rig.stop().await;
}

/// A filter naming no workspace is refused, not answered with an empty list.
///
/// "That project is not here" and "that project has no agents" are different
/// facts, and a caller holding a stale id is entitled to learn which one it
/// holds — which is the same answer every other `workspace` parameter gives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filter_naming_no_workspace_is_refused_rather_than_answered_empty() {
    let mut rig = Rig::start("aglist-noscope").await;
    let ghost = amx_core::WorkspaceId::new_v4();

    let err = rig
        .call_err("agent.list", json!({ "workspace": ghost.to_string() }))
        .await;
    assert_eq!(err.code, amx_proto::rpc::RpcError::INVALID_PARAMS);
    assert!(
        err.message.contains(&ghost.to_string()),
        "a refusal names what it could not find: {err:?}"
    );

    rig.stop().await;
}

// ------------------------------------------------------------- the plumbing

/// Wait until `pane` is on the queue `Core` mirrors.
///
/// `wait_status` reads the hub's own fast view; the queue this method answers
/// with is `Core`'s mirror of it, posted with an un-awaited `try_send`
/// (`docs/08-m2-plan.md` §3), so the two arrive in that order and a read taken
/// between them sees a blocked pane with an empty queue.
async fn wait_queued(rig: &mut Rig, pane: PaneId) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let state = rig.state().await;
        let queued: Vec<String> = state["attention"]
            .as_array()
            .map(|queue| {
                queue
                    .iter()
                    .map(|pane| pane.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        if queued.contains(&pane.to_string()) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "pane {pane} never reached the mirrored queue; it holds {queued:?}"
        );
        tokio::time::sleep(TICK).await;
    }
}
