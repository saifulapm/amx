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
fn sub_context_digest_puts_the_parents_task_and_last_word_in_the_brief() {
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);

    // The parent's last word comes off the transcript its first hook names,
    // which is a moment after `amx new` has returned.
    let transcript = amx.until("the parent to announce a transcript", || {
        amx.meta(&parent)["transcript"].as_str().map(str::to_string)
    });
    std::fs::write(
        &transcript,
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"the importer is ported\"}],\"usage\":{\"input_tokens\":9}}}\n",
    )
    .unwrap();

    let out = a_sub(
        &amx,
        &parent,
        &mock,
        &["--context", "digest", "scout the auth middleware"],
    );
    assert!(
        out.status.success(),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let child = id_on(&out);
    assert_eq!(
        amx.meta(&child)["task"],
        "scout the auth middleware",
        "the record keeps the task alone"
    );

    let shown = amx.until("the child's vendor to say how it was called", || {
        let screen = amx.capture(&amx.pane_of(&child));
        screen.contains("argv:").then_some(screen)
    });
    assert!(
        shown.contains("the parent"),
        "the parent's task is in the brief: {shown}"
    );
    assert!(
        shown.contains("the importer is ported"),
        "and so is its last word: {shown}"
    );
    assert!(
        shown.contains("scout the auth middleware"),
        "and the child's own task comes last: {shown}"
    );
}

#[test]
fn sub_context_digest_outside_a_pane_is_refused() {
    let amx = Harness::new();
    let mock = amx.mock();
    let out = a_sub_from_outside(&amx, &mock, &["--context", "digest", "scout"]);
    assert_eq!(out.status.code(), Some(64), "a usage refusal");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("parent"), "names what is missing: {said:?}");
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

#[test]
fn sub_hands_the_parents_vendor_down_when_no_agent_is_named() {
    // A subagent of a pi agent is pi. The parent's own command is the child's
    // default whenever the caller names neither `--agent` nor `--model`, so a
    // child typed inside a pane does not quietly run whatever the config file
    // says the machine's agent is. Found live on 2026-09-17: a pi parent's
    // child came up claude.
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config(&format!("agent = \"{mock} --not-the-parent\"\n"));
    let parent = a_parent(&amx, &mock);

    // No `--agent`, unlike every other test here.
    let out = amx
        .amx_command(&["sub", "--bg", "scout"])
        .env("AMX_ID", &parent)
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx sub");
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let child = id_on(&out);
    assert_eq!(
        amx.meta(&child)["agent"],
        amx.meta(&parent)["agent"],
        "the child runs what its parent runs"
    );
    assert_ne!(
        amx.meta(&child)["agent"],
        format!("{mock} --not-the-parent"),
        "and not what the config file names"
    );
}

/// `amx stop` on a parent that has no worktree, so it asks nothing.
fn stopped(amx: &Harness, id: &str) {
    let out = amx
        .amx_command(&["stop", id])
        .output()
        .expect("running amx stop");
    assert!(
        out.status.success(),
        "amx stop: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn sub_from_outside_takes_a_name_and_a_parent_that_has_ended() {
    // A program driving amx from no pane -- workflow dispatching a task's
    // reader -- names the child and the agent it belongs to, and the parent
    // may be one that has already ended: the worker whose diff is read, or
    // the dead worker a fresh one takes over from.
    let amx = Harness::new();
    let mock = amx.mock();
    let parent = a_parent(&amx, &mock);
    stopped(&amx, &parent);

    let out = a_sub_from_outside(
        &amx,
        &mock,
        &[
            "--bg",
            "--name",
            "wf-t1-review-a1b2",
            "--parent",
            &parent,
            "scout",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(id_on(&out), "wf-t1-review-a1b2", "the name is the id");
    let meta = amx.meta("wf-t1-review-a1b2");
    assert_eq!(meta["parent"], parent);
    assert_eq!(meta["depth"], 1);
}

#[test]
fn sub_from_outside_with_no_worktree_runs_in_the_directory_as_it_is() {
    // In a checkout, where a parentless sub would otherwise cut a tree.
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();
    let repo_s = repo.to_string_lossy().to_string();
    let cut = a_sub_from_outside(&amx, &mock, &["--bg", "--dir", &repo_s, "scout"]);
    assert_eq!(cut.status.code(), Some(0));
    assert_ne!(
        amx.meta(&id_on(&cut))["worktree"],
        Value::Null,
        "without the flag a tree is cut, which is what the flag is against"
    );

    let out = a_sub_from_outside(
        &amx,
        &mock,
        &["--bg", "--no-worktree", "--dir", &repo_s, "scout"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta = amx.meta(&id_on(&out));
    assert_eq!(meta["worktree"], Value::Null, "no tree of its own");
    assert_eq!(meta["dir"], repo_s);

    // A role with an opinion does not undo a typed flag.
    std::fs::create_dir_all(repo.join(".amx/agents")).unwrap();
    std::fs::write(
        repo.join(".amx/agents/reader.md"),
        "---\nworktree: true\n---\n\nread only\n",
    )
    .unwrap();
    let out = a_sub_from_outside(
        &amx,
        &mock,
        &[
            "--bg",
            "--role",
            "reader",
            "--no-worktree",
            "--dir",
            &repo_s,
            "scout",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        amx.meta(&id_on(&out))["worktree"],
        Value::Null,
        "the flag stands over the role"
    );
}

#[test]
fn sub_refuses_a_parent_amx_has_no_record_of() {
    let amx = Harness::new();
    let mock = amx.mock();
    let out = a_sub_from_outside(&amx, &mock, &["--bg", "--parent", "nope", "scout"]);
    assert_eq!(
        out.status.code(),
        Some(64),
        "usage, before anything is claimed"
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("nope"), "names the id: {said}");
    assert!(
        amx.amx_command(&["ls", "--json"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "[]")
            .unwrap_or(false),
        "nothing was started"
    );
}
