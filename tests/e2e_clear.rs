//! `amx clear`: forgetting finished rows, whether or not their work landed.
//!
//! `sweep` takes only agents whose work a forge or git says has landed;
//! `clear` also takes stopped rows and failed commands. It is the wall's
//! ctrl+x for the whole list: one question, a reason per row, and a tree with
//! uncommitted work keeps its record.

mod common;

use common::{
    Harness, a_merged_request, an_ended_agent, branches, finished, git, work_on_the_branch,
};
use std::path::Path;
use std::process::Output;

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx clear: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn clear_lists_the_finished_rows_and_forgets_the_ones_whose_trees_hold_nothing() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // Two stopped rows with no branch for a forge to know about.
    finished(&amx, "fix-login-a1b", "stopped", 0);
    finished(&amx, "add-search-b2c", "stopped", 0);

    // A finished row whose tree holds uncommitted work.
    let tree = an_ended_agent(&amx, "port-import-c3d", &repo);
    work_on_the_branch(&tree, "import.rs");
    std::fs::write(Path::new(&tree).join("notes.md"), "not committed\n").expect("a loose file");

    // An agent idle at its prompt is not finished.
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
            "kept port-import-c3d: {tree} is still there, and so is its record"
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

    // The commit is not in main, so only the recorded forge lookup says the
    // work landed.
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    work_on_the_branch(&tree, "login.rs");
    a_merged_request(&amx, "fix-login-a1b", 12, &tree);

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
    finished(&amx, "fix-login-a1b", "stopped", 0);

    let out = said(&amx.amx_with_input(&["clear"], "n\n"));
    assert!(out.contains("fix-login-a1b  stopped"), "{out}");
    assert!(out.contains("clear 1? [y/N]"), "{out}");
    assert!(out.contains("nothing cleared"), "{out}");
    assert!(amx.agent_dir("fix-login-a1b").exists(), "{out}");
}

#[test]
fn clear_goes_past_a_row_whose_tree_git_will_not_remove() {
    // git refuses to remove a locked tree. That row keeps its tree and record,
    // and the next row is still cleared.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let locked = an_ended_agent(&amx, "fix-login-a1b", &repo);
    let loose = an_ended_agent(&amx, "tidy-b2c", &repo);
    git(&repo, &["worktree", "lock", &locked]);

    let out = amx.amx_with_input(&["clear", "--force"], "");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("kept fix-login-a1b"), "{said}");
    assert!(Path::new(&locked).exists(), "the locked tree stays: {said}");
    assert!(
        amx.agent_dir("fix-login-a1b").exists(),
        "and its record: {said}"
    );
    assert!(
        !Path::new(&loose).exists(),
        "the next row was cleared: {said}"
    );
    assert!(!amx.agent_dir("tidy-b2c").exists(), "{said}");
}

#[test]
fn a_kept_tree_keeps_its_record_under_stop_delete() {
    // A tree with uncommitted work is kept, so the record naming it is kept
    // too.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = an_ended_agent(&amx, "fix-login-a1b", &repo);
    std::fs::write(Path::new(&tree).join("wip.rs"), "not committed\n").expect("work");

    let out = amx.amx(&["stop", "fix-login-a1b", "--force", "--delete"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(Path::new(&tree).exists(), "{said}");
    assert!(
        amx.agent_dir("fix-login-a1b").exists(),
        "the record stays: {said}"
    );
    assert!(said.contains("kept fix-login-a1b's record"), "{said}");
}
