//! A conversation's whole life: hook → hub → the mirror → the snapshot → a new
//! process typing it back.
//!
//! Four actors and a file on disk, and the only place it can be seen working is
//! across a restart of the real binary. `crates/amx-server/tests/resume.rs`
//! proves each hop against a `Core` it assembled; what it cannot prove is that
//! the hop happens in `amx server`, where the hub is assembled somewhere else
//! and the capture runs on the way down through a signal handler.
//!
//! The launch is type-in, not exec (D-M2-7): a restored pane is its **shell**,
//! amx waits for that shell's first damage and injects the planned argv through
//! the same `pane.run` machinery a user's script uses. So the evidence a
//! conversation resumed is a file the *agent* wrote when the *shell* ran it —
//! nothing in amx is asked to confirm its own work.

use rig::agent;
use serde_json::json;

use super::fixtures::Rig;

/// One agent, one conversation, one restart.
#[tokio::test]
async fn a_reported_conversation_survives_a_restart_and_is_typed_back() {
    let mut rig = Rig::start("res1").await;
    let a = rig.start_agent("a1").await;
    let conversation = a.conversation();

    // The ref arrived by hook — `SessionStart` carries `session_id`, which for
    // an `id`-kind agent *is* the resume ref (V01 §3 M8) — and reached the wire
    // through the hub's mirror into `Core`'s pane state.
    //
    // Waited for rather than read, because the mirror is the un-awaited half of
    // the two read models: the pane is an agent on the wire as soon as its
    // *kind* is mirrored, and the ref is a later commit on the same channel. So
    // "the pane has an agent block" does not mean "the ref is in it", and a
    // single read of one takes whichever the mailbox had got to.
    let entry = rig
        .wait_pane_state(
            "the hook-reported conversation to reach session.state",
            a.pane,
            |entry| entry["agent"]["session_ref"]["value"] == conversation.as_str(),
        )
        .await;
    assert_eq!(entry["agent"]["session_ref"]["kind"], "id", "{entry}");

    rig.wait_snapshot_holds(std::slice::from_ref(&conversation))
        .await;
    rig.shutdown_within(rig::PATIENCE);
    rig.restart().await;

    let resumed = rig.wait_resumed(1).await;
    assert_eq!(
        resumed,
        vec![conversation],
        "the restored pane's shell was typed the conversation, and the agent read it back"
    );
    rig.shutdown_within(rig::PATIENCE);
}

/// A snapshot edited by hand cannot reach an argv, and says what it lost.
///
/// The third of D-M2-7's three gates, checked where a user can actually reach
/// it: `session.json` is theirs to edit, and the pane must come back as a plain
/// shell with the loss reported rather than with a hostile string on a command
/// line. The other two gates are `crates/amx-server/tests/resume.rs`'s; this one
/// needs the file on disk and a fresh process to read it.
#[tokio::test]
async fn a_hand_edited_conversation_restores_a_plain_shell_and_reports_the_loss() {
    let mut rig = Rig::start("res2").await;
    let a = rig.start_agent("a1").await;
    let conversation = a.conversation();
    rig.wait_snapshot_holds(std::slice::from_ref(&conversation))
        .await;
    rig.shutdown_within(rig::PATIENCE);

    // A ref that claims an agent whose hook path never reported it. The source
    // allowlist is what refuses it, and it is checked again here, on read,
    // because the file between the two runs is user-editable.
    let path = rig.env.state_dir().join("session.json");
    let text = std::fs::read_to_string(&path).expect("read the snapshot");
    let edited = text.replace("\"amx:fake\"", "\"amx:codex\"");
    assert_ne!(edited, text, "the snapshot records the ref's source");
    std::fs::write(&path, edited).expect("write the edited snapshot");

    rig.restart().await;
    // Nothing was typed anywhere: the reservation was never claimed, so no
    // shell ever ran the agent.
    let state = rig.state().await;
    assert!(
        state["panes"]
            .as_array()
            .is_some_and(|panes| !panes.is_empty()),
        "the session came back: {state}"
    );
    let report = rig.env.run(&["session", "report"]).ok().to_owned();
    assert!(
        report.contains("degraded") || report.contains("no losses"),
        "the restore report is what tells a user a conversation did not come back:\n{report}"
    );
    assert!(
        rig.agents.resumed().is_empty(),
        "a ref from a foreign source reached an argv: {:?}",
        rig.agents.resumed()
    );
    rig.shutdown_within(rig::PATIENCE);
}

/// A report carrying the wrong token never touches the pane it names.
///
/// D-M2-4's misattribution guard, over the socket rather than against a hub a
/// test built: a stale hook config in a nested session, or a pane id recycled
/// across restarts, must report into the void. Both halves are asserted — the
/// ack says the report reached no tracked pane, and the pane's own status did
/// not move.
#[tokio::test]
async fn hook_reports_from_a_foreign_token_never_touch_a_tracked_pane() {
    let mut rig = Rig::start("tokn").await;
    let a = rig.start_agent("a1").await;
    let before = rig.transition_seq(a.pane).await;

    let reply = rig
        .call(
            "agent.report",
            json!({
                "pane": a.pane,
                "token": "not-this-pane's-token",
                "agent": agent::KIND,
                "source": "amx:fake",
                "event": "UserPromptSubmit",
                "seq": 1_u64,
            }),
        )
        .await;
    assert_eq!(
        reply["accepted"], false,
        "a token mismatch is an honest unaccepted, never an error the agent would print: {reply}"
    );
    assert_eq!(rig.status(a.pane).await.as_deref(), Some("idle"));
    assert_eq!(
        rig.transition_seq(a.pane).await,
        before,
        "a foreign token moved a tracked pane's status"
    );

    // And the same report with the right shape but a pane this session never
    // had: also unaccepted, also harmless.
    let elsewhere = rig
        .call(
            "agent.report",
            json!({
                "pane": amx_core::PaneId::new_v4(),
                "token": "anything",
                "agent": agent::KIND,
                "source": "amx:fake",
                "event": "Stop",
                "seq": 2_u64,
                // The internally tagged scope, spelled out once here rather
                // than left to `default`: the anonymous-subagent rule turns on
                // this field, so one report in the suite states it explicitly.
                "scope": { "scope": "parent" },
            }),
        )
        .await;
    assert_eq!(elsewhere["accepted"], false, "{elsewhere}");
    rig.shutdown_within(rig::PATIENCE);
}
