//! Clearing away the agents whose work has landed.
//!
//! `sweep` is the one verb that takes a record, a tree and a branch in a
//! breath, and it decides to on evidence it reads off the disk: what the last
//! look wrote down about the request, and what git says about the branch. So
//! these drive the whole of it — a real repository, real branches, real panes
//! — and check both reasons, the law that stops it, and the question.

mod common;

use common::Harness;
use serde_json::json;
use std::path::Path;
use std::process::{Command, Output};

fn sweep(amx: &Harness, args: &[&str]) -> Output {
    amx.amx(&[&["sweep"], args].concat())
}

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx sweep: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// git as these tests run it: none of the developer's own configuration, and
/// an identity of its own for the merges they make.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "amx tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "amx tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn branches(repo: &Path) -> String {
    git(repo, &["branch", "--list"])
}

/// An agent with a tree of its own in `repo`, played to the end of `scenario`.
fn an_agent(amx: &Harness, id: &str, repo: &Path, scenario: &str) -> String {
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

/// An agent that has ended, the ordinary way: it answered and stopped.
fn an_ended_agent(amx: &Harness, id: &str, repo: &Path) -> String {
    let tree = an_agent(amx, id, repo, "finishes");
    amx.until_state(id, "done");
    tree
}

/// What a look at the forge would have written down beside the record: the
/// request on this agent's branch, and that it went in.
fn a_merged_request(amx: &Harness, id: &str, number: u64) {
    let asked = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_secs();
    std::fs::write(
        amx.agent_dir(id).join("pr.json"),
        json!({
            "asked": asked,
            "branch": format!("amx/{id}"),
            "prs": [{ "number": number, "standing": "merged" }],
        })
        .to_string(),
    )
    .expect("writing pr.json");
}

/// A commit of the agent's own, which is what puts its branch somewhere main
/// is not.
fn work_on_the_branch(tree: &str, name: &str) {
    let tree = Path::new(tree);
    std::fs::write(tree.join(name), "fn login() {}\n").expect("a file to commit");
    git(tree, &["add", name]);
    git(tree, &["commit", "-m", "fix the login bug"]);
}

/// The other way work lands: somebody merged the branch themselves.
fn merged_by_hand(repo: &Path, id: &str) {
    git(
        repo,
        &["merge", "--no-ff", "-m", "merge", &format!("amx/{id}")],
    );
}

#[test]
fn sweep_takes_the_agent_the_forge_finished_with_and_the_one_git_did() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // One whose request went in. Its own commit is not in main, so the only
    // thing saying this work has landed is what the last look wrote down.
    let landed = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&landed, "login.rs");
    a_merged_request(&amx, "fix-login-a1b", 12);

    // And one with no request at all, whose branch somebody merged.
    let merged = an_ended_agent(&amx, "add-search-b2c", &repo);
    work_on_the_branch(&merged, "search.rs");
    merged_by_hand(&repo, "add-search-b2c");

    let out = said(&sweep(&amx, &["--force"]));
    assert!(out.contains("fix-login-a1b  #12 merged"), "{out}");
    assert!(
        out.contains("add-search-b2c  amx/add-search-b2c merged into main"),
        "{out}"
    );

    for (id, tree) in [("fix-login-a1b", &landed), ("add-search-b2c", &merged)] {
        assert!(!Path::new(tree).exists(), "{id}'s tree is gone: {out}");
        assert!(
            !branches(&repo).contains(&format!("amx/{id}")),
            "and its branch: {out}"
        );
        assert!(
            !amx.agent_dir(id).exists(),
            "and the record that named them: {out}"
        );
    }
}

#[test]
fn sweep_keeps_a_tree_that_holds_work_no_commit_has_and_the_record_with_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    // A tree left where `new` cut it holds exactly what main holds, so git
    // reads the branch as merged and the agent is on the list.
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    // And then the thing an agent does first: a file git has never heard of.
    std::fs::write(Path::new(&tree).join("login.rs"), "fn login() {}\n").unwrap();

    let out = said(&sweep(&amx, &["--force"]));
    assert!(
        out.contains(&format!(
            "kept fix-login-a1b: {tree} holds work no commit has"
        )),
        "{out}"
    );
    assert!(Path::new(&tree).exists(), "the tree stands: {out}");
    assert!(
        branches(&repo).contains("amx/fix-login-a1b"),
        "and the branch: {out}"
    );
    assert!(
        amx.agent_dir("fix-login-a1b").exists(),
        "and the record, which is where both of them are named: {out}"
    );
}

#[test]
fn sweep_ends_an_agent_that_is_somehow_still_running_before_it_takes_the_tree() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    // A turn that never ends, so the pane is certainly still there when the
    // sweep arrives. What puts the agent on the list is the record saying the
    // work is over — somebody stopped watching this one a while ago — while
    // the vendor sits in its pane holding the tree open.
    let tree = an_agent(&amx, "watch-log-c3d", &repo, "works-without-end");
    amx.until_state("watch-log-c3d", "working");
    let pane = amx.pane_of("watch-log-c3d");
    amx.set_state("watch-log-c3d", json!({ "state": "done" }));

    let out = said(&sweep(&amx, &["--force"]));
    assert!(!amx.pane_alive(&pane), "the pane goes first: {out}");
    assert!(!Path::new(&tree).exists(), "and then the tree: {out}");
    assert!(
        out.find("watch-log-c3d stopped") < out.find("removed"),
        "and it is said in that order: {out}"
    );
    assert!(
        !amx.agent_dir("watch-log-c3d").exists(),
        "and the record with them: {out}"
    );
}

#[test]
fn sweep_says_what_it_would_take_and_takes_none_of_it_when_the_answer_is_no() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);

    let out = said(&amx.amx_with_input(&["sweep"], "n\n"));
    assert!(
        out.contains("fix-login-a1b  amx/fix-login-a1b merged into main"),
        "the agent and why it is on the list: {out}"
    );
    assert!(out.contains("sweep 1? [y/N]"), "{out}");
    assert!(out.contains("nothing swept"), "{out}");

    assert!(Path::new(&tree).exists(), "the tree is where it was: {out}");
    assert!(
        branches(&repo).contains("amx/fix-login-a1b"),
        "and the branch: {out}"
    );
    assert!(amx.agent_dir("fix-login-a1b").exists(), "and the record");
}

#[test]
fn sweep_with_force_asks_nothing_and_a_no_typed_at_it_changes_nothing() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);

    let out = said(&amx.amx_with_input(&["sweep", "--force"], "n\n"));
    assert!(!out.contains("[y/N]"), "nothing was asked: {out}");
    assert!(!Path::new(&tree).exists(), "{out}");
    assert!(
        !branches(&repo).contains("amx/fix-login-a1b"),
        "and there was nothing for the no to answer: {out}"
    );
    assert!(!amx.agent_dir("fix-login-a1b").exists(), "{out}");
}
