//! `amx diff`, `amx events` and the harness's own socket cleanup.

mod common;

use common::{Harness, git, ls, with_a_worktree};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

/// Start an agent with `amx new` and extra `args`, playing `scenario`.
fn started(amx: &Harness, id: &str, scenario: &str, args: &[&str]) {
    let out = amx
        .amx_command(
            &[
                &["new", "--name", id][..],
                args,
                &["--agent", &amx.mock(), "fix the login bug"],
            ]
            .concat(),
        )
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The event kinds printed for agent `id`, in order, read from the id and
/// kind columns.
fn kinds_of(printed: &str, id: &str) -> Vec<String> {
    printed
        .lines()
        .filter(|line| line.split_whitespace().nth(1) == Some(id))
        .filter_map(|line| line.split_whitespace().nth(2).map(str::to_string))
        .collect()
}

#[test]
fn a_session_in_a_worktree_amx_did_not_cut_is_measured_from_where_it_started() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    started(
        &amx,
        "fix-login-a1b",
        "works-without-end",
        &["--dir", &repo.to_string_lossy(), "--no-worktree"],
    );

    let meta = amx.meta("fix-login-a1b");
    assert!(meta["worktree"].is_null(), "amx cut no tree: {meta}");
    assert_eq!(meta["base"].as_str(), Some(head.as_str()), "{meta}");
}

#[test]
fn a_session_in_a_directory_git_has_never_heard_of_has_no_base() {
    let amx = Harness::new();
    let plain = amx.home().join("plain");
    std::fs::create_dir_all(&plain).expect("a directory to work in");
    started(
        &amx,
        "fix-login-a1b",
        "works-without-end",
        &["--dir", &plain.to_string_lossy(), "--no-worktree"],
    );

    let meta = amx.meta("fix-login-a1b");
    assert!(meta["worktree"].is_null(), "{meta}");
    assert!(meta["base"].is_null(), "nothing to measure from: {meta}");
}

#[test]
fn diff_measures_a_record_with_no_base_from_the_branch_it_is_on() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    std::fs::write(repo.join("README.md"), "after\n").expect("the changed file");

    // No worktree, branch or base, as an adopted agent's record has.
    amx.record("adopted-b2c", "%404");
    amx.set_meta("adopted-b2c", json!({ "dir": repo }));

    let out = amx.amx(&["diff", "adopted-b2c"]);
    assert!(
        out.status.success(),
        "amx diff: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let patch = String::from_utf8_lossy(&out.stdout);
    assert!(
        patch.contains("-before") && patch.contains("+after"),
        "{patch}"
    );
}

#[test]
fn diff_from_names_the_commit_to_measure_from() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    git(&tree, &["commit", "-am", "the work"]);

    // From the commit the tree was cut from, the new commit is the work.
    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("+after"));

    // From HEAD, the work is already committed.
    let out = amx.amx(&["diff", "fix-login-a1b", "--from", "HEAD"]);
    assert!(
        out.status.success(),
        "amx diff --from: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "nothing is measured from HEAD but the tree's own uncommitted work"
    );
}

#[test]
fn diff_shows_the_work_including_a_file_git_has_never_heard_of() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));

    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    std::fs::write(tree.join("login.rs"), "fn login() {}\n").expect("the new file");

    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(
        out.status.success(),
        "amx diff: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let patch = String::from_utf8_lossy(&out.stdout);
    assert!(patch.contains("-before"), "{patch}");
    assert!(patch.contains("+after"), "{patch}");
    assert!(patch.contains("+fn login() {}"), "the new file: {patch}");
}

#[test]
fn diff_has_nothing_to_show_for_an_agent_that_has_changed_nothing() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");

    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "an empty patch is an answer, not a failure"
    );
}

/// A `diff` viewer that prefixes each line, keeps a copy in
/// `$HOME/viewed.patch`, and then holds the terminal like a pager.
const VIEWER: &str =
    "diff = \"sed 's/^/VIEWED /' | tee $HOME/viewed.patch; while :; do sleep 0.05; done\"\n";

#[test]
fn diff_at_a_terminal_goes_through_the_viewer_the_config_names() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    amx.config(VIEWER);

    let pane = amx.in_a_terminal(&[], &["diff", "fix-login-a1b"]);

    amx.until("the viewer to draw the patch", || {
        amx.capture(&pane).contains("VIEWED +after").then_some(())
    });
}

#[test]
fn diff_leaves_the_viewer_out_down_a_pipe_and_under_stat() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    amx.config(VIEWER);
    let copy = amx.home().join("viewed.patch");

    // Down a pipe the caller gets git's own patch.
    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(out.status.success());
    let patch = String::from_utf8_lossy(&out.stdout);
    assert!(
        patch.contains("+after") && !patch.contains("VIEWED"),
        "{patch}"
    );
    assert!(!copy.exists(), "and nothing was run to read it");

    // --stat prints a summary and never runs the viewer.
    let pane = amx.in_a_terminal(&[], &["diff", "fix-login-a1b", "--stat"]);
    amx.until(
        "the summary to be printed and the terminal given back",
        || (!amx.pane_alive(&pane)).then_some(()),
    );
    assert!(!copy.exists(), "{}", copy.display());
}

#[test]
fn clibatch_diff_stat_summarises_the_work_instead_of_printing_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));

    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    std::fs::write(tree.join("login.rs"), "fn login() {}\n").expect("the new file");

    let out = amx.amx(&["diff", "fix-login-a1b", "--stat"]);
    assert!(
        out.status.success(),
        "amx diff --stat: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let summary = String::from_utf8_lossy(&out.stdout);
    assert!(summary.contains("README.md"), "{summary}");
    assert!(
        summary.contains("login.rs"),
        "a file git has never heard of counts too: {summary}"
    );
    assert!(summary.contains("2 files changed"), "{summary}");
    assert!(
        !summary.contains("+fn login() {}"),
        "and it is a summary, not the patch: {summary}"
    );
}

#[test]
fn diff_is_taken_from_the_last_commit_the_base_and_the_tree_share() {
    // The agent rebased onto `release`, which does not contain the commit its
    // tree was cut from. Diffing from that commit would show main's later
    // changes reverted as if the agent had made them.
    let amx = Harness::new();
    let repo = amx.a_repo();
    git(&repo, &["branch", "release"]);
    std::fs::write(repo.join("README.md"), "after\n").expect("a second version");
    std::fs::write(repo.join("shipped.rs"), "fn shipped() {}\n").expect("a shipped file");
    git(&repo, &["add", "shipped.rs"]);
    git(&repo, &["commit", "-am", "second"]);
    let cut_from = git(&repo, &["rev-parse", "HEAD"]);

    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));
    std::fs::write(tree.join("login.rs"), "fn login() {}\n").expect("the agent's file");
    git(&tree, &["add", "login.rs"]);
    git(&tree, &["commit", "-m", "the agent's own commit"]);
    git(&tree, &["rebase", "--onto", "release", "main"]);

    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(
        out.status.success(),
        "amx diff: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let patch = String::from_utf8_lossy(&out.stdout);
    assert!(patch.contains("+fn login() {}"), "{patch}");
    assert!(
        !patch.contains("shipped.rs") && !patch.contains("-after"),
        "and not the base's own work, undone: {patch}"
    );

    let out = amx.amx(&["diff", "fix-login-a1b", "--stat"]);
    let summary = String::from_utf8_lossy(&out.stdout);
    assert!(summary.contains("1 file changed"), "{summary}");

    assert_eq!(
        amx.meta("fix-login-a1b")["base"],
        cut_from,
        "and the record still holds the commit the tree was cut from"
    );
}

#[test]
fn diff_measures_the_agent_that_works_in_the_directory_as_it_is() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    started(
        &amx,
        "no-tree-b2c",
        "works-without-end",
        &["--dir", &repo.to_string_lossy(), "--no-worktree"],
    );

    // Nothing has changed yet: an empty patch, not an error.
    let out = amx.amx(&["diff", "no-tree-b2c"]);
    assert!(
        out.status.success(),
        "amx diff: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "nothing changed yet"
    );

    // Changes in the directory count from the commit HEAD was on at spawn.
    std::fs::write(repo.join("README.md"), "after\n").expect("the changed file");
    let out = amx.amx(&["diff", "no-tree-b2c"]);
    assert!(out.status.success());
    let patch = String::from_utf8_lossy(&out.stdout);
    assert!(
        patch.contains("-before") && patch.contains("+after"),
        "{patch}"
    );
}

#[test]
fn diff_names_the_branch_when_the_tree_is_gone() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = PathBuf::from(with_a_worktree(
        &amx,
        "fix-login-a1b",
        &repo,
        "works-without-end",
    ));
    std::fs::remove_dir_all(&tree).expect("removing the tree");

    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("amx/fix-login-a1b"),
        "the work outlives the tree, on the branch: {said}"
    );
}

#[test]
fn diff_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = amx.amx(&["diff", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}

#[test]
fn clibatch_new_refuses_a_task_with_nothing_in_it() {
    // `amx new "$TASK"` with `TASK` unset must not start an idle agent that
    // holds a pane and a worktree.
    let amx = Harness::new();
    let out = amx.amx(&["new", "", "--agent", &amx.mock()]);

    assert_eq!(out.status.code(), Some(64), "a malformed command line");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("something to do"), "{said}");
    assert!(
        std::fs::read_dir(amx.state_root())
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "and nothing was made: {said}"
    );
}

#[test]
fn clibatch_rename_puts_the_word_where_a_program_reads_it() {
    // The name is in `ls --json` next to the id, so programs can label agents
    // the way the person did.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["rename", "fix-login-a1b", "auth"]);
    assert!(
        out.status.success(),
        "amx rename: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let listed = ls(&amx);
    let agent = listed
        .iter()
        .find(|agent| agent["id"] == "fix-login-a1b")
        .expect("the agent that was renamed");

    assert_eq!(agent["name"], "auth");
    assert_eq!(
        agent["id"], "fix-login-a1b",
        "and the id a caller addresses it by did not move"
    );
}

#[test]
fn events_merges_the_logs_and_keeps_each_agents_own_order() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-importer-c3d", "finishes");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-importer-c3d", "done");

    let out = amx.amx(&["events"]);
    assert!(
        out.status.success(),
        "amx events: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        printed.lines().count(),
        amx.events("fix-login-a1b").len() + amx.events("port-importer-c3d").len(),
        "one line per event, and nothing else: {printed}"
    );
    assert_eq!(
        kinds_of(&printed, "fix-login-a1b"),
        ["SessionStart", "UserPromptSubmit", "PreToolUse", "Stop"],
        "{printed}"
    );
    assert_eq!(
        kinds_of(&printed, "port-importer-c3d"),
        ["SessionStart", "UserPromptSubmit", "Stop", "exit"],
        "{printed}"
    );
    assert!(
        printed.contains("the tests pass now"),
        "and what happened is on the line: {printed}"
    );
}

#[test]
fn events_reads_only_the_agents_it_was_named() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-importer-c3d", "finishes");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-importer-c3d", "done");

    let out = amx.amx(&["events", "fix-login-a1b"]);
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(printed.lines().count(), amx.events("fix-login-a1b").len());
    assert!(!printed.contains("port-importer-c3d"), "{printed}");
}

#[test]
fn events_says_so_when_it_is_named_an_agent_that_does_not_exist() {
    let amx = Harness::new();
    let out = amx.amx(&["events", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}

#[test]
fn events_has_nothing_to_say_about_an_agent_nothing_has_happened_to() {
    let amx = Harness::new();
    amx.record("quiet-a1b", "%1");

    let out = amx.amx(&["events"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

#[test]
fn clibatch_events_json_is_the_same_merge_for_a_program_to_read() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-importer-c3d", "finishes");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-importer-c3d", "done");

    let out = amx.amx(&["events", "--json"]);
    assert!(
        out.status.success(),
        "amx events --json: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);

    let read: Vec<serde_json::Value> = printed
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}")))
        .collect();
    assert_eq!(
        read.len(),
        amx.events("fix-login-a1b").len() + amx.events("port-importer-c3d").len(),
        "one object per event, and nothing else: {printed}"
    );

    // Every line names its agent.
    let whose: Vec<&str> = read
        .iter()
        .filter_map(|event| event["id"].as_str())
        .collect();
    assert_eq!(whose.len(), read.len(), "{printed}");
    assert!(whose.contains(&"fix-login-a1b") && whose.contains(&"port-importer-c3d"));

    // The payload is the vendor's, unchanged.
    let stop = read
        .iter()
        .find(|event| event["id"] == "fix-login-a1b" && event["kind"] == "Stop")
        .unwrap_or_else(|| panic!("the turn's end: {printed}"));
    assert_eq!(
        stop["payload"]["last_assistant_message"], "the tests pass now",
        "{printed}"
    );
    assert!(
        stop["payload"]["session_id"].is_string(),
        "whole: {printed}"
    );
}

#[test]
fn following_prints_events_as_they_arrive() {
    let amx = Harness::new();
    let mut child = amx
        .amx_command(&["events", "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("running amx events --follow");
    let stream = child.stdout.take().expect("stdout was asked for");

    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let reading = {
        let seen = Arc::clone(&seen);
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                seen.lock().expect("the lines read so far").push(line);
            }
        })
    };

    // An agent started after the follow began still shows up.
    amx.play("fix-login-a1b", "happy-turn");
    amx.until("the turn to end on the stream", || {
        let seen = seen.lock().expect("the lines read so far");
        seen.iter()
            .any(|line| line.contains("the tests pass now"))
            .then(|| seen.clone())
    });

    let followed = seen.lock().expect("the lines read so far").join("\n");
    assert_eq!(
        kinds_of(&followed, "fix-login-a1b"),
        ["SessionStart", "UserPromptSubmit", "PreToolUse", "Stop"],
        "the whole turn arrived, in order: {followed}"
    );

    child.kill().expect("ending the stream");
    child.wait().expect("waiting for the stream");
    reading.join().expect("the reader");
}

#[test]
fn a_dropped_harness_takes_its_tmux_socket_with_it() {
    let path = {
        let amx = Harness::new();
        // A session keeps the server, and so its socket, alive.
        amx.tmux(&["new-session", "-d", "-s", "keep", "sleep", "60"]);
        let path = PathBuf::from(amx.tmux(&["display-message", "-p", "#{socket_path}"]));
        assert!(path.exists(), "no socket at {}", path.display());
        path
    };
    // Stale sockets pile up in the socket directory until tmux times out
    // opening it.
    assert!(!path.exists(), "{} outlived its harness", path.display());
}
