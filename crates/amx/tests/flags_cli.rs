//! The flag forms, driven the way a human drives them: the real binary, a real
//! session, a real pane.
//!
//! `crates/amx-proto/tests/flags.rs` proves the translation — argv in, payload
//! out — against values, which is the right place for it. What it cannot prove
//! is that the payload a flag builds is one the *server* accepts: a field
//! spelled right for serde and wrong for the row would pass there and fail
//! here. Twice in this project a green suite has hidden a non-working feature
//! until a live run caught it (`docs/08-m2-plan.md`, opening), so every verb
//! below goes over the socket and its reply is read.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::process::Stdio;

use amx_server::session::probe::probe;
use serde_json::{Value, json};

mod support;

use support::{Env, wait_until};

/// A session with one workspace, one pane, and that pane labelled `build`.
///
/// The label is set through `--params`, which is deliberate: the escape hatch
/// has to keep working on a row that never grew flags, and `pane.rename` is
/// one of those — it takes the pane UUID a client already holds.
struct Session {
    env: Env,
    server: std::process::Child,
    pane: String,
}

impl Session {
    fn new(tag: &str) -> Self {
        let env = Env::new(tag);
        let server = env
            .command()
            .args(["server", "--session", &env.session])
            // Pinned for the reason every process-level suite here pins it: a
            // pane on the developer's own login shell is slow to start, and an
            // interrupted zsh leaves a lock on their real history file.
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn amx server");
        wait_until("the server binds", || {
            probe(&env.socket()).expect("probe").is_running()
        });

        env.run(&["workspace", "create"]).ok();
        let pane = state(&env)["panes"][0]["pane"]
            .as_str()
            .expect("the session has a pane")
            .to_owned();
        env.run(&[
            "pane",
            "rename",
            "--params",
            &json!({"pane": pane, "label": "build"}).to_string(),
        ])
        .ok();
        Self { env, server, pane }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.env.stop();
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

#[test]
fn the_pane_driving_verbs_take_a_target_and_flags() {
    let session = Session::new("flags-pane");
    let env = &session.env;

    // The pane is addressed by the name a human gave it, not by its UUID —
    // which is the whole argument for a positional target.
    let run = reply(&env.run(&["pane", "run", "build", "--text", "echo flagged-hello"]));
    assert_eq!(run["pane"].as_str(), Some(session.pane.as_str()));

    let matched = reply(&env.run(&[
        "pane",
        "wait-output",
        "build",
        "--match",
        "flagged-hello",
        "--timeout",
        "10s",
    ]));
    assert_eq!(matched["matched"], json!(true), "{matched}");
    assert!(
        matched["line"]
            .as_str()
            .is_some_and(|line| line.contains("flagged-hello")),
        "the reply carries the line that matched: {matched}"
    );

    // `--regex` is the other half of the same row, and takes the pattern
    // through without the shell or clap eating it.
    let by_regex = reply(&env.run(&[
        "pane",
        "wait-output",
        "build",
        "--regex",
        "flagged-[a-z]+",
        "--timeout",
        "10s",
    ]));
    assert_eq!(by_regex["matched"], json!(true), "{by_regex}");

    let read = reply(&env.run(&["pane", "read", "build", "--lines", "40"]));
    let screen = read["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        screen.contains("flagged-hello"),
        "the screen holds what was run: {screen:?}"
    );

    let keys = reply(&env.run(&["pane", "send-keys", "build", "ctrl+l", "enter"]));
    assert_eq!(keys["keys"], json!(2), "both combos were encoded: {keys}");

    let sent = reply(&env.run(&["pane", "send-text", "build", "--text", "still here"]));
    assert_eq!(sent["pane"].as_str(), Some(session.pane.as_str()));
}

#[test]
fn wait_takes_until_and_target_and_reports_its_timeout_honestly() {
    let session = Session::new("flags-wait");

    // A shell that is not going anywhere: the wait has to end by its deadline
    // and say it was not satisfied, rather than hang or claim success.
    let waited = reply(&session.env.run(&[
        "wait",
        "--until",
        "exited",
        "--target",
        "build",
        "--timeout",
        "300ms",
    ]));
    assert_eq!(waited["satisfied"], json!(false), "{waited}");
    assert_eq!(waited["pane"].as_str(), Some(session.pane.as_str()));

    // And the agent verbs reach the server with a payload it accepts: this
    // pane runs a shell, so `explain` answers about a pane with no manifest —
    // an answer, not an error.
    let explained = reply(&session.env.run(&["agent", "explain", "build"]));
    assert_eq!(explained["pane"].as_str(), Some(session.pane.as_str()));

    let next = reply(&session.env.run(&["agent", "next"]));
    assert_eq!(next["waiting"], json!(0), "an empty queue is not an error");
}

#[test]
fn a_flagged_verb_refuses_params_and_flags_together() {
    let session = Session::new("flags-exclusive");
    let refused = session
        .env
        .run(&["pane", "read", "build", "--params", "{}"]);

    assert_eq!(refused.code, Some(2), "clap refuses it: {refused:?}");
    assert!(
        refused.stderr.contains("--params") && refused.stderr.contains("cannot be used with"),
        "the error says which two: {refused:?}"
    );
}

#[test]
fn a_flagged_verb_with_nothing_says_what_it_wants_and_names_the_escape_hatch() {
    let session = Session::new("flags-missing");
    let refused = session.env.run(&["pane", "read"]);

    assert_eq!(refused.code, Some(1), "{refused:?}");
    assert!(
        refused.stderr.contains("<TARGET> is required") && refused.stderr.contains("--params"),
        "a user who does not know the flags is told both ways in: {refused:?}"
    );
}

#[test]
fn the_escape_hatch_still_drives_a_flagged_row_end_to_end() {
    let session = Session::new("flags-escape");
    let read = reply(&session.env.run(&[
        "pane",
        "read",
        "--params",
        &json!({"target": "build", "lines": 3}).to_string(),
    ]));
    assert_eq!(read["pane"].as_str(), Some(session.pane.as_str()));
    assert_eq!(read["rows"].as_array().map(Vec::len), Some(3), "{read}");
}

/// The JSON a successful call printed.
fn reply(out: &support::Output) -> Value {
    serde_json::from_str(out.ok()).expect("a reply is JSON")
}

/// The session's state.
fn state(env: &Env) -> Value {
    serde_json::from_str(env.run(&["session", "state"]).ok()).expect("session.state replies")
}
