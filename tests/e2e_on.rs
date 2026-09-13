//! The command a moment runs.
//!
//! `on_waiting`, `on_idle`, `on_done` and `on_stopped` name a command apiece,
//! and which amx runs one is which amx wrote the phase: the hook that recorded
//! the vendor's event, or the `stop` that ended the agent. Only here is the
//! whole of that true — the config is read by a hook in a real pane, the phase
//! reaches the command in its environment, and the event that moved the agent
//! arrives on its stdin.

mod common;

use common::Harness;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A machine where one moment runs a command that writes down what it was
/// handed: the phase and whether anybody was looking on one line, and the event
/// it was given on stdin under that.
fn writes_down(amx: &Harness, key: &str) -> PathBuf {
    let said = amx.home().join(format!("said-{key}"));
    amx.config(&format!(
        "{key} = \"{{ echo $AMX_STATE ${{AMX_WATCHED-unset}}; cat; }} >> '{}'\"\n",
        said.display()
    ));
    said
}

/// An agent started the way a person starts one, playing `scenario`.
///
/// `new` rather than a pane of the harness's own: the hook that runs the
/// command reads the person's config file, and it is `new` that hands the pane
/// the home that file is kept under.
fn start(amx: &Harness, id: &str, scenario: &str) {
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// One more hook, delivered the way the vendor delivers one: the payload on
/// stdin, and the environment saying whose it is.
fn deliver(amx: &Harness, id: &str, payload: &str) {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = amx
        .amx_command(&["_hook"])
        .env("AMX_ID", id)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("running amx _hook");
    child
        .stdin
        .take()
        .expect("stdin was asked for")
        .write_all(payload.as_bytes())
        .expect("delivering the payload");
    assert!(
        child.wait().expect("waiting for the hook").success(),
        "a hook always exits 0"
    );
}

/// What the command has written, once there is a run's worth of it: the line
/// about its environment and the event it was handed, a pair per run.
fn wrote(amx: &Harness, said: &Path, runs: usize) -> Vec<(String, Value)> {
    let text = amx.until(&format!("the command to have run {runs} times"), || {
        let text = std::fs::read_to_string(said).ok()?;
        (text.lines().count() >= runs * 2).then_some(text)
    });
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), runs * 2, "two lines a run: {text}");
    lines
        .chunks(2)
        .map(|pair| {
            (
                pair[0].to_string(),
                serde_json::from_str(pair[1]).expect("the event as one JSON line"),
            )
        })
        .collect()
}

#[test]
fn a_stop_on_a_question_runs_the_waiting_command_once() {
    let amx = Harness::new();
    let said = writes_down(&amx, "on_waiting");
    start(&amx, "ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    // The notification repeating a box is the same screen said a second time,
    // to somebody who has already been told.
    deliver(
        &amx,
        "ask-a1b",
        r#"{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash","notification_type":"permission_prompt"}"#,
    );
    // A turn back at work, and then a screen nobody has heard of: a stop of its
    // own, and so a moment of its own.
    deliver(
        &amx,
        "ask-a1b",
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"write the file instead"}"#,
    );
    deliver(
        &amx,
        "ask-a1b",
        r#"{"hook_event_name":"Notification","message":"Claude needs your permission to use Write","notification_type":"permission_prompt"}"#,
    );

    let runs = wrote(&amx, &said, 2);
    assert_eq!(
        runs[0].0, "waiting 0",
        "the phase as a word, and nobody attached to the pane"
    );
    assert_eq!(runs[0].1["kind"], "Notification");
    assert_eq!(
        runs[0].1["payload"]["message"],
        "Claude needs your permission to use Bash"
    );
    assert!(
        runs[0].1["at"].as_u64().is_some_and(|at| at > 0),
        "the line the event log got, whole: {}",
        runs[0].1
    );
    assert_eq!(
        runs[1].1["payload"]["message"], "Claude needs your permission to use Write",
        "the second run is the second stop, not the notice repeating the first"
    );
}

#[test]
fn a_turn_that_ends_runs_the_idle_command_and_the_nudge_behind_it_runs_nothing() {
    let amx = Harness::new();
    let said = writes_down(&amx, "on_idle");
    start(&amx, "fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    // The vendor nudges about an idle session a minute after the turn ended.
    // It is the same turn still over.
    deliver(
        &amx,
        "fix-login-a1b",
        r#"{"hook_event_name":"Notification","message":"Claude is waiting for your input","notification_type":"idle_prompt"}"#,
    );
    // A turn of its own, which ends in a moment of its own.
    deliver(
        &amx,
        "fix-login-a1b",
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"and now the linter"}"#,
    );
    deliver(
        &amx,
        "fix-login-a1b",
        r#"{"hook_event_name":"Stop","last_assistant_message":"the linter is clean"}"#,
    );

    let runs = wrote(&amx, &said, 2);
    assert_eq!(runs[0].0, "idle 0");
    assert_eq!(runs[0].1["kind"], "Stop");
    assert_eq!(
        runs[0].1["payload"]["last_assistant_message"],
        "the tests pass now"
    );
    assert_eq!(
        runs[1].1["payload"]["last_assistant_message"], "the linter is clean",
        "the nudge between the two turns was not a moment of its own"
    );
}

#[test]
fn a_command_that_finishes_runs_the_done_command() {
    let amx = Harness::new();
    let said = writes_down(&amx, "on_done");
    start(&amx, "say-hello-b2c", "finishes");
    assert_eq!(amx.until_state("say-hello-b2c", "done")["exit"], 0);

    // The turn ended before the command did, and nobody wrote a command for
    // that moment: one key, one moment.
    let runs = wrote(&amx, &said, 1);
    assert_eq!(runs[0].0, "done 0");
    assert_eq!(runs[0].1["kind"], "exit");
    assert_eq!(runs[0].1["payload"]["code"], 0);
}

#[test]
fn stopping_an_agent_runs_the_stopped_command() {
    let amx = Harness::new();
    let said = writes_down(&amx, "on_stopped");
    start(&amx, "watch-the-log-c3d", "works-without-end");
    amx.until_state("watch-the-log-c3d", "working");

    let out = amx.amx(&["stop", "watch-the-log-c3d", "--force"]);
    assert!(
        out.status.success(),
        "amx stop: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let runs = wrote(&amx, &said, 1);
    assert_eq!(
        runs[0].0, "stopped unset",
        "stop asks tmux nothing about a pane it is closing"
    );
    assert_eq!(runs[0].1["kind"], "stop");
    assert!(
        !amx.event_kinds("watch-the-log-c3d")
            .iter()
            .any(|kind| kind == "stop"),
        "the line is built for the command; nothing happened the record does not say"
    );
}
