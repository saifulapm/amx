//! The vendor's stand-in, driven through a real pane.
//!
//! Everything else in this suite rests on this fixture, so it is checked here:
//! a fixture nobody checks is a fixture that quietly stops proving anything.
//! What these tests prove is that a scenario runs in a tmux pane, that its
//! hooks travel amx's real path, and that the agent's record moves as they
//! arrive.

mod common;

use common::Harness;
use std::process::Command;

#[test]
fn a_scenario_moves_the_record_through_a_turn() {
    let amx = Harness::new();
    let pane = amx.play("fix-login-a1b", "happy-turn");

    let state = amx.until_state("fix-login-a1b", "idle");
    assert_eq!(state["result"], "the tests pass now");
    assert_eq!(
        state["source"], "payload",
        "the Stop payload is the freshest place the answer is"
    );
    assert_eq!(state["question"], serde_json::Value::Null);
    assert!(state["last_event"].as_u64().unwrap() > 0);

    // The whole turn is on the record, in the order it happened.
    let kinds = amx.event_kinds("fix-login-a1b");
    assert_eq!(
        kinds,
        ["SessionStart", "UserPromptSubmit", "PreToolUse", "Stop"],
        "every hook the scenario delivered arrived"
    );

    // And the session the vendor announced is on the record too.
    let meta = amx.meta("fix-login-a1b");
    assert!(meta["session"].is_string(), "{meta}");
    assert_eq!(
        meta["transcript"],
        amx.transcript("fix-login-a1b").to_string_lossy().as_ref()
    );

    assert!(
        amx.pane_alive(&pane),
        "and the agent is still sitting there"
    );
}

#[test]
fn a_scenario_prints_to_a_real_pane() {
    let amx = Harness::new();
    let pane = amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let screen = amx.until("the vendor's screen", || {
        let screen = amx.capture(&pane);
        screen.contains("the tests pass now").then_some(screen)
    });
    assert!(
        screen.contains("⏵⏵ auto mode on"),
        "the vendor's own chrome is on the pane: {screen}"
    );
}

#[test]
fn a_question_reaches_the_record_with_its_answer_outstanding() {
    let amx = Harness::new();
    let pane = amx.play("ask-a1b", "asks-a-question");

    let state = amx.until_state("ask-a1b", "waiting");
    assert_eq!(
        state["question"], "Claude needs your permission to use Bash",
        "the question is what a person is being asked, verbatim"
    );

    let screen = amx.until("the permission box", || {
        let screen = amx.capture(&pane);
        screen.contains("Do you want to proceed?").then_some(screen)
    });
    assert!(screen.contains("❯ 1. Yes"), "{screen}");
}

#[test]
fn an_agent_that_ends_badly_records_the_code_it_ended_with() {
    let amx = Harness::new();
    amx.play("port-importer-c3d", "fails");

    let state = amx.until_state("port-importer-c3d", "failed");
    assert_eq!(state["exit"], 2);
    assert_eq!(
        amx.event_kinds("port-importer-c3d").last().unwrap(),
        "exit",
        "how it ended is the last thing that happened to it"
    );
}

#[test]
fn an_agent_that_finishes_records_that_it_did() {
    let amx = Harness::new();
    let pane = amx.play("say-hello-b2c", "finishes");

    let state = amx.until_state("say-hello-b2c", "done");
    assert_eq!(state["exit"], 0);
    assert_eq!(state["result"], "hello", "the answer outlives the pane");

    amx.until("the pane to close with the command", || {
        (!amx.pane_alive(&pane)).then_some(())
    });
}

#[test]
fn an_answer_the_payload_did_not_carry_comes_from_the_transcript() {
    let amx = Harness::new();
    amx.play("summarise-d4e", "answers-from-the-transcript");

    let state = amx.until_state("summarise-d4e", "idle");
    assert_eq!(state["result"], "the importer now retries");
    assert_eq!(state["source"], "transcript");
}

#[test]
fn hooks_from_a_pane_amx_knows_nothing_about_are_ignored() {
    // A claude somebody started themselves fires the same hooks. They must
    // cost nothing and change nothing.
    let amx = Harness::new();
    let out = amx.amx(&["_hook"]);
    assert!(out.status.success(), "a hook always exits 0");
    assert!(
        !amx.state_root().exists() || amx.state_root().read_dir().unwrap().next().is_none(),
        "and writes no records"
    );
}

#[test]
fn the_fixture_says_so_when_it_has_no_scenario_to_play() {
    let out = Command::new(common::fixtures().join("mock-claude"))
        .output()
        .expect("running the fixture");
    assert_eq!(out.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&out.stderr).contains("MOCK_CLAUDE_SCENARIO"));

    let missing = Command::new(common::fixtures().join("mock-claude"))
        .env("MOCK_CLAUDE_SCENARIO", "/nowhere/at/all.scenario")
        .output()
        .expect("running the fixture");
    assert_eq!(missing.status.code(), Some(66));
}

#[test]
fn the_fixture_refuses_a_step_it_does_not_know() {
    // A scenario with a typo in it must fail loudly, not quietly do nothing.
    let dir = tempfile::TempDir::new().unwrap();
    let scenario = dir.path().join("typo.scenario");
    std::fs::write(&scenario, "prnit hello\n").unwrap();

    let out = Command::new(common::fixtures().join("mock-claude"))
        .env("MOCK_CLAUDE_SCENARIO", &scenario)
        .output()
        .expect("running the fixture");
    assert_eq!(out.status.code(), Some(65));
    assert!(String::from_utf8_lossy(&out.stderr).contains("prnit"));
}
