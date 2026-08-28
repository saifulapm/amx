//! The card: the closer look floated over the wall, and the answer typed on
//! it.
//!
//! Driven in a real tmux pane like the rest of the view, because what the card
//! draws over the list and what reaches the agent's own pane are questions a
//! pty answers and nothing else does.

mod common;

use common::Harness;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch seconds, for the records a test writes as though they had just
/// happened.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

/// An agent whose command has ended: no pane, and the record is the whole
/// story. `ago` is how long since it ended, which is what orders them.
fn finished(amx: &Harness, id: &str, state: &str, ago: u64) {
    let at = now() - ago;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": state,
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "did what it was asked",
        }),
    );
}

/// What is on the view's screen now.
fn screen(amx: &Harness, pane: &str) -> String {
    amx.capture(pane)
}

/// The same screen with the colours the view drew it in, as the escapes tmux
/// wrote them: what a bar is made of cannot be read off the text.
fn coloured(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-e", "-J", "-t", pane])
}

/// The line of the list holding `text`, escapes and all. The last of them,
/// because the header at the top says what there is in the same words the
/// headings under it do, and the list is the part the cursor walks.
fn coloured_line(amx: &Harness, view: &str, text: &str) -> String {
    let drawn = coloured(amx, view);
    drawn
        .lines()
        .rfind(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
        .to_string()
}

/// Wait for a view with nothing in it, which is the one line amx has for a
/// wall nobody has put anything on.
fn until_empty(amx: &Harness, view: &str) {
    amx.until("the empty view", || {
        screen(amx, view).contains("nobody asking").then_some(())
    });
}

/// Type a line at the view, as a person types one.
fn types(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["send-keys", "-t", view, "-l", text]);
}

fn press(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key]);
}

/// A pane showing exactly these rows and nothing else, where a real agent's
/// pane would be: the fixture screens the chrome cut is measured against, put
/// somewhere the view has to read them the way it reads any other pane.
fn a_pane_showing(amx: &Harness, rows: &[&str]) -> String {
    let drawn: String = rows.iter().map(|row| format!("{row}\\n")).collect();
    amx.tmux(&[
        "new-session",
        "-d",
        "-x",
        "60",
        "-y",
        "24",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        &format!("printf '{drawn}'; while :; do sleep 0.05; done"),
    ])
}

/// The five rows claude draws at the bottom of every pane it has the room
/// for, in the vendor's own order: the composer's top border with its
/// right-anchored label, whatever is staged in the box, the composer's bottom
/// border, the statusline, and the mode footer. Transcribed from a live
/// 2.1.237 on 2026-08-21.
const CHROME: [&str; 5] = [
    "──────────────────────────── execute amx-v2 ─",
    "❯ ",
    "─────────────────────────────────────────────",
    "  Opus 5 │ amx-main (main) │ xhigh",
    "  ⏵⏵ accept edits on (shift+tab to cycle)",
];

#[test]
fn card_floats_over_the_list_with_the_question_alone() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let carded = card_on(&amx, &view, "ask-a1b");

    // The question the agent stopped on — which is on its row as well, so the
    // card is the second of the two. The pane it stopped on is not under it:
    // the prompt there is the same question behind an echo of the command,
    // and every row of it is noise below the answer line.
    assert_eq!(
        carded.matches("Claude needs your permission").count(),
        2,
        "the row asks it and the card asks it again:\n{carded}"
    );
    assert!(
        !carded.contains("rm -rf build") && !carded.contains("Do you want to proceed?"),
        "the question block is the whole of the card:\n{carded}"
    );

    // A box, with the list it was opened from still drawn above it.
    let top = carded
        .lines()
        .find(|line| line.trim_start().starts_with('╭'))
        .unwrap_or_else(|| panic!("no card in:\n{carded}"));
    assert!(top.contains("ask-a1b · waiting"), "{top}");
    assert!(top.trim_end().ends_with('╮'), "{top}");
    assert!(
        carded.lines().any(|line| line.trim_end().ends_with('╯')),
        "{carded}"
    );
    assert!(
        carded
            .lines()
            .next()
            .unwrap_or_default()
            .starts_with("AMX  "),
        "the header is where it was:\n{carded}"
    );
    assert!(
        carded.contains("NEEDS INPUT"),
        "and so is the group the row is under:\n{carded}"
    );

    // Esc puts it away and leaves the wall as it was.
    press(&amx, &view, "Escape");
    amx.until("the card to go", || {
        (!screen(&amx, &view)
            .lines()
            .any(|line| line.trim_start().starts_with('╭')))
        .then_some(())
    });
}

#[test]
fn card_tail_cuts_the_chrome_claude_draws_under_its_pane() {
    let amx = Harness::new();
    let mut pane_rows = vec![
        "i ported the importer",
        "",
        "✻ Nesting… (15s · still thinking)",
        "",
    ];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("port-cli-b2c").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("i ported the importer").then_some(drawn)
    });
    for furniture in ["accept edits on", "execute amx-v2", "amx-main (main)"] {
        assert!(
            !carded.contains(furniture),
            "{furniture} is claude's, not the agent's:\n{carded}"
        );
    }
    // The spinner line is claude's too and is cut from the card with the rest
    // of the chrome. It is on the row by then, because a working row says what
    // the vendor's own line says it is doing, so once on the screen is the
    // whole of it and twice would be the card drawing it again.
    assert_eq!(
        carded.matches("still thinking").count(),
        1,
        "the row says it and the card does not:\n{carded}"
    );
}

#[test]
fn card_tail_cuts_a_composer_a_message_was_typed_into() {
    let amx = Harness::new();
    // Wrapped over three rows, because a composer holding one row of text is
    // the state a walk that cut exactly one input row would pass.
    let mut pane_rows = vec!["i ported the importer", CHROME[0]];
    pane_rows.extend_from_slice(&[
        "❯ and now check every call site that",
        "  used to take the old shape, then",
        "  run the suite",
    ]);
    pane_rows.extend_from_slice(&CHROME[2..]);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("port-cli-b2c").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("i ported the importer").then_some(drawn)
    });
    for staged in ["check every call site", "the old shape", "run the suite"] {
        assert!(
            !carded.contains(staged),
            "a line somebody is part way through typing is not the tail:\n{carded}"
        );
    }
    assert!(!carded.contains("accept edits on"), "{carded}");
}

#[test]
fn card_tail_keeps_the_paint_the_agent_drew_its_screen_in() {
    let amx = Harness::new();
    // A row claude wrote in bold and in a colour of its own, which is what a
    // heading in one of its answers looks like on a real pane.
    let mut pane_rows = vec!["\\033[1m\\033[38;5;208mI ported the importer\\033[0m"];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("port-cli-b2c").then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("I ported the importer").then_some(drawn)
    });
    assert!(
        !carded.contains("38;5;208"),
        "no escape reaches the screen as text:\n{carded}"
    );

    let painted = coloured_line(&amx, &view, "I ported the importer");
    assert!(
        painted.contains("38;5;208"),
        "the colour claude chose is the colour the card draws it in:\n{painted:?}"
    );
    assert!(
        painted.contains("\u{1b}[1m"),
        "and so is the weight:\n{painted:?}"
    );
}

#[test]
fn card_answers_the_question_the_agent_stopped_on() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    // One key: the card is where somebody decides what to answer and where
    // they type it.
    press(&amx, &view, "Space");
    amx.until("the card, with a line to answer on", || {
        let drawn = screen(&amx, &view);
        (drawn.matches("Claude needs your permission").count() == 2 && drawn.contains('❯'))
            .then_some(())
    });

    // A key of the grammar that is nowhere on the agent's own screen, so what
    // reaches the pane is what was typed here and not what was already drawn.
    types(&amx, &view, "9");
    amx.until("the key on the line", || {
        screen(&amx, &view).contains("❯ 9").then_some(())
    });
    press(&amx, &view, "Enter");

    amx.until("the key to reach the agent's pane", || {
        amx.capture(&amx.pane_of("ask-a1b"))
            .contains('9')
            .then_some(())
    });
    amx.until("the question to stop being pending", || {
        (amx.state("ask-a1b")["state"] != "waiting").then_some(())
    });
}

/// Park an agent on a menu of the vendor's own: the one question that takes
/// words as well as a key, with the choices it drew recorded beside it.
fn parked_on_a_menu(amx: &Harness, id: &str) {
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    amx.set_state(
        id,
        json!({
            "state": "waiting",
            "question": {
                "text": "Which fixture should the port keep?",
                "options": ["the sqlite one", "the docker one"],
                "kind": "question",
            },
            "since": now(),
            "last_event": now(),
        }),
    );
}

/// Park an agent on a whole call of the vendor's own: every question it holds,
/// the sentences under their choices and the flag saying how many may be
/// taken, as a hook that carried the payload leaves them.
///
/// The question showing is the first with no answer on it, and it goes where
/// every reader has always found the question — which is what `asks_all` does
/// in the record's own code, done here by hand because a test writes the
/// document rather than the struct.
fn parked_on_a_call(amx: &Harness, id: &str, asking: &[Value]) {
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    showing_the_pending_one(amx, id, asking);
}

/// Write the call down with the question it is showing where the question
/// goes, without starting anything: the record moving on under a card that is
/// already open.
fn showing_the_pending_one(amx: &Harness, id: &str, asking: &[Value]) {
    let pending = asking
        .iter()
        .find(|ask| ask["answer"].is_null())
        .expect("a question with nothing on it");
    let options: Vec<Value> = pending["options"]
        .as_array()
        .expect("the choices under it")
        .iter()
        .map(|choice| choice["label"].clone())
        .collect();

    amx.set_state(
        id,
        json!({
            "state": "waiting",
            "question": {
                "text": pending["text"],
                "options": options,
                "kind": "question",
                "asking": asking,
            },
            "since": now(),
            "last_event": now(),
        }),
    );
}

/// The three questions of one `AskUserQuestion` call, as the payload measured
/// against claude 2.1.240 on 2026-08-24 records them in
/// `docs/question-shapes.md`: a question that takes one choice, a checkbox
/// question behind it, and a third that takes one.
fn a_call_of_three() -> Vec<Value> {
    vec![
        json!({
            "header": "Runtime",
            "text": "Which runtime should the service target?",
            "options": [
                { "label": "Node", "description": "Widest library support" },
                { "label": "Deno", "description": "Batteries included" },
            ],
            "multi": false,
        }),
        json!({
            "header": "Rollout",
            "text": "Which rollout steps should run?",
            "options": [
                { "label": "Canary", "description": "Five percent first" },
                { "label": "Migrate", "description": "Run the schema change" },
                { "label": "Announce", "description": "Post to the channel" },
            ],
            "multi": true,
        }),
        json!({
            "header": "Storage",
            "text": "Which store should hold sessions?",
            "options": [
                { "label": "Redis", "description": "Fast, volatile" },
                { "label": "Postgres", "description": "Durable, already deployed" },
            ],
            "multi": false,
        }),
    ]
}

/// The one question that carries a preview beside each choice, from the same
/// measurement: the layout the vendor draws a notes field for and no row for
/// words of your own.
fn a_previewed_question() -> Vec<Value> {
    vec![json!({
        "header": "Layout",
        "text": "Which header layout should the page use?",
        "options": [
            {
                "label": "Stacked",
                "description": "Title over subtitle",
                "preview": "+----------+\n| TITLE    |\n+----------+",
            },
            {
                "label": "Inline",
                "description": "Title beside subtitle",
                "preview": "+---------------------+\n| TITLE - subtitle    |\n+---------------------+",
            },
        ],
        "multi": false,
    })]
}

/// The vendor's own checkbox menu, row for row as claude 2.1.240 draws one
/// (`docs/question-shapes.md` § 1): the full-width rule the box opens with,
/// the tab strip, the question, the boxes with their descriptions, the two rows
/// no payload carries, and the footer every question screen ends in.
const MENU: [&str; 17] = [
    "────────────────────────────────────────────────────────────",
    "←  ☐ Features  ✔ Submit  →",
    "",
    "Which features should be enabled?",
    "",
    "❯ 1. [ ] Logging",
    "  Write a log file",
    "  2. [ ] Metrics",
    "  Export counters",
    "  3. [ ] Tracing",
    "  Emit spans",
    "  4. [ ] Type something",
    "     Submit",
    "────────────────────────────────────────────────────────────",
    "  5. Chat about this",
    "",
    "Enter to select · ↑/↓ to navigate · Esc to cancel",
];

/// The call that drew it, as the tool made it: one question, three choices and
/// the flag that says the boxes take more than one.
fn the_menus_call() -> Vec<Value> {
    vec![json!({
        "header": "Features",
        "text": "Which features should be enabled?",
        "options": [
            { "label": "Logging", "description": "Write a log file" },
            { "label": "Metrics", "description": "Export counters" },
            { "label": "Tracing", "description": "Emit spans" },
        ],
        "multi": true,
    })]
}

/// An agent standing at that menu: the screen live on a pane of its own, with
/// what it said before it asked above the box, and the call on its record.
fn parked_at_a_live_menu(amx: &Harness, id: &str) -> String {
    let mut rows = vec!["i wired the logging up", ""];
    rows.extend_from_slice(&MENU);
    let pane = a_pane_showing(amx, &rows);
    amx.record(id, &pane);
    showing_the_pending_one(amx, id, &the_menus_call());
    pane
}

/// The card, opened on the agent the view is holding the cursor over.
fn card_on(amx: &Harness, view: &str, id: &str) -> String {
    amx.until("the row", || screen(amx, view).contains(id).then_some(()));
    press(amx, view, "Space");
    amx.until("the card", || {
        let drawn = screen(amx, view);
        drawn
            .lines()
            .any(|line| line.trim_start().starts_with('╭'))
            .then_some(drawn)
    })
}

#[test]
fn card_says_which_question_of_the_call_it_is_showing() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    // A call of three questions. Nothing on the pane says how many there are:
    // the vendor's tab strip elides its own headers as the pane narrows, so
    // this is amx saying what only the payload knows.
    let mut call = a_call_of_three();
    parked_on_a_call(&amx, "pick-a1b", &call);
    let carded = card_on(&amx, &view, "pick-a1b");
    assert!(
        carded.contains("Runtime · 1 of 3"),
        "which question of the call, and what its tab is called:\n{carded}"
    );

    // Answering one does not end the call: the vendor moves to the tab after
    // it and the prompt is still up, so the card moves with it.
    call[0]["answer"] = json!("Node");
    showing_the_pending_one(&amx, "pick-a1b", &call);
    let moved = amx.until("the card to move to the tab behind it", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Rollout · 2 of 3").then_some(drawn)
    });
    assert!(
        moved.contains("Which rollout steps should run?"),
        "with the question that tab is asking:\n{moved}"
    );
    assert!(
        !moved.contains("1 of 3"),
        "and one question of it on the card at a time:\n{moved}"
    );
}

#[test]
fn card_draws_a_box_beside_the_choices_of_a_question_that_takes_several() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    let mut call = a_call_of_three();
    call[0]["answer"] = json!("Node");
    parked_on_a_call(&amx, "pick-a1b", &call);

    let carded = card_on(&amx, &view, "pick-a1b");
    for choice in ["1. [ ] Canary", "2. [ ] Migrate", "3. [ ] Announce"] {
        assert!(
            carded.contains(choice),
            "{choice} is a box to check, not a choice to make:\n{carded}"
        );
    }
    assert!(
        carded.contains("1,3 for several"),
        "and the line says how to check more than one:\n{carded}"
    );

    // The question in front of it takes one choice, and a box beside those
    // would be a screen offering something the vendor will not take.
    showing_the_pending_one(&amx, "pick-a1b", &a_call_of_three());
    let drawn = amx.until("the card on the plain question", || {
        let drawn = screen(&amx, &view);
        drawn.contains("1. Node").then_some(drawn)
    });
    assert!(!drawn.contains("[ ] Node"), "{drawn}");
    assert!(
        !drawn.contains("for several"),
        "and the line offers what this one takes:\n{drawn}"
    );
}

#[test]
fn card_names_the_rows_the_vendor_adds_that_no_payload_carries() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    // Every menu the tool draws carries one free-text row as its last choice,
    // and nothing in the payload accounts for it.
    parked_on_a_call(&amx, "pick-a1b", &a_call_of_three());
    let carded = card_on(&amx, &view, "pick-a1b");
    assert!(
        carded.contains("words of your own"),
        "the row the vendor adds under the choices:\n{carded}"
    );

    // A previewed question has no such row, and has a field for a note
    // instead.
    showing_the_pending_one(&amx, "pick-a1b", &a_previewed_question());
    let previewed = amx.until("the card on the previewed question", || {
        let drawn = screen(&amx, &view);
        drawn.contains("1. Stacked").then_some(drawn)
    });
    assert!(
        previewed.contains("field for a note"),
        "the field the vendor draws where a choice carries a preview:\n{previewed}"
    );
    assert!(
        !previewed.contains("words of your own"),
        "and that layout draws no free-text row at all:\n{previewed}"
    );
}

#[test]
fn card_takes_words_where_the_question_asks_for_them() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    parked_on_a_menu(&amx, "pick-a1b");
    amx.until("the question on its row", || {
        screen(&amx, &view)
            .contains("Which fixture should the port keep?")
            .then_some(())
    });

    press(&amx, &view, "Space");
    let carded = amx.until("the card, with the choices numbered", || {
        let drawn = screen(&amx, &view);
        drawn.contains("1. the sqlite one").then_some(drawn)
    });
    assert!(carded.contains("2. the docker one"), "{carded}");
    assert_eq!(
        carded
            .matches("Which fixture should the port keep?")
            .count(),
        2,
        "the row asks it and the card asks it again:\n{carded}"
    );

    // Words of somebody's own, which is what this one question takes.
    types(&amx, &view, "neither, keep both");
    amx.until("the answer on the line", || {
        screen(&amx, &view)
            .contains("❯ neither, keep both")
            .then_some(())
    });
    press(&amx, &view, "Enter");

    amx.until("the words to reach the agent's pane", || {
        amx.capture(&amx.pane_of("pick-a1b"))
            .contains("neither, keep both")
            .then_some(())
    });
    amx.until("the question to be answered rather than repeated", || {
        (amx.state("pick-a1b")["question"] == json!(null)).then_some(())
    });
}

/// The last answer amx wrote down for this agent, once it has written one.
fn answered(amx: &Harness, id: &str) -> Value {
    amx.until(&format!("{id} to be answered"), || {
        amx.events(id)
            .into_iter()
            .rfind(|event| event["kind"] == "answer")
    })["payload"]
        .clone()
}

#[test]
fn card_answers_the_tab_it_is_showing_and_leaves_the_one_behind_it_standing() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    parked_on_a_call(&amx, "pick-a1b", &a_call_of_three());

    let carded = card_on(&amx, &view, "pick-a1b");
    assert!(carded.contains("Runtime · 1 of 3"), "{carded}");
    types(&amx, &view, "1");
    press(&amx, &view, "Enter");

    assert_eq!(
        answered(&amx, "pick-a1b")["key"],
        "1",
        "the key the vendor takes for that choice"
    );
    // Answering one question of a call does not end it: the vendor records the
    // answer, moves to the tab after it, and the prompt is still up.
    let moved = amx.until("the card to move to the tab behind it", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Rollout · 2 of 3").then_some(drawn)
    });
    assert!(
        moved.contains("1. [ ] Canary"),
        "with the choices that tab offers, in the shape it offers them:\n{moved}"
    );
    assert!(
        moved.contains('❯'),
        "and the line to answer it on came back by itself, the card never \
         having been closed:\n{moved}"
    );
    assert_eq!(
        amx.state("pick-a1b")["state"],
        "waiting",
        "and the agent is still waiting on the rest of the call"
    );
}

#[test]
fn card_checks_the_boxes_of_a_question_that_takes_more_than_one() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    let mut call = a_call_of_three();
    call[0]["answer"] = json!("Node");
    parked_on_a_call(&amx, "pick-a1b", &call);

    let carded = card_on(&amx, &view, "pick-a1b");
    assert!(carded.contains("1. [ ] Canary"), "{carded}");
    types(&amx, &view, "1,3");
    press(&amx, &view, "Enter");

    // Two boxes and the key that leaves the choices, which is the only way off
    // them: on this shape every digit and every enter is a toggle.
    let said = answered(&amx, "pick-a1b");
    assert_eq!(said["key"], "1,3");
    assert_eq!(
        said["answer"], "Canary, Announce",
        "written down the way the vendor's own answer map writes it: the \
         labels, in the order the boxes were checked"
    );
    amx.until("the card to move to the tab behind it", || {
        screen(&amx, &view)
            .contains("Storage · 3 of 3")
            .then_some(())
    });
}

#[test]
fn card_over_a_live_menu_draws_the_question_once() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    parked_at_a_live_menu(&amx, "pick-a1b");

    let carded = card_on(&amx, &view, "pick-a1b");
    assert_eq!(
        carded.matches("Which features should be enabled?").count(),
        2,
        "the row asks it and the card asks it again, and the menu under the \
         card is not a third:\n{carded}"
    );
    for choice in ["1. [ ] Logging", "2. [ ] Metrics", "3. [ ] Tracing"] {
        assert_eq!(
            carded.matches(choice).count(),
            1,
            "{choice} is on the card, and the pane's own copy of it is not:\n{carded}"
        );
    }

    // The rows the vendor adds are the vendor's, and the card names them in its
    // own words rather than drawing the menu they are on.
    for furniture in ["Chat about this", "Enter to select", "☐ Features"] {
        assert!(
            !carded.contains(furniture),
            "{furniture} is the menu's furniture:\n{carded}"
        );
    }
    assert!(
        carded.contains("words of your own"),
        "with the free-text row named:\n{carded}"
    );

    // And nothing of the pane at all, what the agent said before it asked
    // included: the question block is the whole of the card, and the work is
    // still on the pane for the enter that brings it forward.
    assert!(
        !carded.contains("i wired the logging up"),
        "the card spends no window on the pane:\n{carded}"
    );
}

#[test]
fn card_answers_a_live_checkbox_menu_in_the_grammar_the_screen_takes() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    let pane = parked_at_a_live_menu(&amx, "pick-a1b");
    // An older amx wrote `permission` over every menu it saw, and records
    // outlive the amx that wrote them. What the screen is showing is a menu,
    // and a permission box's one key at a menu answers a question nobody chose.
    let mut state = amx.state("pick-a1b");
    state["question"]["kind"] = json!("permission");
    amx.set_state("pick-a1b", state);

    let carded = card_on(&amx, &view, "pick-a1b");
    assert!(
        carded.contains("press 1-3, 1,3 for several"),
        "the boxes are checked by naming them:\n{carded}"
    );
    assert!(
        !carded.contains("y or n"),
        "and this screen has no y and no n on it:\n{carded}"
    );

    types(&amx, &view, "1,3");
    press(&amx, &view, "Enter");

    let said = answered(&amx, "pick-a1b");
    assert_eq!(said["key"], "1,3");
    assert_eq!(
        said["answer"], "Logging, Tracing",
        "the labels, in the order the boxes were checked"
    );
    amx.until("the digits to reach the menu", || {
        amx.capture(&pane).contains("13").then_some(())
    });
}

#[test]
fn card_sends_the_note_the_vendor_lets_an_answer_ride_beside() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    parked_on_a_call(&amx, "pick-a1b", &a_previewed_question());

    let carded = card_on(&amx, &view, "pick-a1b");
    assert!(
        carded.contains("press 1-2, and words after it are a note"),
        "that layout has no row for words of your own, so words on the line \
         can only be the note:\n{carded}"
    );

    types(&amx, &view, "1 prefer the stacked one");
    press(&amx, &view, "Enter");

    let said = answered(&amx, "pick-a1b");
    assert_eq!(said["key"], "1", "the choice the note rides beside");
    assert_eq!(said["note"], "prefer the stacked one");
    amx.until("the note to reach the agent's pane", || {
        amx.capture(&amx.pane_of("pick-a1b"))
            .contains("prefer the stacked one")
            .then_some(())
    });
    amx.until("the call to be answered rather than repeated", || {
        (amx.state("pick-a1b")["question"] == json!(null)).then_some(())
    });
}

#[test]
fn card_refuses_words_at_a_prompt_that_reads_one_key() {
    let amx = Harness::new();
    let pane = amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    press(&amx, &view, "Space");
    amx.until("the card", || {
        (screen(&amx, &view)
            .matches("Claude needs your permission")
            .count()
            == 2)
            .then_some(())
    });
    types(&amx, &view, "neither, keep both");
    press(&amx, &view, "Enter");

    // The verb's own refusal, in the verb's own words: the card and a shell
    // prompt are two callers reading one line against one record.
    let refused = amx.until("the refusal", || {
        let drawn = screen(&amx, &view);
        drawn.contains("is not an answer").then_some(drawn)
    });
    assert!(
        refused.contains("use y, n, 1-9, enter or esc"),
        "with what this prompt would have taken:\n{refused}"
    );
    assert!(
        refused.contains("❯ neither, keep both"),
        "a line the question would not take is a line somebody is still \
         writing:\n{refused}"
    );
    assert!(
        !amx.capture(&pane).contains("neither, keep both"),
        "and a permission box reads one key, so words typed at it would \
         answer it by accident"
    );
    assert_eq!(
        amx.state("ask-a1b")["state"],
        "waiting",
        "with the question still pending"
    );
}

#[test]
fn d_shows_what_the_agent_has_changed() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            "fix-login-a1b",
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let tree = PathBuf::from(
        amx.meta("fix-login-a1b")["worktree"]
            .as_str()
            .expect("a worktree"),
    );
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    types(&amx, &view, "d");
    let shown = amx.until("the diff", || {
        let drawn = screen(&amx, &view);
        drawn.contains("+after").then_some(drawn)
    });
    assert!(shown.contains("-before"), "{shown}");
    assert!(
        shown.contains("what it has changed"),
        "and the panel says what it is showing: {shown}"
    );
}

#[test]
fn page_keys_page_a_long_diff_and_the_frame_says_how_far() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            "fix-login-a1b",
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // A patch far taller than the card: forty added lines against a box of
    // about ten rows.
    let tree = PathBuf::from(
        amx.meta("fix-login-a1b")["worktree"]
            .as_str()
            .expect("a worktree"),
    );
    let long: String = (0..40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(tree.join("README.md"), long).expect("the changed file");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    // The card opens on the top of the patch, with nothing said to be hidden.
    types(&amx, &view, "d");
    let top = amx.until("the diff", || {
        let drawn = screen(&amx, &view);
        drawn.contains("+line 0").then_some(drawn)
    });
    assert!(!top.contains("more"), "{top}");

    // A page forward leaves the top, and the card's own frame says how far.
    press(&amx, &view, "NPage");
    let paged = amx.until("the paged diff", || {
        let drawn = screen(&amx, &view);
        drawn.contains("more").then_some(drawn)
    });
    assert!(!paged.contains("+line 0"), "the top is behind: {paged}");
    let saying = paged
        .lines()
        .find(|line| line.contains("more"))
        .expect("the indicator");
    assert!(
        saying.contains('↑') && saying.contains('╰'),
        "on the bottom border, pointing at the top: {paged}"
    );

    // A page back is the top again, with the indicator gone.
    press(&amx, &view, "PPage");
    let back = amx.until("the top again", || {
        let drawn = screen(&amx, &view);
        drawn.contains("+line 0").then_some(drawn)
    });
    assert!(!back.contains("more"), "{back}");

    // Paged away, `d` takes the patch afresh from its top.
    press(&amx, &view, "NPage");
    amx.until("the paged diff", || {
        screen(&amx, &view).contains("more").then_some(())
    });
    types(&amx, &view, "d");
    let taken = amx.until("the fresh patch", || {
        let drawn = screen(&amx, &view);
        drawn.contains("+line 0").then_some(drawn)
    });
    assert!(!taken.contains("more"), "{taken}");
}

#[test]
fn page_keys_leave_a_fitting_card_alone_and_the_arrows_still_walk() {
    let amx = Harness::new();
    finished(&amx, "short-a1b", "done", 10);
    amx.record("tall-b2c", "%404");
    let at = now() - 100;
    amx.set_state(
        "tall-b2c",
        json!({
            "state": "done",
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": (0..40).map(|n| format!("said {n}\n")).collect::<String>(),
        }),
    );

    let view = amx.in_a_terminal(&[], &[]);
    card_on(&amx, &view, "short-a1b");

    // Two pages up on a body that fits, then a round trip through the keys
    // overlay: the overlay coming and going proves both presses were read
    // before the frame this asserts on.
    press(&amx, &view, "PPage");
    press(&amx, &view, "PPage");
    press(&amx, &view, "?");
    amx.until("the keys", || {
        screen(&amx, &view).contains("page the card").then_some(())
    });
    press(&amx, &view, "Escape");
    let unmoved = amx.until("the card, unmoved", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("short-a1b · done") && !drawn.contains("page the card")).then_some(drawn)
    });
    assert!(unmoved.contains("did what it was asked"), "{unmoved}");
    assert!(!unmoved.contains("more"), "nothing is hidden: {unmoved}");

    // The arrows keep walking the list, card in tow: the next agent's
    // recorded answer opens on its first words, over the row whose summary
    // is that same first line.
    press(&amx, &view, "Down");
    amx.until("the next card", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("tall-b2c · done") && drawn.matches("said 0").count() == 2).then_some(())
    });

    // This body overflows, so the page key now pages it, down from its top.
    press(&amx, &view, "NPage");
    let paged = amx.until("the paged card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("more").then_some(drawn)
    });
    assert_eq!(
        paged.matches("said 0").count(),
        1,
        "the top is behind, and only the row still says it: {paged}"
    );
    let saying = paged
        .lines()
        .find(|line| line.contains("more"))
        .expect("the indicator");
    assert!(
        saying.contains('↑') && saying.contains('╰'),
        "on the bottom border, pointing at the top: {paged}"
    );

    // And walking off the agent puts the next card on its own edge.
    press(&amx, &view, "Up");
    let followed = amx.until("the first card again", || {
        let drawn = screen(&amx, &view);
        drawn.contains("short-a1b · done").then_some(drawn)
    });
    assert!(!followed.contains("more"), "{followed}");
}

#[test]
fn ctrl_f_and_ctrl_b_page_the_card_like_the_page_keys() {
    let amx = Harness::new();
    amx.record("tall-b2c", "%404");
    let at = now() - 100;
    amx.set_state(
        "tall-b2c",
        json!({
            "state": "done",
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": (0..40).map(|n| format!("said {n}\n")).collect::<String>(),
        }),
    );

    let view = amx.in_a_terminal(&[], &[]);
    let carded = card_on(&amx, &view, "tall-b2c");
    assert_eq!(
        carded.matches("said 0").count(),
        2,
        "the top of the answer, over the row saying the same:\n{carded}"
    );

    // ctrl+f is pgdn: on into the recorded answer, with the how-far
    // indicator up.
    press(&amx, &view, "C-f");
    let paged = amx.until("the paged card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("more").then_some(drawn)
    });
    assert_eq!(
        paged.matches("said 0").count(),
        1,
        "the top is behind, and only the row still says it:\n{paged}"
    );

    // And ctrl+b is pgup: the page back to the edge. A lone ctrl+b never
    // reaches a view inside a default tmux — the prefix eats it — but
    // injected keys go to the pane, which is exactly what ctrl+b ctrl+b
    // delivers there.
    press(&amx, &view, "C-b");
    let back = amx.until("the edge again", || {
        let drawn = screen(&amx, &view);
        (drawn.matches("said 0").count() == 2).then_some(drawn)
    });
    assert!(!back.contains("more"), "{back}");
}
