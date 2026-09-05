//! What amx says an agent is doing, and what it is going on when it says it.

mod common;

use common::Harness;
use serde_json::{Value, json};

fn ls(amx: &Harness) -> Vec<Value> {
    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the listing is json")
}

fn status(amx: &Harness, id: &str) -> Value {
    let out = amx.amx(&["status", id, "--json"]);
    assert!(
        out.status.success(),
        "amx status: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the status is json")
}

/// The AskUserQuestion menu, measured off claude v2.1.229 at 80 columns.
const A_MENU: &str = "\
────────────────────────────────────────────────────────────────────────────────
 ☐ Indentation

Should this project be indented with spaces or tabs?

❯ 1. Spaces
     Indent with spaces (most common default across JS/TS, PHP/Laravel, and
     Python codebases).
  2. Tabs
     Indent with tab characters, since each reader can set their own width.
  3. Type something.
────────────────────────────────────────────────────────────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

/// A pane with one of the vendor's blocking screens on it and nothing running
/// but a sleep.
///
/// claude draws the menu inside a turn it is running, which is not something
/// the stand-in can be made to reach; the screen itself is what a reader has to
/// work from, so the screen itself is what is put in front of it.
fn a_pane_showing(amx: &Harness, screen: &str) -> String {
    let word = format!("'{}'", screen.replace('\'', r"'\''"));
    amx.tmux(&[
        "new-session",
        "-d",
        // Wide enough that the pane does not wrap what the vendor already
        // wrapped, and no taller than the rows a rule may look at.
        "-x",
        "100",
        "-y",
        "24",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        &format!("printf '%s' {word}; sleep 600"),
    ])
}

#[test]
fn a_waiting_agents_question_reaches_the_record_with_the_answers_it_offers() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    let heard = amx.until_state("ask-a1b", "waiting");
    assert_eq!(
        heard["question"], "Claude needs your permission to use Bash",
        "the hook carries the words and nothing else, so that is all there is"
    );
    amx.until("the permission box to be drawn", || {
        amx.capture(&amx.pane_of("ask-a1b"))
            .contains("❯ 1. Yes")
            .then_some(())
    });

    // The hooks have gone quiet with the box still on the pane, which is when
    // a reader looks at the screen.
    amx.set_state(
        "ask-a1b",
        json!({
            "state": "waiting",
            "question": "Claude needs your permission to use Bash",
            "since": 1,
            "last_event": 1,
        }),
    );

    let agent = status(&amx, "ask-a1b");
    assert_eq!(agent["state"], "waiting");
    assert_eq!(agent["options"], json!(["Yes", "No"]));

    let recorded = amx.state("ask-a1b");
    assert_eq!(
        recorded["question"]["text"], "Claude needs your permission to use Bash",
        "the vendor's own words are not corrected by a reading of a picture"
    );
    assert_eq!(
        recorded["question"]["options"],
        json!(["Yes", "No"]),
        "and the keys that answer it, which no hook has ever carried"
    );
}

#[test]
fn a_menu_records_the_question_it_is_asking() {
    let amx = Harness::new();
    let text = "Should this project be indented with spaces or tabs?";
    let pane = a_pane_showing(&amx, A_MENU);
    amx.record("picks-a1b", &pane);
    amx.until("the menu to be drawn", || {
        amx.capture(&pane).contains("❯ 1.").then_some(())
    });

    // Nothing was ever heard from this one, so the screen is all there is and
    // the question on it is nobody's but the screen's.
    let agent = status(&amx, "picks-a1b");
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(agent["question"], text);

    let recorded = amx.state("picks-a1b");
    assert_eq!(recorded["question"]["text"], text);
    assert_eq!(
        recorded["question"]["options"],
        json!(["Spaces", "Tabs", "Type something.", "Chat about this"])
    );
}

#[test]
fn a_trust_gate_records_the_question_and_not_one_of_its_answers() {
    // The stand-in paints the gate the way 2.1.259 draws it: choices with no
    // numbers on them and the cursor opening on the exit. `Yes, I trust this
    // folder` is a row that reads like an answer, and the place the record
    // must not put it is where the question goes.
    let amx = Harness::new();
    let pane = amx.play("trusts-b2c", "stops-on-trust");
    amx.until("the gate to be drawn", || {
        amx.capture(&pane)
            .contains("Enter to confirm")
            .then_some(())
    });

    let text = "Quick safety check: Is this a project you created or one you trust? \
                (Like your own code, a well-known open source project, or work from \
                your team). If not, take a moment to review what's in this folder \
                first.";
    let agent = status(&amx, "trusts-b2c");
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(agent["rule"], "folder_trust");
    assert_eq!(agent["question"], text);
    assert_eq!(
        agent["options"],
        json!([]),
        "the choices a reader hands back are the numbered ones, and the vendor \
         numbers none of these"
    );

    // The words alone, which is how the record writes a question with no
    // choices under it.
    assert_eq!(amx.state("trusts-b2c")["question"], text);
}

#[test]
fn a_fresh_record_is_read_from_the_hooks() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let listed = ls(&amx);
    assert_eq!(listed.len(), 1);
    let agent = &listed[0];
    assert_eq!(agent["id"], "fix-login-a1b");
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["evidence"], "hooks");
    assert_eq!(agent["result"], "the tests pass now");
    assert!(agent["last_event"].as_u64().unwrap() > 0, "{agent}");
    assert!(agent["age"].as_u64().unwrap() < 8, "{agent}");
}

#[test]
fn the_listing_reads_as_a_table_when_nobody_asks_for_json() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let out = amx.amx(&["ls"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("waiting"), "{text}");
    assert!(text.contains("ask-a1b"), "{text}");
    assert!(
        text.contains("Claude needs your permission"),
        "the question is what somebody scanning the list needs: {text}"
    );
    assert_eq!(text.lines().count(), 1, "one agent, one row: {text}");
}

#[test]
fn a_record_that_has_gone_quiet_falls_back_to_the_screen() {
    // The pane is sitting at the vendor's idle prompt. The record says the
    // turn is still running, and nothing has been heard for a while.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    amx.set_state(
        "fix-login-a1b",
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    let agent = status(&amx, "fix-login-a1b");
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["evidence"], "screen");
    assert_eq!(agent["rule"], "idle_prompt");
    assert!(agent["age"].as_u64().unwrap() > 8, "{agent}");
}

#[test]
fn a_still_screen_does_not_end_a_turn_the_record_says_is_running() {
    // The same screen, with a turn outstanding: the idle screen and a
    // mid-turn pause are the same bytes, so one look decides nothing.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    amx.set_state(
        "fix-login-a1b",
        json!({ "state": "working", "since": 1, "last_event": 1 }),
    );

    let agent = status(&amx, "fix-login-a1b");
    assert_eq!(agent["state"], "working", "the record stands");
    assert_eq!(agent["evidence"], "hooks");
    assert_eq!(agent["rule"], "idle_prompt", "and says what it saw");

    let out = amx.amx(&["status", "fix-login-a1b"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("held still"), "{text}");
}

#[test]
fn a_screen_no_rule_claims_is_unknown_and_says_how_stale_it_is() {
    let amx = Harness::new();
    amx.play("nothing-known-b2c", "unrecognisable");
    amx.until("the pane to print something", || {
        amx.capture(&amx.pane_of("nothing-known-b2c"))
            .contains("$ ls")
            .then_some(())
    });
    amx.set_state(
        "nothing-known-b2c",
        json!({ "state": "working", "since": 1, "last_event": 1 }),
    );

    let agent = status(&amx, "nothing-known-b2c");
    assert_eq!(agent["state"], "unknown");
    assert_eq!(agent["evidence"], "unknown");
    assert!(agent["age"].as_u64().unwrap() > 8);

    let out = amx.amx(&["status", "nothing-known-b2c"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("no rule claims"), "{text}");
}

#[test]
fn an_agent_whose_pane_is_gone_reads_stopped() {
    let amx = Harness::new();
    let pane = amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    // Killed outright: no exit is recorded, because nothing got to record one.
    amx.tmux(&["kill-pane", "-t", &pane]);
    amx.until("the pane to leave the server", || {
        (!amx.pane_alive(&pane)).then_some(())
    });

    let agent = status(&amx, "fix-login-a1b");
    assert_eq!(agent["state"], "stopped");
    assert_eq!(agent["evidence"], "gone");
    assert_eq!(
        agent["result"], "the tests pass now",
        "and what it answered is still on the record"
    );
}

#[test]
fn an_agent_that_ended_is_read_from_the_record_alone() {
    let amx = Harness::new();
    amx.play("port-importer-c3d", "fails");
    amx.until_state("port-importer-c3d", "failed");

    let agent = status(&amx, "port-importer-c3d");
    assert_eq!(agent["state"], "failed");
    assert_eq!(agent["evidence"], "record");
    assert_eq!(agent["exit"], 2);
}

#[test]
fn the_json_carries_what_a_caller_branches_on() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let agent = &ls(&amx)[0];
    for field in [
        "id",
        "state",
        "evidence",
        "age",
        "since",
        "last_event",
        "seq",
        "summary",
        "question",
        "options",
        "result",
        "source",
        "exit",
        "task",
        "dir",
        "worktree",
        "branch",
        "pane",
        "socket",
        "created",
    ] {
        assert!(
            agent.get(field).is_some(),
            "{field} is part of the schema: {agent}"
        );
    }
    assert_eq!(status(&amx, "fix-login-a1b")["id"], agent["id"]);
}

#[test]
fn status_names_an_agent_that_is_not_there() {
    let amx = Harness::new();
    let out = amx.amx(&["status", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}

#[test]
fn the_listing_forgets_an_agent_that_ended_a_week_ago() {
    let amx = Harness::new();
    amx.record("old-done-a1b", "%404");
    amx.set_state(
        "old-done-a1b",
        json!({ "state": "done", "exit": 0, "last_event": 1 }),
    );
    amx.record("old-stopped-c3d", "%404");
    amx.set_state(
        "old-stopped-c3d",
        json!({ "state": "stopped", "last_event": 1 }),
    );

    let listed = ls(&amx);
    let ids: Vec<&str> = listed
        .iter()
        .map(|agent| agent["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["old-stopped-c3d"],
        "an agent somebody stopped keeps its record, and its branch with it"
    );
    assert!(!amx.agent_dir("old-done-a1b").exists());
}

#[test]
fn an_empty_machine_says_so() {
    let amx = Harness::new();
    let out = amx.amx(&["ls"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "no agents");
    assert_eq!(ls(&amx).len(), 0);
}
