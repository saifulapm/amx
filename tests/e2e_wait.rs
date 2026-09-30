//! `amx wait` and `amx result --children`: one timeout over several agents,
//! against real panes.
//!
//! Callers branch on the exit code and read the lines as they arrive, so each
//! test asserts the code first and then the full output, order included. Every
//! wait has a `--timeout`, so a wait that never returns fails only its test.

mod common;

use common::{Harness, code, stderr, stdout};
use serde_json::json;

/// Stamp `since` and `last_event` on the agent's state, as `amx new` does.
///
/// A hand-made record has no stamp and is stale from the start: until the
/// first hook lands, the reader trusts the blank pane and reports an agent
/// that has not started yet as having finished its turn.
fn started(id: &str, amx: &Harness) {
    let now = common::now();
    amx.set_state(
        id,
        json!({ "state": "starting", "since": now, "last_event": now }),
    );
}

#[test]
fn every_agent_is_named_as_it_settles_and_the_wait_ends_with_the_last() {
    let amx = Harness::new();
    // `b` is idle before the wait starts; `a` ends while the wait runs.
    //
    // `a` exits without a Stop hook, so a sweep sees it `working` or `done`. A
    // scenario that sends Stop and then exits reads `idle` until its pane goes,
    // and a sweep in that gap would print a phase the agent does not keep.
    amx.play("b", "happy-turn");
    amx.until_state("b", "idle");
    amx.play("a", "ends-without-an-answer");
    started("a", &amx);

    let out = amx.amx(&["wait", "a", "b", "--timeout", "20"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "b idle\na done\n",
        "the order they settled in, not the order they were named"
    );
}

#[test]
fn any_comes_back_with_the_first_and_leaves_the_rest_working() {
    let amx = Harness::new();
    amx.play("watch-log-e5f", "works-without-end");
    amx.until_state("watch-log-e5f", "working");
    amx.play("say-hello-b2c", "finishes");
    amx.until_state("say-hello-b2c", "done");

    let out = amx.amx(&[
        "wait",
        "watch-log-e5f",
        "say-hello-b2c",
        "--any",
        "--timeout",
        "20",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "say-hello-b2c done\n",
        "the one that is ready, and nothing about the one that is not"
    );
    assert_eq!(
        amx.state("watch-log-e5f")["state"],
        "working",
        "the agent still at work was not waited out"
    );
    assert!(
        amx.pane_alive(&amx.pane_of("watch-log-e5f")),
        "and it is still there to be waited on again"
    );
}

#[test]
fn for_a_phase_comes_back_when_the_agent_reaches_it() {
    // No `until_state` first: the agent is still `starting` when the wait
    // begins.
    let amx = Harness::new();
    amx.play("watch-log-e5f", "works-without-end");

    let out = amx.amx(&[
        "wait",
        "watch-log-e5f",
        "--for",
        "working",
        "--timeout",
        "20",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "watch-log-e5f working\n");
}

#[test]
fn a_wait_gives_up_when_the_caller_says_when() {
    let amx = Harness::new();
    amx.play("watch-log-e5f", "works-without-end");
    amx.until_state("watch-log-e5f", "working");

    let out = amx.amx(&["wait", "watch-log-e5f", "--timeout", "1"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(stdout(&out), "", "nothing settled, so nothing is ready");
}

#[test]
fn an_agent_stopped_on_a_question_is_one_the_caller_can_act_on() {
    // `waiting` counts as settled: the question arrives mid-wait, and a caller
    // that is not told about it cannot answer it.
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    started("ask-a1b", &amx);

    let out = amx.amx(&["wait", "ask-a1b", "--timeout", "20"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "ask-a1b waiting\n");
}

#[test]
fn an_id_that_names_no_agent_is_refused_before_the_wait_starts() {
    // A wait on an unknown id could never end. It is refused before the first
    // sweep, so even the agent that is ready is not printed.
    let amx = Harness::new();
    amx.play("say-hello-b2c", "finishes");
    amx.until_state("say-hello-b2c", "done");

    let out = amx.amx(&["wait", "say-hello-b2c", "never-made-abc", "--timeout", "20"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("never-made-abc"), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn a_state_nobody_knows_is_a_command_line_to_fix() {
    let amx = Harness::new();
    amx.play("watch-log-e5f", "works-without-end");

    let out = amx.amx(&[
        "wait",
        "watch-log-e5f",
        "--for",
        "sleeping",
        "--timeout",
        "20",
    ]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("sleeping"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("waiting"),
        "and the words it could have been: {}",
        stderr(&out)
    );
    assert_eq!(stdout(&out), "", "nothing was waited on");
}

#[test]
fn wait_children_covers_a_parents_whole_family_with_one_clock() {
    // The fan-in after `amx sub --bg`: every record whose `parent` is the
    // given agent, in creation order.
    let amx = Harness::new();
    amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    amx.play("kid-one-b2c", "happy-turn");
    amx.until_state("kid-one-b2c", "idle");
    amx.play("kid-two-c3d", "ends-without-an-answer");
    started("kid-two-c3d", &amx);
    amx.set_meta("kid-one-b2c", json!({ "parent": "parent-a1b", "depth": 1 }));
    amx.set_meta("kid-two-c3d", json!({ "parent": "parent-a1b", "depth": 1 }));

    let out = amx.amx(&["wait", "--children", "parent-a1b", "--timeout", "20"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "kid-one-b2c idle\nkid-two-c3d done\n");
}

#[test]
fn wait_children_of_a_childless_parent_is_a_failure() {
    let amx = Harness::new();
    amx.play("lonely-a1b", "happy-turn");
    amx.until_state("lonely-a1b", "idle");

    let out = amx.amx(&["wait", "--children", "lonely-a1b", "--timeout", "5"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("has no children"), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn wait_children_refuses_a_parent_nobody_knows() {
    let amx = Harness::new();
    let out = amx.amx(&["wait", "--children", "never-made-abc", "--timeout", "5"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("never-made-abc"), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn result_children_hands_every_childs_answer_back() {
    let amx = Harness::new();
    amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    amx.play("kid-one-b2c", "happy-turn");
    amx.until_state("kid-one-b2c", "idle");
    amx.play("kid-two-c3d", "happy-turn");
    amx.until_state("kid-two-c3d", "idle");
    amx.set_meta("kid-one-b2c", json!({ "parent": "parent-a1b", "depth": 1 }));
    amx.set_meta("kid-two-c3d", json!({ "parent": "parent-a1b", "depth": 1 }));

    let out = amx.amx(&["result", "--children", "parent-a1b", "--timeout", "20"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "kid-one-b2c idle\nthe tests pass now\nkid-two-c3d idle\nthe tests pass now\n"
    );

    // `--json` prints one object keyed by child id.
    let out = amx.amx(&[
        "result",
        "--children",
        "parent-a1b",
        "--json",
        "--timeout",
        "20",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let object: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one json object");
    assert_eq!(object["kid-one-b2c"]["answer"], "the tests pass now");
    assert_eq!(object["kid-one-b2c"]["phase"], "idle");
    assert_eq!(object["kid-two-c3d"]["answer"], "the tests pass now");
}

#[test]
fn result_children_surfaces_a_waiting_childs_question() {
    let amx = Harness::new();
    amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    amx.play("kid-asks-b2c", "asks-a-question");
    amx.until_state("kid-asks-b2c", "waiting");
    amx.set_meta(
        "kid-asks-b2c",
        json!({ "parent": "parent-a1b", "depth": 1 }),
    );

    let out = amx.amx(&[
        "result",
        "--children",
        "parent-a1b",
        "--json",
        "--timeout",
        "5",
    ]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let object: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one json object");
    assert_eq!(object["kid-asks-b2c"]["phase"], "waiting");
    assert!(
        object["kid-asks-b2c"]["question"]
            .as_str()
            .is_some_and(|question| !question.is_empty()),
        "the question rides in the collection: {object}"
    );
}

/// Append a `kind` event to the agent's events.jsonl, as amx's verbs do.
fn happened(amx: &Harness, id: &str, kind: &str) {
    use std::io::Write;
    let line = json!({ "at": 1, "kind": kind, "payload": { "text": "and now the linter" } });
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(amx.agent_dir(id).join("events.jsonl"))
        .expect("the agent's log");
    writeln!(log, "{line}").expect("an event appended");
}

#[test]
fn a_message_a_hand_interrupt_left_open_keeps_the_family_from_settling() {
    // Esc in the pane ends the turn without recording anything, so the child
    // reads idle with amx's message unanswered. The answer on record belongs
    // to the previous turn and must not be returned for this one.
    let amx = Harness::new();
    amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    amx.play("kid-one-b2c", "happy-turn");
    amx.until_state("kid-one-b2c", "idle");
    amx.set_meta("kid-one-b2c", json!({ "parent": "parent-a1b", "depth": 1 }));
    happened(&amx, "kid-one-b2c", "send");

    let out = amx.amx(&["wait", "--children", "parent-a1b", "--timeout", "2"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let out = amx.amx(&["result", "--children", "parent-a1b", "--timeout", "2"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));

    // `amx interrupt` records an event, so the family settles, and the turn it
    // cut short has no answer.
    happened(&amx, "kid-one-b2c", "interrupt");
    let out = amx.amx(&["wait", "--children", "parent-a1b", "--timeout", "5"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "kid-one-b2c idle\n");
    let out = amx.amx(&["result", "--children", "parent-a1b", "--timeout", "5"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out), "kid-one-b2c idle\n");
}
