//! Registry → spawn → manifest → tier-2 status → `agent explain`, and the
//! waits that read the same model.
//!
//! This is the chain a user follows when a status is wrong, and 04 §5 keeps
//! `agent explain` precisely so they can: "a detection you cannot interrogate
//! is a detection you cannot fix". Every link is asserted here over the wire —
//! the stanza that decided the argv, the argv that identified the pane, the
//! manifest that stanza named, the rule that matched, and the status the rule
//! produced.
//!
//! The waits belong in the same module because they read the *other* model of
//! the same truth: `StatusView`, written before the event that announces it
//! (§3's ordering). A wait that answered from a view nobody wrote is the
//! failure this suite exists to make impossible, and it is what the tree
//! actually did before V17 joined the gateway's view to the hub's.

use std::time::Duration;

use rig::agent;
use serde_json::json;

use super::fixtures::{Rig, connected};

/// The registry decided what ran, and the run identified itself.
///
/// Three facts in one pane: the process is the stanza's program, the pane's
/// status names the stanza's id, and the manifest bound to it is the one the
/// stanza asked for — which for the scripted agent is the *shipped*
/// `claude.toml`, so the rules under test here are the rules a real Claude Code
/// pane runs on.
///
/// The argv is read off the durable snapshot rather than off `session.state`,
/// because that is the only place it is visible from outside the server: it is
/// pane state for identity and for restore, not a wire field, and V07 recorded
/// it exactly so a later run can reason about what was running here.
#[tokio::test]
async fn the_registry_stanza_decides_the_argv_the_identity_and_the_manifest() {
    let mut rig = Rig::start("regi").await;
    let a = rig.start_agent("a1").await;

    let entry = rig.pane_state(a.pane).await;
    assert_eq!(entry["agent"]["kind"], agent::KIND, "{entry}");

    let explained = rig.call("agent.explain", json!({ "target": a.name })).await;
    assert!(
        explained["manifest"]
            .as_str()
            .is_some_and(|name| name.ends_with(agent::MANIFEST)),
        "the stanza's manifest is the one bound to the pane: {explained}"
    );
    assert_eq!(explained["kind"], agent::KIND, "{explained}");

    let program = rig.agents.program().to_string_lossy().into_owned();
    rig.wait_snapshot_holds(std::slice::from_ref(&program))
        .await;
    let snapshot = rig.snapshot().expect("the snapshot just landed");
    let saved = snapshot["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .find(|pane| pane["id"] == a.pane.to_string())
        .cloned()
        .unwrap_or_else(|| panic!("no pane {} in the snapshot: {snapshot}", a.pane));
    assert_eq!(
        saved["argv"][0], program,
        "the pane recorded the program the stanza named: {saved}"
    );
    rig.shutdown_within(rig::PATIENCE);
}

/// `agent explain` reports every rule's verdict, not only the winner's.
///
/// Driven across two states so the *winner moves*: the same manifest, the same
/// pane, a different screen, and a different rule named. A reply that only ever
/// said "matched: something" would pass one of these and not both.
#[tokio::test]
async fn explain_names_the_matching_rule_and_reports_every_other_one() {
    let mut rig = Rig::start("expl").await;
    let a = rig.start_agent("a1").await;

    let idle = rig.call("agent.explain", json!({ "target": a.name })).await;
    assert_eq!(
        idle["matched"], "prompt_box_idle",
        "the composer between two rules is what an idle Claude Code looks like: {idle}"
    );
    assert_eq!(idle["agent"]["state"], "idle", "{idle}");
    let rules = idle["rules"].as_array().expect("every rule reports");
    assert!(
        rules.len() >= 5,
        "the shipped manifest's whole rule list comes back, not just the winner: {idle}"
    );
    assert!(
        rules
            .iter()
            .any(|rule| rule["rule"] == "permission_dialog" && rule["matched"] == false),
        "a rule that did not match says so, which is half of what makes this debuggable: {idle}"
    );
    assert!(
        !idle["region_preview"]
            .as_array()
            .expect("a region preview")
            .is_empty(),
        "the text the rules were evaluated against comes back with them: {idle}"
    );

    rig.block(&a).await;
    let blocked = rig.call("agent.explain", json!({ "target": a.name })).await;
    assert_eq!(blocked["matched"], "permission_dialog", "{blocked}");
    assert_eq!(blocked["agent"]["state"], "blocked", "{blocked}");
    rig.shutdown_within(rig::PATIENCE);
}

/// The diagnostic verb answers for a pane that is *not* an agent.
///
/// The reason `agent.explain` resolves its target in the wider scope: the
/// question it exists to answer is "why does amx not think this is an agent?",
/// and a resolver that refused to name an unidentified pane would refuse
/// exactly the pane being asked about. An honest "no manifest here" beats an
/// empty rule list that reads like "nothing matched".
#[tokio::test]
async fn explain_answers_for_a_plain_shell_without_pretending_it_has_rules() {
    let mut rig = Rig::start("plai").await;
    let created = rig.call("workspace.create", json!({ "focus": true })).await;
    let switched = rig
        .call(
            "workspace.switch",
            json!({ "workspace": created["workspace"] }),
        )
        .await;
    let root = switched["focused_pane"].as_str().expect("a focused pane");
    rig.call("pane.rename", json!({ "pane": root, "label": "shell" }))
        .await;

    let explained = rig
        .call("agent.explain", json!({ "target": "shell" }))
        .await;
    assert!(explained.get("manifest").is_none(), "{explained}");
    assert!(
        explained["rules"].as_array().is_none_or(Vec::is_empty),
        "no manifest means no rules, said plainly: {explained}"
    );
    rig.shutdown_within(rig::PATIENCE);
}

/// A wire wait returns on the transition, with no poll interval anywhere.
///
/// Two connections, because that is the shape a script uses: one holds the
/// long poll open, the other drives the agent. The claim is *ordering* — the
/// wait was still outstanding when the block was triggered, and completed after
/// — which is what distinguishes a wait from a query that happened to be right.
#[tokio::test]
async fn wait_until_blocked_races_no_poll_interval() {
    let mut rig = Rig::start("wait").await;
    let a = rig.start_agent("a1").await;

    let mut waiter = connected(&rig.env).await;
    let target = a.name.clone();
    let waiting = tokio::spawn(async move {
        waiter
            .request(
                "wait",
                json!({ "until": "blocked", "target": target, "timeout_ms": 30_000 }),
            )
            .await
    });

    // Still outstanding: the pane is idle, so nothing could have satisfied it.
    assert!(
        !waiting.is_finished(),
        "the wait answered before there was anything to answer"
    );
    rig.drive(&a.name, agent::ASK).await;

    let reply = tokio::time::timeout(rig::PATIENCE, waiting)
        .await
        .expect("the wait returned inside the suite's patience")
        .expect("the waiting task did not panic");
    let value = rig::result_of(&reply);
    assert_eq!(value["satisfied"], true, "{value}");
    assert_eq!(value["agent"]["state"], "blocked", "{value}");
    assert_eq!(value["pane"], a.pane.to_string(), "{value}");
    rig.shutdown_within(rig::PATIENCE);
}

/// A condition already true returns without waiting for an event.
///
/// The other half of the predicate contract: a wait reads *live state*, so a
/// pane that is already idle answers immediately rather than hanging until the
/// next transition happens to come along.
#[tokio::test]
async fn a_wait_on_a_condition_already_true_returns_at_once() {
    let mut rig = Rig::start("wnow").await;
    let a = rig.start_agent("a1").await;

    let reply = rig
        .call(
            "wait",
            json!({ "until": "idle", "target": a.name, "timeout_ms": 30_000 }),
        )
        .await;
    assert_eq!(reply["satisfied"], true, "{reply}");
    assert_eq!(reply["agent"]["state"], "idle", "{reply}");
    rig.shutdown_within(rig::PATIENCE);
}

/// `agent.prompt --wait` rides the same read model, end to end.
///
/// The verb a script actually uses: submit, and come back when the agent is
/// blocked. The floor is what makes it correct on a pane that was already
/// blocked once, and V13's suite pins that; what this adds is that the whole
/// path works over a socket, where the status view a predicate reads is the
/// gateway's rather than one the test built.
#[tokio::test]
async fn agent_prompt_waits_for_the_block_it_caused() {
    let mut rig = Rig::start("promp").await;
    let a = rig.start_agent("a1").await;

    let reply = rig
        .call(
            "agent.prompt",
            json!({
                "target": a.name,
                "text": agent::ASK,
                "wait": "blocked",
                "timeout_ms": 30_000,
            }),
        )
        .await;
    assert_eq!(reply["satisfied"], true, "{reply}");
    assert_eq!(reply["agent"]["state"], "blocked", "{reply}");
    rig.shutdown_within(rig::PATIENCE);
}

/// A timeout is an answer, and it takes the time it was given.
///
/// Named here because the wait tests above prove the satisfied path only, and
/// a wait that returned `satisfied: false` immediately would pass every one of
/// them.
#[tokio::test]
async fn an_unsatisfied_wait_reports_so_rather_than_failing() {
    let mut rig = Rig::start("wto").await;
    let a = rig.start_agent("a1").await;

    let started = std::time::Instant::now();
    let reply = rig
        .call(
            "wait",
            json!({ "until": "blocked", "target": a.name, "timeout_ms": 300 }),
        )
        .await;
    assert_eq!(reply["satisfied"], false, "{reply}");
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "the wait returned before the timeout it was given"
    );
    rig.shutdown_within(rig::PATIENCE);
}
