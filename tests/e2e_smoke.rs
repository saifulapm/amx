//! Smoke tests for mock-claude, the stand-in the other suites rely on.
//!
//! A scenario runs in a real tmux pane, its hooks go through amx's real hook
//! path, and the agent's record moves as they arrive.

mod common;

use common::Harness;
use std::process::Command;

#[test]
fn the_harness_runs_amx_in_its_home_not_in_the_checkout() {
    // cargo runs a test in the checkout, where an `amx new` naming no `--dir`
    // would cut its tree and branch (483849b5).
    let amx = Harness::new();
    let command = amx.amx_command(&["new", "--exec", "true"]);
    assert_eq!(command.get_current_dir(), Some(amx.home()));
}

#[test]
fn the_harness_refuses_a_test_binary_built_for_another_checkout() {
    // cargo runs a test in the checkout it was built for. A run that reused
    // binaries a removed worktree had built into the shared target dir read
    // fixtures from the removed folder, and 74 tests failed far from the
    // cause (lessons 2026-11-16).
    let elsewhere = tempfile::TempDir::new().unwrap();
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "the_harness_runs_amx_in_its_home_not_in_the_checkout",
        ])
        .current_dir(elsewhere.path())
        .output()
        .expect("running this test binary");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(&format!(
            "this test binary was built for {}",
            env!("CARGO_MANIFEST_DIR")
        )),
        "{said}"
    );
}

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

    let kinds = amx.event_kinds("fix-login-a1b");
    assert_eq!(
        kinds,
        ["SessionStart", "UserPromptSubmit", "PreToolUse", "Stop"],
        "every hook the scenario delivered arrived"
    );

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

    // The footer is printed last, after the Stop hook has run, so waiting on
    // anything earlier races that hook.
    let screen = amx.until("the vendor's screen", || {
        let screen = amx.capture(&pane);
        screen.contains("⏵⏵ auto mode on").then_some(screen)
    });
    assert!(
        screen.contains("the tests pass now"),
        "what the vendor said is on the pane under its own chrome: {screen}"
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

    // The stand-in prints the box a line at a time, so wait for its last line.
    let screen = amx.until("the permission box", || {
        let screen = amx.capture(&pane);
        screen.contains("Esc to cancel").then_some(screen)
    });
    assert!(screen.contains("Do you want to proceed?"), "{screen}");
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
fn a_turn_that_captured_no_answer_says_why_the_vendor_stopped() {
    // The vendor writes the stop reason only to the transcript, and a caller
    // needs it to decide whether to retry, so `result` reads it from there.
    let amx = Harness::new();
    amx.play("summarise-d4e", "stops-at-the-token-limit");
    amx.until_state("summarise-d4e", "idle");

    let out = amx.amx(&["result", "summarise-d4e"]);
    assert_eq!(out.status.code(), Some(1), "no answer is a failure");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("no answer was captured"), "{said}");
    assert!(said.contains("token limit"), "and why: {said}");
}

#[test]
fn hooks_from_a_pane_amx_knows_nothing_about_are_ignored() {
    // A claude started outside amx fires the same hooks, which must be no-ops.
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
