//! `amx sub`: one call that spawns a child and waits for its answer.
//!
//! A subagent is an ordinary amx agent whose record names a parent, so most of
//! what these tests weigh is what `amx new` already does — the parent on the
//! record, the id printed somewhere a caller can read it — and the one thing
//! `new` does not: the answer, waited for and handed back, with the exit code
//! saying how the turn ended.

mod common;

use common::Harness;
use serde_json::Value;
use std::process::Output;
use std::time::{Duration, Instant};

/// `amx new` for the parent whose pane later runs `amx sub`.
fn a_parent(amx: &Harness, mock: &str) -> String {
    let out = amx
        .amx_command(&[
            "new",
            "--no-worktree",
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            mock,
            "the parent",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `amx sub` typed inside `parent`'s pane.
fn a_sub(amx: &Harness, parent: &str, mock: &str, args: &[&str]) -> Output {
    let mut line = vec!["sub", "--agent", mock];
    line.extend_from_slice(args);
    amx.amx_command(&line)
        .env("AMX_ID", parent)
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx sub")
}

/// `amx sub` from a person's own shell, which has no `AMX_ID`.
fn a_sub_from_outside(amx: &Harness, mock: &str, args: &[&str]) -> Output {
    let mut line = vec!["sub", "--agent", mock];
    line.extend_from_slice(args);
    amx.amx_command(&line)
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx sub")
}

/// The id `amx sub` wrote on stderr, which is one line and nothing else.
fn id_on(out: &Output) -> String {
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(said.lines().count(), 1, "one line on stderr: {said:?}");
    said.trim().to_string()
}

#[test]
fn sub_spawns_a_child_and_prints_its_answer_and_its_id() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    let out = a_sub(&amx, &parent, &mock, &["scout the auth middleware"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let answer = String::from_utf8_lossy(&out.stdout);
    assert!(
        answer.contains("the importer is ported"),
        "the child's answer is on stdout: {answer:?}"
    );
    let child = id_on(&out);
    let meta = amx.meta(&child);
    assert_eq!(
        meta["parent"], parent,
        "the record names the pane it ran in"
    );
    assert_eq!(meta["depth"], 1);
}

#[test]
fn sub_json_carries_the_id_and_the_answer_in_one_object() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    let out = a_sub(
        &amx,
        &parent,
        &mock,
        &["--json", "scout the auth middleware"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub --json: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let object: Value = serde_json::from_slice(&out.stdout).expect("one object");
    assert_eq!(object["parent"], parent);
    assert!(
        object["id"].as_str().is_some_and(|id| !id.is_empty()),
        "the id is in the object: {object}"
    );
    assert_eq!(object["phase"], "idle");
    assert!(
        object["evidence"].as_str().is_some(),
        "and what the reading came from: {object}"
    );
    assert!(
        object["answer"]
            .as_str()
            .is_some_and(|answer| answer.contains("the importer is ported")),
        "and the answer: {object}"
    );
}

#[test]
fn sub_bg_returns_as_soon_as_it_has_an_id() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    let started = Instant::now();
    let out = a_sub(&amx, &parent, &mock, &["--bg", "scout the auth middleware"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub --bg: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it did not wait for the turn: {:?}",
        started.elapsed()
    );
    assert!(out.stdout.is_empty(), "and printed no answer");
    assert_eq!(amx.meta(&id_on(&out))["parent"], parent);
}

#[test]
fn sub_no_parent_spawns_a_peer_with_no_family() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    let out = a_sub(&amx, &parent, &mock, &["--no-parent", "--bg", "scout"]);
    assert_eq!(out.status.code(), Some(0));
    let child = id_on(&out);
    assert_eq!(amx.meta(&child)["parent"], Value::Null);
    assert_eq!(amx.meta(&child)["depth"], 0);
}

#[test]
fn sub_from_a_persons_shell_is_an_ordinary_spawn() {
    let amx = Harness::new();
    let mock = amx.mock();
    let out = a_sub_from_outside(&amx, &mock, &["--bg", "scout"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let child = id_on(&out);
    assert_eq!(amx.meta(&child)["parent"], Value::Null);
    assert_eq!(amx.meta(&child)["depth"], 0);
}

#[test]
fn sub_refuses_one_child_over_max_children() {
    let amx = Harness::new();
    amx.config("max_children = 1\n");
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    let first = a_sub(&amx, &parent, &mock, &["--bg", "one"]);
    assert_eq!(first.status.code(), Some(0), "the first child stands");

    let second = a_sub(&amx, &parent, &mock, &["--bg", "two"]);
    assert_eq!(
        second.status.code(),
        Some(2),
        "amx sub: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let said = String::from_utf8_lossy(&second.stderr);
    assert!(said.contains("max_children"), "names the key: {said:?}");
}

#[test]
fn sub_refuses_permission_unless_the_config_allows_it() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    let out = a_sub(
        &amx,
        &parent,
        &mock,
        &["--permission", "plan", "--bg", "scout"],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("subagents_may_escalate"),
        "names the key: {said:?}"
    );
}
