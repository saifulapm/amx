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

/// The command a surface offers for the question an agent is waiting on, off
/// the line that says what answers it.
fn offered(said: &str) -> String {
    said.lines()
        .find_map(|line| line.trim().strip_prefix("answer "))
        .expect("the line that says what answers it")
        .trim()
        .to_string()
}

/// The keys an offer names, as a person reads them off it.
fn keys_offered(offer: &str) -> Vec<String> {
    offer
        .split_once('<')
        .expect("an offer says what it will take")
        .1
        .trim_end_matches('>')
        .split('|')
        .map(str::to_string)
        .collect()
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
fn surfaces_the_offer_runs_to_the_choices_that_were_read_off_the_screen() {
    // A box amx read two choices off is not answered by `7`, so a row offering
    // `1-9` at one is naming seven keys that do nothing to it.
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["status", "ask-a1b"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let offer = offered(&stdout(&out));
    assert!(offer.contains("1-2"), "{offer:?}");
    assert!(!offer.contains("1-9"), "{offer:?}");

    // And the same on a question of the vendor's own, where the digits run to
    // the choices it named and the words are the rest of the offer.
    parked_on_a_menu(&amx, "pick-a1b");
    let out = amx.amx(&["send", "pick-a1b", "carry on"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let offer = stderr(&out);
    assert!(offer.contains("<1-2|"), "{offer}");
    assert!(offer.contains("words"), "{offer}");
    assert!(!offer.contains("1-9"), "{offer}");
}

#[test]
fn surfaces_status_neutralises_the_task_it_quotes() {
    // The task is free text typed by whoever spawned the agent, and status
    // hands it to a terminal: an escape or a bidi override in it must arrive
    // neutralised, like every other word amx did not author.
    let amx = Harness::new();
    let id = "sly-task-a1b";
    amx.play(id, "works-without-end");

    let path = amx.agent_dir(id).join("meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    meta["task"] = serde_json::json!("fix\u{1b}]0;owned\u{7} the\u{202e}login");
    std::fs::write(&path, meta.to_string()).unwrap();

    let out = amx.amx(&["status", id]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let said = stdout(&out);
    assert!(!said.contains('\u{1b}'), "an escape reached the terminal");
    assert!(
        !said.contains('\u{202e}'),
        "a bidi override reached the terminal"
    );
    assert!(said.contains("fix"), "{said:?}");
}

#[test]
fn surfaces_the_table_carries_the_choices_beside_the_question() {
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["ls"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let row = stdout(&out);
    assert!(row.contains("Claude needs your permission"), "{row:?}");
    assert!(row.contains("1. Yes"), "{row:?}");
    assert!(row.contains("2. No"), "{row:?}");
    assert_eq!(row.lines().count(), 1, "and a row is still a row: {row:?}");
}

/// Park an agent on the vendor's own folder-trust gate, as claude 2.1.259
/// draws it: two choices, no number on either, and the cursor on the one that
/// ends the agent.
///
/// There are no hooks under this screen. The vendor puts it in front of a
/// session rather than inside one, so the pane is the whole of what a reader
/// has to go on and the record is what the reader writes off it.
fn parked_on_the_gate(amx: &Harness, id: &str) -> String {
    let pane = amx.play(id, "stops-on-trust");
    amx.until("the gate to be drawn", || {
        amx.capture(&pane)
            .contains("Enter to confirm")
            .then_some(())
    });
    pane
}

#[test]
fn surfaces_a_screen_that_numbers_nothing_is_answered_by_walking_to_the_row() {
    // docs/claude-screens.md, driven against a live 2.1.259 on 2026-09-05:
    // `1`, `2` and `y` do nothing at this gate, `n` and `enter` end the agent,
    // and the only thing that reaches `Yes, I trust this folder` is a walk
    // down and the key that takes what it lands on.
    let amx = Harness::new();
    parked_on_the_gate(&amx, "trusts-b2c");

    let out = amx.amx(&["answer", "trusts-b2c", "down enter"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let answered = amx
        .events("trusts-b2c")
        .into_iter()
        .find(|event| event["kind"] == "answer")
        .expect("the walk on the record");
    assert_eq!(
        answered["payload"]["key"], "Down Enter",
        "what amx typed is what the record keeps: {answered}"
    );
}

#[test]
fn surfaces_the_key_that_takes_the_highlighted_row_is_refused_where_none_is_numbered() {
    // The cursor opens on `No, exit`, so `enter` here is the key that ends the
    // agent, and a screen numbering none of its choices gives amx nothing to
    // see that with. Refusing it is what keeps the grammar off the row the
    // vendor happened to highlight.
    let amx = Harness::new();
    let pane = parked_on_the_gate(&amx, "trusts-b2c");

    let out = amx.amx(&["answer", "trusts-b2c", "enter"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("down enter"), "{}", stderr(&out));
    assert!(
        amx.capture(&pane).contains("Enter to confirm"),
        "the gate is still up, and the agent is still behind it"
    );
    assert!(
        !amx.event_kinds("trusts-b2c")
            .contains(&"answer".to_string()),
        "a refused key is not an answer, and the question is still there to be answered"
    );

    // A walk that takes nothing leaves the prompt standing, so it is not an
    // answer either: the walk and the take go in one line.
    let out = amx.amx(&["answer", "trusts-b2c", "down"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("down enter"), "{}", stderr(&out));
}

#[test]
fn surfaces_a_walk_leaves_the_screen_it_walked_answerable_again() {
    // The row a walk lands on carries no number, so nothing amx holds says
    // what the take took — and there are no hooks under this screen to say it
    // afterwards. A record moved to `working` off the keystroke alone would
    // have `answer` refuse the next caller while the gate is still on the pane,
    // and `status` read that same pane moments later and say `waiting` again.
    let amx = Harness::new();
    let pane = parked_on_the_gate(&amx, "trusts-b2c");

    let out = amx.amx(&["answer", "trusts-b2c", "down enter"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        amx.capture(&pane).contains("Enter to confirm"),
        "the gate is still up, and nothing amx heard says otherwise"
    );

    let said = stdout(&amx.amx(&["status", "trusts-b2c"]));
    assert!(said.contains("trusts-b2c  waiting"), "{said:?}");

    // And what status says is pending is what answer takes: the same screen,
    // answered again, rather than a caller told there is nothing to answer.
    let out = amx.amx(&["answer", "trusts-b2c", "down enter"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn surfaces_a_row_waiting_on_a_screen_that_numbers_nothing_is_offered_the_walk() {
    // The same gate, from the other side: what a person is told to type at it.
    // `1`, `2` and `y` do nothing there and `enter` ends the agent, so an offer
    // of `y|n|1-9|enter|esc` names eight keys, seven of which do nothing and
    // one of which is the exit.
    let amx = Harness::new();
    parked_on_the_gate(&amx, "trusts-b2c");

    let out = amx.amx(&["status", "trusts-b2c"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let offer = offered(&stdout(&out));
    assert!(offer.starts_with("amx answer trusts-b2c"), "{offer:?}");
    assert!(!offer.contains("1-9"), "no digit reaches a row: {offer:?}");
    assert!(offer.contains("down enter"), "{offer:?}");

    // And every key it does offer is one `amx answer` takes. An offer amx then
    // refuses is worse than none: it is the sentence a person reads before
    // they type.
    for (at, key) in keys_offered(&offer).iter().enumerate() {
        let id = format!("gate-{at}-a1b");
        parked_on_the_gate(&amx, &id);
        let out = amx.amx(&["answer", &id, key]);
        assert_eq!(code(&out), 0, "{key:?} was offered: {}", stderr(&out));
    }
}

#[test]
fn surfaces_answer_takes_words_where_the_vendor_asks_a_question_of_its_own() {
    let amx = Harness::new();
    parked_on_a_menu(&amx, "pick-a1b");

    let out = amx.amx(&["answer", "pick-a1b", "neither, keep both"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    amx.until("the words to reach the pane", || {
        amx.capture(&amx.pane_of("pick-a1b"))
            .contains("neither, keep both")
            .then_some(())
    });

    let recorded = amx.state("pick-a1b");
    assert_ne!(
        recorded["state"], "waiting",
        "an answered question is not still pending"
    );
    assert_eq!(
        recorded["question"],
        json!(null),
        "and it leaves neither its words nor its choices behind"
    );
}

#[test]
fn surfaces_answer_refuses_words_at_a_prompt_that_takes_a_key() {
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["answer", "ask-a1b", "neither, keep both"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("y, n, 1-9"), "{}", stderr(&out));
    assert!(
        !amx.capture(&amx.pane_of("ask-a1b"))
            .contains("neither, keep both"),
        "a permission box takes one key, and words typed at it answer it by accident"
    );
}

#[test]
fn surfaces_a_key_amx_cannot_see_the_effect_of_leaves_the_question_standing() {
    // `y` at a box is a key amx types and cannot check: this vendor says
    // nothing when a prompt is dismissed, and the screens it draws where the
    // key does nothing at all look exactly the same from here. So the record
    // keeps what amx typed and says nothing about what it did.
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["answer", "ask-a1b", "y"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        amx.state("ask-a1b")["state"],
        "waiting",
        "the box amx last saw is the box the record still says is up"
    );
    let typed = amx
        .events("ask-a1b")
        .pop()
        .expect("the answer on the record");
    assert_eq!(typed["payload"]["key"], "y", "{typed}");

    // So the same screen is answered again rather than refused.
    let out = amx.amx(&["answer", "ask-a1b", "1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // And a choice is the other half of it: amx knows what that answered, so
    // the question comes off the record and the agent is back at work.
    let recorded = amx.state("ask-a1b");
    assert_eq!(recorded["state"], "working", "{recorded}");
    assert_eq!(recorded["question"], json!(null), "{recorded}");
}

#[test]
fn surfaces_an_empty_answer_is_not_an_answer_to_anything() {
    let amx = Harness::new();
    parked_on_a_menu(&amx, "pick-a1b");

    let out = amx.amx(&["answer", "pick-a1b", "   "]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert_eq!(
        amx.state("pick-a1b")["state"],
        "waiting",
        "the question is still there to be answered"
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
