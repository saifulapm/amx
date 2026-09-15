//! Forgetting the rows that are over, whether or not the work landed.
//!
//! `sweep` takes the agents a forge or git says are done with; `clear` takes
//! the rest of what a wall fills up with — a stopped row, a command that
//! failed, an agent whose branch nobody ever cut. It is the ctrl+x on the wall
//! said once for the whole list, so these drive the same things that key is
//! held to: the list and its reasons, the one question, and the tree holding
//! work no commit has that keeps its record.

mod common;

use common::Harness;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx clear: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

/// git as these tests run it: none of the developer's own configuration.
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

/// A row whose agent is not there any more: no pane, and the record is the
/// whole story.
fn a_row(amx: &Harness, id: &str, state: &str) {
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": state,
            "exit": 0,
            "since": now(),
            "last_event": now(),
            "result": "did what it was asked",
        }),
    );
}

/// An agent with a tree of its own in `repo`, played to the end of its turn.
fn an_ended_agent(amx: &Harness, id: &str, repo: &Path) -> String {
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
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("finishes"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_state(id, "done");
    amx.meta(id)["worktree"]
        .as_str()
        .expect("a worktree")
        .to_string()
}

/// What the last look at the forge wrote down beside the record: the request
/// on this agent's branch, and that it went in.
fn a_merged_request(amx: &Harness, id: &str, number: u64) {
    std::fs::write(
        amx.agent_dir(id).join("pr.json"),
        json!({
            "asked": now(),
            "branch": format!("amx/{id}"),
            "prs": [{ "number": number, "standing": "merged" }],
        })
        .to_string(),
    )
    .expect("writing pr.json");
}

/// A commit of the agent's own, which is what puts its branch somewhere main
/// is not — so nothing about this row has landed.
fn work_on_the_branch(tree: &str, name: &str) {
    let tree = Path::new(tree);
    std::fs::write(tree.join(name), "fn login() {}\n").expect("a file to commit");
    git(tree, &["add", name]);
    git(tree, &["commit", "-m", "fix the login bug"]);
}

#[test]
fn clear_lists_the_finished_rows_and_forgets_the_ones_whose_trees_hold_nothing() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // Two rows somebody stopped. Nothing landed and nothing ever will: there
    // is no branch on either of them for a forge to have an opinion about.
    a_row(&amx, "fix-login-a1b", "stopped");
    a_row(&amx, "add-search-b2c", "stopped");

    // A row whose turn ended, holding a tree with work no commit has.
    let tree = an_ended_agent(&amx, "port-import-c3d", &repo);
    work_on_the_branch(&tree, "import.rs");
    std::fs::write(Path::new(&tree).join("notes.md"), "not committed\n").expect("a loose file");

    // And one sitting at its prompt, in a pane somebody can still type into,
    // which is not finished.
    amx.play("watch-log-d4e", "happy-turn");
    amx.until_state("watch-log-d4e", "idle");

    let out = said(&amx.amx_with_input(&["clear"], "y\n"));

    assert!(out.contains("fix-login-a1b  stopped"), "{out}");
    assert!(out.contains("add-search-b2c  stopped"), "{out}");
    assert!(out.contains("port-import-c3d  done"), "{out}");
    assert!(
        !out.contains("watch-log-d4e"),
        "an agent at its prompt is not finished: {out}"
    );
    assert!(out.contains("clear 3? [y/N]"), "one question: {out}");

    assert!(
        !amx.agent_dir("fix-login-a1b").exists(),
        "the stopped records are gone: {out}"
    );
    assert!(!amx.agent_dir("add-search-b2c").exists(), "{out}");

    assert!(
        out.contains(&format!(
            "kept port-import-c3d: {tree} holds work no commit has"
        )),
        "{out}"
    );
    assert!(Path::new(&tree).exists(), "the tree stands: {out}");
    assert!(
        branches(&repo).contains("amx/port-import-c3d"),
        "and the branch: {out}"
    );
    assert!(
        amx.agent_dir("port-import-c3d").exists(),
        "and the record, which is where both of them are named: {out}"
    );
    assert!(
        amx.agent_dir("watch-log-d4e").exists(),
        "and the row that had not finished: {out}"
    );
}

#[test]
fn clear_takes_a_landed_row_the_way_the_sweep_takes_it_branch_and_all() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // Its own commit is not in main, so the only thing saying this work has
    // landed is what the last look at the forge wrote down.
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&tree, "login.rs");
    a_merged_request(&amx, "fix-login-a1b", 12);

    let out = said(&amx.amx_with_input(&["clear", "--force"], "n\n"));
    assert!(!out.contains("[y/N]"), "nothing was asked: {out}");
    assert!(
        out.contains("fix-login-a1b  #12 merged"),
        "the sweep's reason, not the phase: {out}"
    );
    assert!(!Path::new(&tree).exists(), "the tree goes: {out}");
    assert!(
        !branches(&repo).contains("amx/fix-login-a1b"),
        "and the branch with it, since the repository holds every commit that \
         was on it: {out}"
    );
    assert!(!amx.agent_dir("fix-login-a1b").exists(), "{out}");
}

#[test]
fn clear_keeps_the_branch_of_a_row_whose_work_went_nowhere() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&tree, "login.rs");

    let out = said(&amx.amx(&["clear", "--force"]));
    assert!(out.contains("fix-login-a1b  done"), "{out}");
    assert!(!Path::new(&tree).exists(), "the tree goes: {out}");
    assert!(
        branches(&repo).contains("amx/fix-login-a1b"),
        "and the branch stands, because nothing here says the commits on it \
         are anywhere else: {out}"
    );
    assert!(!amx.agent_dir("fix-login-a1b").exists(), "{out}");
}

#[test]
fn clear_says_nothing_to_clear_where_no_row_has_finished() {
    let amx = Harness::new();
    amx.play("watch-log-d4e", "works-without-end");
    amx.until_state("watch-log-d4e", "working");

    let out = said(&amx.amx(&["clear"]));
    assert_eq!(out, "nothing to clear\n");
    assert!(amx.agent_dir("watch-log-d4e").exists());
}

#[test]
fn clear_says_what_it_would_take_and_takes_none_of_it_when_the_answer_is_no() {
    let amx = Harness::new();
    a_row(&amx, "fix-login-a1b", "stopped");

    let out = said(&amx.amx_with_input(&["clear"], "n\n"));
    assert!(out.contains("fix-login-a1b  stopped"), "{out}");
    assert!(out.contains("clear 1? [y/N]"), "{out}");
    assert!(out.contains("nothing cleared"), "{out}");
    assert!(amx.agent_dir("fix-login-a1b").exists(), "{out}");
}
