//! `amx result`, `send` and `answer`: driving an agent from a script, and how
//! `status` and `ls` show the question it is waiting on.
//!
//! Callers branch on the exit code, so every test asserts it first.

mod common;

use common::{Harness, code, stderr, stdout};
use serde_json::json;
use std::process::{Output, Stdio};

/// `amx result` with a timeout, so a hang fails only the test it is in.
fn result(amx: &Harness, id: &str) -> Output {
    amx.amx(&["result", id, "--timeout", "20"])
}

/// The command on the `answer ...` line of `said`.
fn offered(said: &str) -> String {
    said.lines()
        .find_map(|line| line.trim().strip_prefix("answer "))
        .expect("the line that says what answers it")
        .trim()
        .to_string()
}

/// The keys in an offer's `<...>`, with a digit range such as `1-2` expanded
/// to one key per digit.
fn keys_offered(offer: &str) -> Vec<String> {
    offer
        .split_once('<')
        .expect("an offer says what it will take")
        .1
        .trim_end_matches('>')
        .split('|')
        .flat_map(each_key)
        .collect()
}

fn each_key(offered: &str) -> Vec<String> {
    let range = offered
        .split_once('-')
        .and_then(|(from, to)| Some((from.parse::<u32>().ok()?, to.parse::<u32>().ok()?)));
    match range {
        Some((from, to)) => (from..=to).map(|digit| digit.to_string()).collect(),
        None => vec![offered.to_string()],
    }
}

#[test]
fn result_waits_for_the_turn_to_end_and_prints_the_answer() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");

    // No `until_state` first: `result` itself waits out the turn.
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

    // happy-turn never takes the message. The test only needs the send
    // recorded before `result` runs.
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
fn send_takes_the_text_from_a_file() {
    // The pane gets the file's contents, not its path.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");

    let notes = amx.home().join("notes.md");
    std::fs::write(&notes, "and now the linter\n").expect("notes to send");
    let named = notes.to_string_lossy().into_owned();

    let out = amx.amx(&["send", "fix-login-a1b", "--file", &named]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        amx.capture(&amx.pane_of("fix-login-a1b"))
            .contains("and now the linter"),
        "the file's text is what the agent was given"
    );

    let out = result(&amx, "fix-login-a1b");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "the linter is clean");
}

#[test]
fn send_takes_the_text_from_stdin_for_a_bare_dash() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx_with_input(
        &["send", "fix-login-a1b", "--file", "-"],
        "and now the linter\n",
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        amx.events("fix-login-a1b")
            .iter()
            .find(|event| event["kind"] == "send")
            .map(|event| event["payload"]["text"].clone()),
        Some(json!("and now the linter")),
        "read whole, with the last newline off"
    );
}

#[test]
fn send_refuses_a_file_it_cannot_read_before_anything_reaches_the_pane() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");

    let empty = amx.home().join("empty.md");
    std::fs::write(&empty, "\n").expect("a file with nothing in it");
    let named = empty.to_string_lossy().into_owned();
    let missing = amx.home().join("nowhere.md").to_string_lossy().into_owned();

    // An empty file is an empty message. A missing file is named in the
    // error.
    let out = amx.amx(&["send", "fix-login-a1b", "--file", &named]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));

    let out = amx.amx(&["send", "fix-login-a1b", "--file", &missing]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("nowhere.md"), "{}", stderr(&out));

    // Text and `--file` together are ambiguous.
    let out = amx.amx(&["send", "fix-login-a1b", "carry on", "--file", &named]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));

    assert_eq!(
        amx.state("fix-login-a1b")["seq"].as_u64().unwrap_or(0),
        0,
        "and a refused send is not a send"
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

/// Park an agent on its scenario's permission box, with a stale record so the
/// reader takes the choices off the screen.
///
/// No hook carries the choices, only the question text.
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

/// Park an agent on the vendor's own `AskUserQuestion` menu, the one kind of
/// question that also takes free text.
///
/// The record is written fresh, as that hook leaves it (text, options and
/// kind), so the reader trusts it and does not read the screen.
fn parked_on_a_menu(amx: &Harness, id: &str) {
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    let now = common::now();
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
fn result_prints_the_question_with_the_choices_under_it() {
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
fn send_says_a_question_of_the_vendors_own_will_take_words() {
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
fn status_prints_the_question_with_the_choices_under_it() {
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
fn the_offer_runs_to_the_choices_that_were_read_off_the_screen() {
    // A box with two choices read off the screen ignores `7`, so the offer is
    // `1-2`, not `1-9`.
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["status", "ask-a1b"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let offer = offered(&stdout(&out));
    assert!(offer.contains("1-2"), "{offer:?}");
    assert!(!offer.contains("1-9"), "{offer:?}");

    // Same for a vendor menu: digits up to its options, plus words.
    parked_on_a_menu(&amx, "pick-a1b");
    let out = amx.amx(&["send", "pick-a1b", "carry on"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let offer = stderr(&out);
    assert!(offer.contains("<1-2|"), "{offer}");
    assert!(offer.contains("words"), "{offer}");
    assert!(!offer.contains("1-9"), "{offer}");
}

#[test]
fn status_neutralises_the_task_it_quotes() {
    // The task is untrusted text printed to a terminal, so escapes and bidi
    // overrides in it must be neutralised.
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
fn the_table_carries_the_choices_beside_the_question() {
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

/// Park an agent on claude 2.1.259's folder-trust gate: two unnumbered
/// choices, with the cursor on `No, exit`.
///
/// No hooks fire on this screen because it comes before the session starts,
/// so the reader has only the pane to go on.
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
fn a_screen_that_numbers_nothing_is_answered_by_walking_to_the_row() {
    // Per docs/claude-screens.md (claude 2.1.259): `1`, `2` and `y` do nothing
    // at this gate, `n` and `enter` exit, and only `down` then `enter` reaches
    // `Yes, I trust this folder`.
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
fn the_key_that_takes_the_highlighted_row_is_refused_where_none_is_numbered() {
    // The cursor opens on `No, exit`, so `enter` here exits. Where the vendor
    // numbers no row, amx refuses `enter` and offers the two rows it numbered
    // from the cursor glyph.
    let amx = Harness::new();
    let pane = parked_on_the_gate(&amx, "trusts-b2c");

    let out = amx.amx(&["answer", "trusts-b2c", "enter"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("press 1-2"), "{}", stderr(&out));
    assert!(
        amx.capture(&pane).contains("Enter to confirm"),
        "the gate is still up, and the agent is still behind it"
    );
    assert!(
        !amx.event_kinds("trusts-b2c")
            .contains(&"answer".to_string()),
        "a refused key is not an answer, and the question is still there to be answered"
    );

    // `down` alone answers nothing, so it is refused too: the walk and the
    // `enter` go in one call.
    let out = amx.amx(&["answer", "trusts-b2c", "down"]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).contains("down enter"), "{}", stderr(&out));
}

#[test]
fn a_walk_leaves_the_screen_it_walked_answerable_again() {
    // amx cannot tell which row a walk selected, and no hook fires on this
    // screen to say. Moving the record to `working` on the keystroke would make
    // `answer` refuse the next caller while `status` still reads the gate as
    // `waiting`.
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

    // `answer` accepts what `status` reports as pending.
    let out = amx.amx(&["answer", "trusts-b2c", "down enter"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn a_row_waiting_on_a_screen_that_numbers_nothing_is_offered_the_rows_amx_counted() {
    // amx numbers the gate's two rows itself from the cursor glyph, each digit
    // standing for the walk to its row. An offer of `y|n|1-9|enter|esc` would
    // list keys that do nothing here and one that exits.
    let amx = Harness::new();
    parked_on_the_gate(&amx, "trusts-b2c");

    let out = amx.amx(&["status", "trusts-b2c"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let offer = offered(&stdout(&out));
    assert!(offer.starts_with("amx answer trusts-b2c"), "{offer:?}");
    assert!(offer.contains("1-2"), "the rows it counted: {offer:?}");
    assert!(!offer.contains("1-9"), "and no digit past them: {offer:?}");

    // Every offered key must be one `amx answer` accepts.
    for (at, key) in keys_offered(&offer).iter().enumerate() {
        let id = format!("gate-{at}-a1b");
        parked_on_the_gate(&amx, &id);
        let out = amx.amx(&["answer", &id, key]);
        assert_eq!(code(&out), 0, "{key:?} was offered: {}", stderr(&out));
    }
}

#[test]
fn answer_takes_words_where_the_vendor_asks_a_question_of_its_own() {
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
fn answer_refuses_words_at_a_prompt_that_takes_a_key() {
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
fn a_key_amx_cannot_see_the_effect_of_leaves_the_question_standing() {
    // `enter` takes whichever row the cursor is on, and claude fires no hook
    // when a prompt is dismissed, so amx cannot tell what it did. The record
    // keeps the key and stays `waiting`.
    let amx = Harness::new();
    parked_on_the_box(&amx, "ask-a1b");

    let out = amx.amx(&["answer", "ask-a1b", "enter"]);
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
    assert_eq!(typed["payload"]["key"], "Enter", "{typed}");

    // claude's menu ignores letters, so `y` is sent as the number of the Yes
    // row. A numbered choice has a known effect: the question is cleared and
    // the agent is `working`.
    let out = amx.amx(&["answer", "ask-a1b", "y"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let typed = amx
        .events("ask-a1b")
        .pop()
        .expect("the answer on the record");
    assert_eq!(typed["payload"]["key"], "1", "{typed}");
    let recorded = amx.state("ask-a1b");
    assert_eq!(recorded["state"], "working", "{recorded}");
    assert_eq!(recorded["question"], json!(null), "{recorded}");
}

#[test]
fn an_empty_answer_is_not_an_answer_to_anything() {
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
