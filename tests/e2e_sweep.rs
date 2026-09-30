//! `amx sweep`: removing the record, worktree and branch of agents whose
//! work has landed.
//!
//! The evidence is the recorded forge lookup (`pr.json`) and what git says
//! about the branch, so these tests use real repositories, branches and panes.

mod common;

use common::{
    Harness, a_merged_request, an_ended_agent, branches, git, merged_by_hand, with_a_worktree,
    work_on_the_branch,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

fn sweep(amx: &Harness, args: &[&str]) -> Output {
    amx.amx(&[&["sweep"], args].concat())
}

/// Run `amx sweep` with `bin` first on PATH.
fn sweep_with(amx: &Harness, bin: &Path, args: &[&str]) -> Output {
    let path = match std::env::var("PATH") {
        Ok(rest) => format!("{}:{rest}", bin.display()),
        Err(_) => bin.display().to_string(),
    };
    amx.amx_command(&[&["sweep"], args].concat())
        .env("PATH", path)
        .output()
        .expect("running amx sweep")
}

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx sweep: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A fake `gh` that prints `said` for every call, and answer with its
/// directory.
///
/// The machine's real gh must never be asked about a temporary repository.
fn a_forge_saying(amx: &Harness, said: &str) -> PathBuf {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the forge");
    let gh = bin.join("gh");
    std::fs::write(&gh, format!("#!/bin/sh\ncat <<'SAID'\n{said}\nSAID\n")).expect("writing gh");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("a runnable gh");
    bin
}

/// A bare origin next to `repo`, with `main` pushed to it.
fn an_origin(repo: &Path) -> PathBuf {
    let bare = repo.with_file_name("origin.git");
    git(
        repo.parent().expect("somewhere to put it"),
        &["init", "--bare", "-b", "main", &bare.to_string_lossy()],
    );
    git(repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(repo, &["push", "-q", "origin", "main"]);
    bare
}

#[test]
fn sweep_takes_the_agent_the_forge_finished_with_and_the_one_git_did() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // Merged by request: the commit is not in main, so only the recorded
    // forge lookup says it landed.
    let landed = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&landed, "login.rs");
    a_merged_request(&amx, "fix-login-a1b", 12, &landed);

    // Merged by hand, with no request.
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
fn sweep_asks_the_forge_itself_where_no_look_has_written_a_request_down() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    // The commit is not in main, so only the forge knows the branch was
    // merged at its current head.
    work_on_the_branch(&tree, "login.rs");
    let head = git(Path::new(&tree), &["rev-parse", "HEAD"]);
    let bin = a_forge_saying(
        &amx,
        &format!(
            r#"[{{"number":12,"state":"MERGED","isDraft":false,
             "reviewDecision":"","statusCheckRollup":[],"headRefOid":"{}"}}]"#,
            head.trim()
        ),
    );
    assert!(
        !amx.agent_dir("fix-login-a1b").join("pr.json").exists(),
        "nobody has opened the view, so nothing is written down beside the record"
    );

    let out = said(&sweep_with(&amx, &bin, &["--force"]));
    assert!(out.contains("fix-login-a1b  #12 merged"), "{out}");
    assert!(!Path::new(&tree).exists(), "the tree is gone: {out}");
    assert!(
        !branches(&repo).contains("amx/fix-login-a1b"),
        "and its branch: {out}"
    );
    assert!(
        !amx.agent_dir("fix-login-a1b").exists(),
        "and the record that named them: {out}"
    );
}

#[test]
fn sweep_keeps_a_merged_branch_the_agent_went_on_committing_on() {
    // The request merged, then two more unpushed commits landed on the same
    // branch. They are on no other branch.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&tree, "login.rs");
    a_merged_request(&amx, "fix-login-a1b", 12, &tree);
    work_on_the_branch(&tree, "after.rs");
    work_on_the_branch(&tree, "later.rs");

    let out = said(&sweep(&amx, &["--force"]));
    assert!(out.contains("fix-login-a1b  #12 merged"), "{out}");
    assert!(
        out.contains("kept amx/fix-login-a1b: 3 commits are not on any other branch"),
        "{out}"
    );
    assert!(branches(&repo).contains("amx/fix-login-a1b"), "{out}");
    let kept = git(&repo, &["log", "--format=%s", "amx/fix-login-a1b", "-3"]);
    assert_eq!(
        kept.lines().count(),
        3,
        "every commit is still there: {kept}"
    );
}

#[test]
fn sweep_never_deletes_a_branch_a_person_named() {
    // An agent started with `--branch develop` reads as landed once main has
    // caught up, but develop belongs to the person.
    let amx = Harness::new();
    let repo = amx.a_repo();
    git(&repo, &["branch", "develop"]);
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            "fix-login-a1b",
            "--dir",
            &repo.to_string_lossy(),
            "--branch",
            "develop",
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("finishes"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_state("fix-login-a1b", "done");

    let out = said(&sweep(&amx, &["--force"]));
    assert!(
        out.contains("fix-login-a1b  develop merged into main"),
        "{out}"
    );
    assert!(out.contains("kept develop: amx did not create it"), "{out}");
    assert!(branches(&repo).contains("develop"), "{out}");
}

#[test]
fn sweep_takes_the_agent_whose_branch_the_origin_no_longer_has() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let origin = an_origin(&repo);
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&tree, "login.rs");
    git(
        Path::new(&tree),
        &["push", "-q", "-u", "origin", "amx/fix-login-a1b"],
    );
    // A squash merge: the work landed under a commit this branch does not
    // hold, and the forge deleted the branch. Only the sweep's own fetch
    // learns that.
    git(&origin, &["branch", "-D", "amx/fix-login-a1b"]);

    let out = said(&sweep(&amx, &["--force"]));
    assert!(
        out.contains("fix-login-a1b  amx/fix-login-a1b gone from origin"),
        "{out}"
    );
    assert!(!Path::new(&tree).exists(), "the tree is gone: {out}");
    // Nothing says this head merged, and its commit is on no other branch, so
    // the branch is kept.
    assert!(
        out.contains("kept amx/fix-login-a1b: 1 commit is not on any other branch"),
        "{out}"
    );
    assert!(branches(&repo).contains("amx/fix-login-a1b"), "{out}");
    assert!(
        !amx.agent_dir("fix-login-a1b").exists(),
        "and the record that named them: {out}"
    );
}

#[test]
fn sweep_keeps_a_tree_that_holds_work_no_commit_has_and_the_record_with_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    // An untouched tree matches main, so git reads the branch as merged.
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    // Then an untracked file.
    std::fs::write(Path::new(&tree).join("login.rs"), "fn login() {}\n").unwrap();

    let out = said(&sweep(&amx, &["--force"]));
    assert!(
        out.contains(&format!(
            "kept fix-login-a1b: {tree} has uncommitted changes"
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
    // A turn that never ends keeps the pane alive, while the record says the
    // agent is done, which puts it on the sweep's list.
    let tree = with_a_worktree(&amx, "watch-log-c3d", &repo, "works-without-end");
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
