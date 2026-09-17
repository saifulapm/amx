//! `amx wait` end to end: one clock over several agents, against real panes.
//!
//! What a coordinator does with this verb is branch on the exit code and read
//! the lines as they arrive, so every test here asserts the code first and then
//! the whole of what was printed — the lines and their order are the answer.
//!
//! Every wait is bounded by `--timeout`, so a verb that never comes back fails
//! the test it is in rather than the suite it is in.

mod common;

use common::Harness;
use serde_json::json;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

/// The code the caller branches on.
fn code(out: &Output) -> i32 {
    out.status.code().expect("amx exited with a code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Stamp an agent's record the way `amx new` stamps the one it writes.
///
/// A record made by hand here carries no stamp at all, so it is stale from
/// birth: until the agent's first hook lands, a reader believes the blank pane
/// over it and calls an agent that has not started yet a turn that is over.
/// `amx new` writes `since` as it creates the record, and an agent that is only
/// just starting is what these tests are waiting on.
fn started(id: &str, amx: &Harness) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs();
    amx.set_state(
        id,
        json!({ "state": "starting", "since": now, "last_event": now }),
    );
}

#[test]
fn every_agent_is_named_as_it_settles_and_the_wait_ends_with_the_last() {
    let amx = Harness::new();
    // `b` has finished its turn before anybody waits on anything; `a` is only
    // just starting, and ends while the wait is already running.
    //
    // `a` plays the scenario that ends with no Stop hook, so the phase a sweep
    // catches it in is `working` or `done` and never anything in between. A
    // scenario that announces the end of its turn and then exits is `idle` for
    // as long as it takes the pane to go, and a sweep landing in there reads a
    // settled agent whose phase is not the one it is about to keep.
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
    // One agent whose turn is over and one whose turn never ends: what `--any`
    // is for is not waiting out the second to hear about the first.
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
    // Nothing is waited for first: the agent is `starting` when the wait
    // begins, so what this proves is the wait holding out for `working` — a
    // caller confirming its fleet got off the ground.
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
    // A wait that went through a question would be the wait `result` refuses to
    // be: the question arrives while the wait is running, and the caller cannot
    // answer what it is not told about.
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    started("ask-a1b", &amx);

    let out = amx.amx(&["wait", "ask-a1b", "--timeout", "20"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "ask-a1b waiting\n");
}

#[test]
fn an_id_that_names_no_agent_is_refused_before_the_wait_starts() {
    // A wait on an agent nobody has could never end, and the caller can still
    // fix what it typed: the refusal comes before the first sweep, so not even
    // the agent that is ready is named.
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
    // The fan-in for a parent that fanned out with `amx sub --bg`: the records
    // that name the parent are the ids, in the order they were made.
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
fn wait_children_of_a_childless_parent_prints_nothing() {
    let amx = Harness::new();
    amx.play("lonely-a1b", "happy-turn");
    amx.until_state("lonely-a1b", "idle");

    let out = amx.amx(&["wait", "--children", "lonely-a1b", "--timeout", "5"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
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

    // The program's reading: one object keyed by child id.
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
