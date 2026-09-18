//! Ending an agent, and what it leaves behind.

mod common;

use common::Harness;
use serde_json::Value;
use std::path::{Path, PathBuf};
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

/// The same, with the vendor's stand-in installed under the name the trust
/// table knows.
///
/// That table is keyed by the program an agent command runs, and claude is the
/// one vendor whose store amx writes. A stop that is to prune that store has to
/// have started something by that name, so the stand-in is copied under it and
/// put in front of PATH, the way `new_as_claude` does in tests/e2e_spawn.rs.
fn as_claude(amx: &Harness, id: &str, repo: &Path, scenario: &str) -> String {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the stand-in");
    std::fs::copy(amx.mock(), bin.join("claude")).expect("the stand-in under claude's name");

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            "claude",
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("PATH", path)
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

/// The key the vendor files a directory under: the path with every symlink
/// resolved, which is what it asks the operating system for.
fn key_for(dir: &str) -> String {
    std::fs::canonicalize(dir)
        .expect("a directory that is there")
        .to_string_lossy()
        .into_owned()
}

/// claude's own config file, as the vendor leaves it: an entry per directory it
/// has ever been started in, and the person's own keys around them.
fn a_store(amx: &Harness, tree: &str, repo: &Path) -> PathBuf {
    let mut before = serde_json::json!({
        "numStartups": 412,
        "projects": { "/src/other": { "hasTrustDialogAccepted": true } }
    });
    before["projects"][key_for(tree)] = serde_json::json!({
        "hasTrustDialogAccepted": true,
        "lastCost": 0.2
    });
    before["projects"][key_for(&repo.to_string_lossy())] =
        serde_json::json!({ "hasTrustDialogAccepted": true });

    let store = amx.home().join(".claude.json");
    std::fs::write(&store, serde_json::to_string_pretty(&before).unwrap()).expect("the store");
    store
}

fn read_store(store: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(store).expect("the store")).expect("json")
}

/// The copies amx left beside the store.
fn copies_beside(store: &Path) -> Vec<String> {
    std::fs::read_dir(store.parent().unwrap())
        .expect("the home")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".claude.json.amx-backup-"))
        .collect()
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

/// A record that names `parent` at `depth`, over the pane `amx.play` made.
fn a_child(amx: &Harness, id: &str, parent: &str, depth: u64) -> String {
    let pane = amx.play(id, "happy-turn");
    amx.set_meta(id, serde_json::json!({ "parent": parent, "depth": depth }));
    pane
}

#[test]
fn stopping_a_parent_leaves_its_children_running() {
    let amx = Harness::new();
    let parent = amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    let child = a_child(&amx, "child-b2c", "parent-a1b", 1);
    let grandchild = a_child(&amx, "grand-c3d", "child-b2c", 2);

    let out = said(&stop(&amx, &["parent-a1b", "--force"]));

    assert!(!amx.pane_alive(&parent), "the parent goes");
    for pane in [&child, &grandchild] {
        assert!(amx.pane_alive(pane), "a child is never its parent's to end");
    }
    for id in ["child-b2c", "grand-c3d"] {
        assert_ne!(amx.state(id)["state"], "stopped", "{id}");
    }
    let lines: Vec<&str> = out
        .lines()
        .filter(|line| line.ends_with(" stopped"))
        .collect();
    assert_eq!(lines, ["parent-a1b stopped"], "the one it was asked for");
}

#[test]
fn stopping_a_child_never_touches_its_parent() {
    let amx = Harness::new();
    let parent = amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    let child = a_child(&amx, "child-b2c", "parent-a1b", 1);

    said(&stop(&amx, &["child-b2c", "--force"]));

    assert!(!amx.pane_alive(&child));
    assert!(
        amx.pane_alive(&parent),
        "the parent is not the child's to end"
    );
    assert_ne!(amx.state("parent-a1b")["state"], "stopped");
}

#[test]
fn a_child_of_a_removed_parent_is_still_an_agent() {
    // `stop --delete` takes the parent's record away. The child's record
    // still names it, and what is left is an ordinary row rather than a
    // record the reader drops.
    let amx = Harness::new();
    amx.play("parent-a1b", "happy-turn");
    amx.until_state("parent-a1b", "idle");
    let child = a_child(&amx, "child-b2c", "parent-a1b", 1);

    said(&stop(&amx, &["parent-a1b", "--force", "--delete"]));
    assert!(!amx.agent_dir("parent-a1b").exists());

    let status = amx.amx(&["status", "child-b2c", "--json"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let row: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(row["id"], "child-b2c");
    assert_eq!(
        row["parent"], "parent-a1b",
        "the record names the gone parent"
    );
    assert!(amx.pane_alive(&child), "and the child is left running");
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
fn a_pane_that_answers_for_another_agent_is_left_standing() {
    // tmux hands pane numbers out again after a server restart, so a record
    // that outlived its server names whichever pane took its number. Recording
    // a second agent on the pane is that, without the reboot: the pane answers
    // for the agent recorded last, and the first record has lost it.
    let amx = Harness::new();
    let pane = amx.play("yesterday-a1b", "happy-turn");
    amx.until_state("yesterday-a1b", "idle");
    amx.record("today-b2c", &pane);

    let out = said(&stop(&amx, &["yesterday-a1b", "--force"]));
    assert!(out.contains("stopped"), "{out}");
    assert!(
        amx.pane_alive(&pane),
        "the pane is the other agent's, and stopping this one does not reach it"
    );
    assert_eq!(
        amx.state("yesterday-a1b")["state"],
        "stopped",
        "the agent it was aimed at is ended all the same"
    );
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
fn stopping_an_agent_takes_its_tree_back_out_of_claudes_store() {
    // claude writes a project entry for every directory it is started in, and
    // amx cuts a directory per agent: a store nobody prunes grows a key for
    // each of them and keeps it long after the tree it names has gone.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = as_claude(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    let store = a_store(&amx, &worktree, &repo);
    let tree_key = key_for(&worktree);

    let out = said(&stop(&amx, &["fix-login-a1b", "--force"]));
    assert!(!Path::new(&worktree).exists(), "the tree is gone: {out}");
    assert!(out.contains("forgot"), "and it says so: {out}");

    let after = read_store(&store);
    assert_eq!(after["projects"].get(&tree_key), None, "{after}");
    assert_eq!(
        after["projects"][key_for(&repo.to_string_lossy())]["hasTrustDialogAccepted"],
        serde_json::json!(true),
        "the repository's entry is the person's consent: {after}"
    );
    assert_eq!(
        after["projects"]["/src/other"]["hasTrustDialogAccepted"],
        serde_json::json!(true)
    );
    assert_eq!(after["numStartups"], 412, "{after}");

    let copies = copies_beside(&store);
    assert_eq!(copies.len(), 1, "the file as it was, once: {copies:?}");
}

#[test]
fn a_worktree_that_is_kept_keeps_its_key_in_claudes_store() {
    // The key says the vendor may work in that directory without asking. A
    // directory that is still there is one somebody may still work in.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = as_claude(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    let store = a_store(&amx, &worktree, &repo);
    let tree_key = key_for(&worktree);

    std::fs::write(Path::new(&worktree).join("login.rs"), "fn login() {}\n").unwrap();
    let out = said(&stop(
        &amx,
        &["fix-login-a1b", "--force", "--worktree", "delete"],
    ));

    assert!(Path::new(&worktree).exists(), "{out}");
    assert!(!out.contains("forgot"), "{out}");
    let after = read_store(&store);
    assert_eq!(
        after["projects"][&tree_key]["hasTrustDialogAccepted"],
        serde_json::json!(true),
        "{after}"
    );
    assert!(
        copies_beside(&store).is_empty(),
        "and the file was never opened for writing"
    );
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
fn clibatch_delete_takes_the_record_off_when_the_worktree_is_gone() {
    // Somebody has already deleted the directory. Everything that names the
    // repository is asked from inside that directory, so the whole of `stop`
    // used to abort on it and the record had to come off by hand.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let worktree = with_a_worktree(&amx, "fix-login-a1b", &repo, "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    std::fs::remove_dir_all(&worktree).unwrap();

    let out = said(&stop(
        &amx,
        &["fix-login-a1b", "--force", "--branch", "delete", "--delete"],
    ));
    assert!(
        !amx.agent_dir("fix-login-a1b").exists(),
        "the row goes, whatever became of the tree: {out}"
    );
    assert!(
        !branches(&repo).contains("amx/fix-login-a1b"),
        "and the repository it was cut under was found without it: {out}"
    );
}

#[test]
fn stop_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = amx.amx(&["stop", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}
