//! `amx sub`: spawn a child agent, wait for its turn to end and print its
//! answer, with the exit code saying how the turn ended. The child is an
//! ordinary agent whose record names its parent.

mod common;

use common::Harness;
use serde_json::Value;
use std::process::Output;
use std::time::{Duration, Instant};

/// Spawn a parent agent with `amx new`, and answer with its id.
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

/// Run `amx sub` as from `parent`'s pane, with `AMX_ID` set to `parent`.
fn a_sub(amx: &Harness, parent: &str, mock: &str, args: &[&str]) -> Output {
    let mut line = vec!["sub", "--agent", mock];
    line.extend_from_slice(args);
    amx.amx_command(&line)
        .env("AMX_ID", parent)
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx sub")
}

/// Run `amx sub` with no `AMX_ID`, as from a shell outside any agent.
fn a_sub_from_outside(amx: &Harness, mock: &str, args: &[&str]) -> Output {
    let mut line = vec!["sub", "--agent", mock];
    line.extend_from_slice(args);
    amx.amx_command(&line)
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx sub")
}

/// The child id `amx sub` printed, which must be the only line on stderr.
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

    // The transcript path arrives with the parent's first hook, shortly after
    // `amx new` returns.
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
    // With neither `--agent` nor `--model`, the child runs the parent's
    // command instead of the configured default, so a pi parent's child is
    // pi.
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config(&format!("agent = \"{mock} --not-the-parent\"\n"));
    let parent = a_parent(&amx, &mock);

    // No `--agent`, unlike the other tests here.
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

/// Run `amx stop` on an agent with no worktree, which prompts for nothing.
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
    // An orchestrator outside any pane names the child and its parent. The
    // parent may have ended already, e.g. a worker whose diff a reviewer
    // reads, or a dead worker a fresh one takes over from.
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
    // In a repo, a sub with no parent cuts a worktree by default.
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

    // `--no-worktree` wins over a role that sets `worktree: true`.
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
fn sub_refuses_a_parent_whose_directory_has_gone() {
    // The child inherits a stopped parent's directory, which may have been
    // removed since. The error names the parent the path came from and
    // suggests `--dir`.
    let amx = Harness::new();
    let mock = amx.mock();
    let gone = amx.home().join("a-tree-a-run-removed");
    std::fs::create_dir_all(&gone).unwrap();

    let out = amx
        .amx_command(&[
            "new",
            "--no-worktree",
            "--dir",
            &gone.to_string_lossy(),
            "--agent",
            &mock,
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
    let parent = String::from_utf8_lossy(&out.stdout).trim().to_string();
    stopped(&amx, &parent);
    std::fs::remove_dir_all(&gone).unwrap();

    let out = a_sub_from_outside(&amx, &mock, &["--bg", "--parent", &parent, "scout"]);
    assert_eq!(
        out.status.code(),
        Some(64),
        "usage, before anything is claimed"
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains(&parent), "names the parent: {said}");
    assert!(said.contains("--dir"), "says what to do instead: {said}");

    // With `--dir` the same call runs: the parent's directory is a default.
    let out = a_sub_from_outside(
        &amx,
        &mock,
        &[
            "--bg",
            "--parent",
            &parent,
            "--dir",
            &amx.home().to_string_lossy(),
            "scout",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx sub: {}",
        String::from_utf8_lossy(&out.stderr)
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
