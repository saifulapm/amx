//! What an agent has changed, and everything that has happened to it.

mod common;

use common::Harness;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

/// An agent with a worktree of its own in `repo`, and the tree it got.
fn with_a_worktree(amx: &Harness, id: &str, repo: &Path, scenario: &str) -> PathBuf {
    started(amx, id, scenario, &["--dir", &repo.to_string_lossy()]);
    amx.meta(id)["worktree"]
        .as_str()
        .expect("a worktree")
        .into()
}

/// An agent playing a scenario, started the way a person starts one.
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

/// The kinds one agent's lines carry, in the order the stream printed them.
///
/// Reading them out of the columns is the point: a merged stream that does not
/// say whose each line is cannot be read at all.
fn kinds_of(printed: &str, id: &str) -> Vec<String> {
    printed
        .lines()
        .filter(|line| line.split_whitespace().nth(1) == Some(id))
        .filter_map(|line| line.split_whitespace().nth(2).map(str::to_string))
        .collect()
}

#[test]
fn diff_shows_the_work_including_a_file_git_has_never_heard_of() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");

    // The agent's own work, while it is still working.
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

#[test]
fn diff_says_so_when_the_agent_works_in_the_directory_as_it_is() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    started(
        &amx,
        "no-tree-b2c",
        "works-without-end",
        &["--dir", &repo.to_string_lossy(), "--no-worktree"],
    );

    let out = amx.amx(&["diff", "no-tree-b2c"]);
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("no worktree"), "{said}");
    assert!(
        said.contains(repo.to_string_lossy().as_ref()),
        "and it names where the agent does work instead: {said}"
    );
}

#[test]
fn diff_names_the_branch_when_the_tree_is_gone() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");
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
    // `amx new "$TASK"` with `TASK` unset is a command line somebody means, so
    // what it must not do is start an agent that sits there with nothing to
    // do, holding a pane and a worktree.
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

    // Every line says whose it is, which is the whole of what a merged stream
    // adds to the logs it merged.
    let whose: Vec<&str> = read
        .iter()
        .filter_map(|event| event["id"].as_str())
        .collect();
    assert_eq!(whose.len(), read.len(), "{printed}");
    assert!(whose.contains(&"fix-login-a1b") && whose.contains(&"port-importer-c3d"));

    // And the payload is the vendor's own, not a phrase amx made of it.
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

    // An agent that starts while the stream is running joins it.
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

    // A follow ends when the person watching it does.
    child.kill().expect("ending the stream");
    child.wait().expect("waiting for the stream");
    reading.join().expect("the reader");
}
