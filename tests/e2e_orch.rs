//! Driving an agent from outside: waiting for its answer, sending it more
//! work, and answering the question it stopped on.
//!
//! These three verbs are the whole of amx's machine-facing surface, and the
//! exit code is what a caller reads. Every test here asserts the code first.

mod common;

use common::Harness;
use serde_json::json;
use std::process::{Output, Stdio};
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

/// Every wait in this file is bounded, so a verb that never returns fails the
/// test it is in rather than the whole suite.
fn result(amx: &Harness, id: &str) -> Output {
    amx.amx(&["result", id, "--timeout", "20"])
}

#[test]
fn result_waits_for_the_turn_to_end_and_prints_the_answer() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");

    // Asked straight away: the agent is still working, and this call is the
    // wait a caller would otherwise have written itself.
    let out = result(&amx, "fix-login-a1b");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "the tests pass now");
}

#[test]
fn result_hands_back_the_answer_of_an_agent_that_has_ended() {
    let amx = Harness::new();
    amx.play("say-hello-b2c", "finishes");
    amx.until_state("say-hello-b2c", "done");

    let out = result(&amx, "say-hello-b2c");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "hello");
}

#[test]
fn result_never_waits_through_a_question() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");

    // The question arrives while this call is already waiting.
    let out = result(&amx, "ask-a1b");
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Claude needs your permission"),
        "the question goes where the answer would have: {:?}",
        stdout(&out)
    );
    assert!(stderr(&out).contains("amx answer"), "{}", stderr(&out));
}

#[test]
fn result_says_when_no_answer_is_coming() {
    let amx = Harness::new();
    amx.play("port-importer-c3d", "fails");
    amx.until_state("port-importer-c3d", "failed");

    let out = result(&amx, "port-importer-c3d");
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out), "", "nothing on stdout is an answer");
    assert!(
        stderr(&out).contains("port-importer-c3d"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_turn_that_ended_with_nothing_captured_is_not_an_empty_answer() {
    let amx = Harness::new();
    amx.play("tidy-imports-d4e", "ends-without-an-answer");
    amx.until_state("tidy-imports-d4e", "done");

    let out = result(&amx, "tidy-imports-d4e");
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("no answer"), "{}", stderr(&out));
}

#[test]
fn result_gives_up_when_the_caller_says_when() {
    let amx = Harness::new();
    amx.play("watch-log-e5f", "works-without-end");
    amx.until_state("watch-log-e5f", "working");

    let out = amx.amx(&["result", "watch-log-e5f", "--timeout", "1"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn the_answer_from_before_a_send_is_not_the_answer_to_it() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    // This agent will never take the message — happy-turn has finished. What
    // matters is that the send is on the record before anybody asks again.
    let mut sending = amx
        .amx_command(&["send", "fix-login-a1b", "and now the linter"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running amx send");
    amx.until("the send to be recorded", || {
        (amx.state("fix-login-a1b")["seq"].as_u64().unwrap_or(0) > 0).then_some(())
    });

    let out = amx.amx(&["result", "fix-login-a1b", "--timeout", "2"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "",
        "the last turn's answer is not this send's"
    );
    assert_eq!(
        amx.state("fix-login-a1b")["result"],
        "the tests pass now",
        "and it is still on the record for whoever wants it"
    );
    let _ = sending.wait();
}

#[test]
fn send_confirms_that_the_agent_took_the_message_and_result_waits_for_its_answer() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["send", "fix-login-a1b", "and now the linter"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        amx.state("fix-login-a1b")["seq"].as_u64().unwrap_or(0) > 0,
        "the send is on the record"
    );

    let out = result(&amx, "fix-login-a1b");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "the linter is clean",
        "the answer to this send, not the one before it"
    );
}

#[test]
fn send_says_so_when_the_text_goes_nowhere() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["send", "fix-login-a1b", "and now the linter"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("did not start working"),
        "{}",
        stderr(&out)
    );
    assert!(
        amx.capture(&amx.pane_of("fix-login-a1b"))
            .contains("and now the linter"),
        "the text reached the pane; what did not happen is the agent taking it"
    );
}

#[test]
fn send_refuses_while_the_agent_is_waiting_on_a_question() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let out = amx.amx(&["send", "ask-a1b", "carry on"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Claude needs your permission"),
        "{:?}",
        stdout(&out)
    );
    assert!(
        !amx.capture(&amx.pane_of("ask-a1b")).contains("carry on"),
        "typing past a question would answer it by accident"
    );
    assert_eq!(
        amx.state("ask-a1b")["seq"].as_u64().unwrap_or(0),
        0,
        "and a refused send is not a send"
    );
}

#[test]
fn answer_types_the_key_the_question_is_waiting_for() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let out = amx.amx(&["answer", "ask-a1b", "9"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    amx.until("the key to reach the pane", || {
        amx.capture(&amx.pane_of("ask-a1b"))
            .contains('9')
            .then_some(())
    });
    assert_ne!(
        amx.state("ask-a1b")["state"],
        "waiting",
        "an answered question is not still pending, or the next caller answers it again"
    );
}

#[test]
fn answer_refuses_when_nothing_is_pending() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["answer", "fix-login-a1b", "9"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("nothing to answer"),
        "{}",
        stderr(&out)
    );
    assert!(
        !amx.capture(&amx.pane_of("fix-login-a1b")).contains('9'),
        "a key typed at an agent that is not asking lands in whatever it does next"
    );
}

#[test]
fn a_key_that_is_not_an_answer_never_reaches_the_agent() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let out = amx.amx(&["answer", "ask-a1b", "yes please"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("y, n, 1-9"), "{}", stderr(&out));
    assert!(
        !amx.capture(&amx.pane_of("ask-a1b")).contains("yes please"),
        "the grammar is checked before anything is typed"
    );
}

/// Park an agent in front of the permission box its scenario draws, with the
/// hooks put back far enough that a reader takes the choices off the screen.
///
/// The hook carried the words and nothing else — no hook has ever carried the
/// choices — so this is the only way the record gets both.
fn parked_on_the_box(amx: &Harness, id: &str) {
    amx.play(id, "asks-a-question");
    amx.until_state(id, "waiting");
    amx.until("the permission box to be drawn", || {
        amx.capture(&amx.pane_of(id))
            .contains("❯ 1. Yes")
            .then_some(())
    });
    amx.set_state(
        id,
        json!({
            "state": "waiting",
            "question": "Claude needs your permission to use Bash",
            "since": 1,
            "last_event": 1,
        }),
    );
}

/// Park an agent on a menu of the vendor's own, which is the one question that
/// takes words rather than a key.
///
/// The record is written straight out and left fresh, so the reading answers
/// from the hooks: what an `AskUserQuestion` hook leaves behind is the words,
/// the tool's own choices and the kind, and no screen is read over the top.
fn parked_on_a_menu(amx: &Harness, id: &str) {
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs();
    amx.set_state(
        id,
        json!({
            "state": "waiting",
            "question": {
                "text": "Which fixture should the port keep?",
                "options": ["the sqlite one", "the docker one"],
                "kind": "question",
            },
            "since": now,
            "last_event": now,
        }),
    );
}

#[test]
fn surfaces_result_prints_the_question_with_the_choices_under_it() {
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = result(&amx, "ask-a1b");
    assert_eq!(code(&out), 2, "{}", stderr(&out));

    let said = stdout(&out);
    assert!(
        said.contains("Claude needs your permission to use Bash"),
        "{said:?}"
    );
    assert!(said.contains("1. Yes"), "the choices go with it: {said:?}");
    assert!(said.contains("2. No"), "{said:?}");
}

#[test]
fn surfaces_send_says_a_question_of_the_vendors_own_will_take_words() {
    let amx = Harness::new();
    parked_on_a_menu(&amx, "pick-a1b");

    let out = amx.amx(&["send", "pick-a1b", "carry on"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Which fixture should the port keep?"),
        "{:?}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("1. the sqlite one"),
        "{:?}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("words"),
        "this one takes words of your own: {}",
        stderr(&out)
    );
}

#[test]
fn surfaces_status_prints_the_question_with_the_choices_under_it() {
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["status", "ask-a1b"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let said = stdout(&out);
    assert!(
        said.contains("Claude needs your permission to use Bash"),
        "{said:?}"
    );
    assert!(said.contains("1. Yes"), "{said:?}");
    assert!(said.contains("2. No"), "{said:?}");
    assert!(
        said.contains("amx answer ask-a1b"),
        "and what unblocks it: {said:?}"
    );
}

#[test]
fn the_orchestration_verbs_say_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    for args in [
        &["result", "never-made-abc"][..],
        &["send", "never-made-abc", "carry on"],
        &["answer", "never-made-abc", "y"],
    ] {
        let out = amx.amx(args);
        assert_eq!(code(&out), 1, "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("never-made-abc"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}
