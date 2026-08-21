//! Ending an agent, and what it leaves behind.

mod common;

use common::Harness;
use std::path::Path;
use std::process::Output;

fn stop(amx: &Harness, args: &[&str]) -> Output {
    amx.amx(&[&["stop"], args].concat())
}

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx stop: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// An agent playing a scenario, with a worktree of its own in `repo`.
fn with_a_worktree(amx: &Harness, id: &str, repo: &Path, scenario: &str) -> String {
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &repo.to_string_lossy(),
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
    amx.meta(id)["worktree"]
        .as_str()
        .expect("a worktree")
        .to_string()
}

fn branches(repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["branch", "--list"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("running git");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn stop_ends_the_agent_and_records_that_it_was_stopped() {
    let amx = Harness::new();
    let pane = amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = stop(&amx, &["fix-login-a1b", "--force"]);
    assert!(said(&out).contains("stopped"));

    assert!(!amx.pane_alive(&pane), "the pane goes with the agent");
    let state = amx.state("fix-login-a1b");
    assert_eq!(
        state["state"], "stopped",
        "stopped, not failed: the signal is why it exited"
    );
    assert_eq!(
        state["result"], "the tests pass now",
        "and what it answered outlives it"
    );
}

#[test]
fn an_agent_that_will_not_stop_when_asked_is_stopped_anyway() {
    let amx = Harness::new();
    let pane = amx.play("stubborn-c3d", "will-not-stop");
    amx.until_state("stubborn-c3d", "working");

    said(&stop(&amx, &["stubborn-c3d", "--force"]));
    assert!(
        !amx.pane_alive(&pane),
        "being asked nicely is the first rung, not the whole ladder"
    );
    assert_eq!(amx.state("stubborn-c3d")["state"], "stopped");
}

#[test]
fn stopping_an_agent_deletes_its_worktree_and_keeps_its_branch() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = said(&stop(&amx, &["fix-login-a1b", "--force"]));
    assert!(out.contains("removed"), "{out}");
    assert!(!Path::new(&worktree).exists(), "the tree is gone");
    assert!(
        branches(&repo).contains("amx/fix-login-a1b"),
        "and the work on it is not"
    );
    assert!(
        amx.agent_dir("fix-login-a1b").exists(),
        "the record stays: it is where the branch is named"
    );
}

#[test]
fn a_worktree_with_work_in_it_is_always_kept_and_always_said() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    // What an agent's first act usually is: a file git has never heard of.
    std::fs::write(Path::new(&worktree).join("login.rs"), "fn login() {}\n").unwrap();

    let out = said(&stop(
        &amx,
        &["fix-login-a1b", "--force", "--worktree", "delete"],
    ));
    assert!(
        Path::new(&worktree).exists(),
        "work no commit has is not amx's to delete: {out}"
    );
    assert!(out.contains("no commit has"), "and it says so: {out}");
}

#[test]
fn the_dispositions_can_be_answered_on_the_command_line() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = said(&stop(
        &amx,
        &[
            "fix-login-a1b",
            "--worktree",
            "delete",
            "--branch",
            "delete",
        ],
    ));
    assert!(!Path::new(&worktree).exists(), "{out}");
    assert!(
        !branches(&repo).contains("amx/fix-login-a1b"),
        "asked for, and done: {out}"
    );
}

#[test]
fn a_branch_a_kept_worktree_has_checked_out_stays_and_says_why() {
    // git will not delete a branch a worktree holds, and the agent is stopped
    // by the time anybody finds out: saying so beats failing the command.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = said(&stop(
        &amx,
        &["fix-login-a1b", "--worktree", "keep", "--branch", "delete"],
    ));
    assert!(Path::new(&worktree).exists(), "{out}");
    assert!(
        branches(&repo).contains("amx/fix-login-a1b"),
        "the branch is still there: {out}"
    );
    assert!(out.contains("still has it checked out"), "{out}");
}

#[test]
fn stopping_asks_when_nobody_has_answered() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    // No to the worktree, and nothing to the branch, which keeps it.
    let out = amx.amx_with_input(&["stop", "fix-login-a1b"], "n\n\n");
    let printed = said(&out);

    assert!(printed.contains("delete the worktree"), "{printed}");
    assert!(printed.contains("[Y/n]"), "which way enter goes: {printed}");
    assert!(
        Path::new(&worktree).exists(),
        "the answer was no: {printed}"
    );
    assert!(branches(&repo).contains("amx/fix-login-a1b"), "{printed}");
}

#[test]
fn stopping_an_agent_that_has_already_ended_still_tidies_up() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "say-hello-b2c", &repo, "finishes");
    amx.until_state("say-hello-b2c", "done");

    said(&stop(&amx, &["say-hello-b2c", "--force"]));
    assert!(!Path::new(&worktree).exists(), "the tree is cleared away");
    assert_eq!(
        amx.state("say-hello-b2c")["state"],
        "done",
        "and how it ended is not rewritten"
    );
}

#[test]
fn clibatch_delete_takes_the_record_away_with_the_agent() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = said(&stop(&amx, &["fix-login-a1b", "--force", "--delete"]));
    assert!(!Path::new(&worktree).exists(), "{out}");
    assert!(
        !amx.agent_dir("fix-login-a1b").exists(),
        "the row goes, and everything under it: {out}"
    );

    let listed = amx.amx(&["ls"]);
    assert!(
        !String::from_utf8_lossy(&listed.stdout).contains("fix-login-a1b"),
        "and nothing lists it any more"
    );
}

#[test]
fn clibatch_delete_still_asks_before_it_takes_a_worktree() {
    // `--delete` says what becomes of the record. It is not `--force`, and it
    // is not an answer to a question about somebody's work.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let printed = said(&amx.amx_with_input(&["stop", "fix-login-a1b", "--delete"], "n\n"));
    assert!(printed.contains("delete the worktree"), "{printed}");
    assert!(
        Path::new(&worktree).exists(),
        "the answer was no: {printed}"
    );
    assert!(
        printed.contains(&worktree),
        "and the record that named it is going, so the line says where it is: {printed}"
    );
    assert!(!amx.agent_dir("fix-login-a1b").exists(), "{printed}");
}

#[test]
fn stop_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = amx.amx(&["stop", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}
