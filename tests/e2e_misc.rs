//! What an agent has changed, and everything that has happened to it.

mod common;

use common::Harness;
use serde_json::json;
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

/// git in a repository the harness made, with none of the developer's own
/// configuration behind it.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
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

    // A record with no worktree, no branch and no base: what an adopted agent
    // carries, and what every record written before amx recorded a base for a
    // tree it did not cut carries.
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
    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    git(&tree, &["commit", "-am", "the work"]);

    // Measured from the commit the tree was cut from, the commit is the work.
    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("+after"));

    // Named instead, from HEAD, there is nothing left to show: the work is
    // already in the commit the base points at.
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

/// A viewer that marks every row it was handed, keeps a copy of them, and then
/// holds the terminal the way a pager does.
const VIEWER: &str =
    "diff = \"sed 's/^/VIEWED /' | tee $HOME/viewed.patch; while :; do sleep 0.05; done\"\n";

#[test]
fn diff_at_a_terminal_goes_through_the_viewer_the_config_names() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    amx.config(VIEWER);

    let pane = amx.in_a_terminal(&[], &["diff", "fix-login-a1b"]);

    // On the terminal amx was asked from, which is what a viewer is for: the
    // patch is drawn where a person is looking rather than piped anywhere.
    amx.until("the viewer to draw the patch", || {
        amx.capture(&pane).contains("VIEWED +after").then_some(())
    });
}

#[test]
fn diff_leaves_the_viewer_out_down_a_pipe_and_under_stat() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");
    std::fs::write(tree.join("README.md"), "after\n").expect("the changed file");
    amx.config(VIEWER);
    let copy = amx.home().join("viewed.patch");

    // A caller reading the patch is a caller reading git's own patch, whatever
    // somebody set the key to for their own screen.
    let out = amx.amx(&["diff", "fix-login-a1b"]);
    assert!(out.status.success());
    let patch = String::from_utf8_lossy(&out.stdout);
    assert!(
        patch.contains("+after") && !patch.contains("VIEWED"),
        "{patch}"
    );
    assert!(!copy.exists(), "and nothing was run to read it");

    // And --stat is the shape of the work, which is not what a patch viewer is
    // handed: the terminal comes back rather than being held by one.
    let pane = amx.in_a_terminal(&[], &["diff", "fix-login-a1b", "--stat"]);
    amx.until(
        "the summary to be printed and the terminal given back",
        || (!amx.pane_alive(&pane)).then_some(()),
    );
    assert!(!copy.exists(), "{}", copy.display());
}

#[test]
fn clibatch_diff_stat_summarises_the_work_instead_of_printing_it() {
    // The question `--stat` answers is how far along an agent is, which a
    // hundred-file patch scrolling past does not.
    let amx = Harness::new();
    let repo = amx.a_repo();
    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");

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
    // The agent rebased its commit onto the release line, which the commit its
    // tree was cut from is not on. Measured from that commit the answer would
    // carry its work backwards -- the file it added deleted, the line it
    // changed changed back -- and read as the agent's.
    let amx = Harness::new();
    let repo = amx.a_repo();
    git(&repo, &["branch", "release"]);
    std::fs::write(repo.join("README.md"), "after\n").expect("a second version");
    std::fs::write(repo.join("shipped.rs"), "fn shipped() {}\n").expect("a shipped file");
    git(&repo, &["add", "shipped.rs"]);
    git(&repo, &["commit", "-am", "second"]);
    let cut_from = git(&repo, &["rev-parse", "HEAD"]);

    let tree = with_a_worktree(&amx, "fix-login-a1b", &repo, "works-without-end");
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

    // Nothing has changed yet, and an empty patch is an answer rather than the
    // refusal it used to be.
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

    // What the directory changes is the agent's work, measured from the commit
    // it was standing on when the session started.
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
fn clibatch_rename_puts_the_word_where_a_program_reads_it() {
    // A rename is for the eye, but the wall is not the only thing with rows to
    // draw. What somebody calls an agent is on `--json` beside the id every
    // surface still addresses it by, so a program listing agents can label
    // them the way the person who renamed them does.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["rename", "fix-login-a1b", "auth"]);
    assert!(
        out.status.success(),
        "amx rename: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).expect("the listing is json");
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

#[test]
fn a_dropped_harness_takes_its_tmux_socket_with_it() {
    let path = {
        let amx = Harness::new();
        // A session keeps the server alive, and the server makes the socket.
        amx.tmux(&["new-session", "-d", "-s", "keep", "sleep", "60"]);
        let path = PathBuf::from(amx.tmux(&["display-message", "-p", "#{socket_path}"]));
        assert!(path.exists(), "no socket at {}", path.display());
        path
    };
    // Dead sockets piled up in /tmp/tmux-1000 by the thousand until tmux
    // itself timed out opening the directory (friction #G40BJA0X).
    assert!(!path.exists(), "{} outlived its harness", path.display());
}
