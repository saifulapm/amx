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
    amx.play("b", "happy-turn");
    amx.until_state("b", "idle");
    amx.play("a", "finishes");
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
