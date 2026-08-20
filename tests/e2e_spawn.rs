//! Starting an agent, and what that leaves behind.

mod common;

use common::Harness;
use std::path::Path;
use std::process::Output;

/// `amx new`, with the vendor pointed at a scenario.
fn new(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new")
}

fn id_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    let id = printed.trim().to_string();
    assert!(!id.is_empty(), "new prints the id and nothing else");
    assert_eq!(printed.lines().count(), 1, "one line: {printed:?}");
    id
}

#[test]
fn new_starts_an_agent_and_prints_its_id() {
    let amx = Harness::new();
    let mock = amx.mock();
    let out = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "fix the login bug"],
    );

    let id = id_of(&out);
    assert!(id.starts_with("fix-the-login-bug-"), "{id}");

    let state = amx.until_state(&id, "idle");
    assert_eq!(state["result"], "the tests pass now");

    let meta = amx.meta(&id);
    assert_eq!(meta["task"], "fix the login bug");
    assert_eq!(meta["socket"]["name"], amx.socket());
    assert!(
        amx.pane_alive(meta["pane"].as_str().unwrap()),
        "the pane is on the server the record names"
    );
}

#[test]
fn the_task_never_rides_the_tmux_command_line() {
    // A task is arbitrary text and a tmux command line is not a place for it.
    // It travels in a file only its owner can read, and the pane is started
    // with nothing but an id.
    let amx = Harness::new();
    let mock = amx.mock();
    let out = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--agent",
            &mock,
            "fix $(whoami); rm -rf \"everything\"",
        ],
    );
    let id = id_of(&out);
    let pane = amx.meta(&id)["pane"].as_str().unwrap().to_string();

    let started = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_start_command}",
    ]);
    assert!(started.contains("_boot"), "{started}");
    assert!(started.contains(&id), "{started}");
    assert!(
        !started.contains("$(") && !started.contains("rm -rf"),
        "nothing of the task's own syntax is on the command line: {started}"
    );

    let handoff = amx.handoff(&id);
    assert_eq!(handoff["task"], "fix $(whoami); rm -rf \"everything\"");
    assert_eq!(
        mode(&amx.agent_dir(&id).join("handoff.json")) & 0o777,
        0o600,
        "the handoff carries the environment, so it is the owner's alone"
    );
}

#[test]
fn the_agent_gets_the_environment_new_was_run_with() {
    // A tmux server started an hour ago has an hour-old environment. The
    // agent's comes from the command that asked for it, not from the server.
    let amx = Harness::new();
    let mock = amx.mock();
    let out = amx
        .amx_command(&[
            "new",
            "--no-worktree",
            "--agent",
            &mock,
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
        .env("ANTHROPIC_MODEL", "opus")
        .env("TMUX_PANE", "%404")
        .output()
        .expect("running amx new");

    let id = id_of(&out);
    let handoff = amx.handoff(&id);

    assert_eq!(handoff["env"]["ANTHROPIC_MODEL"], "opus");
    assert!(
        handoff["env"]["TMUX"].is_null() && handoff["env"]["TMUX_PANE"].is_null(),
        "tmux's own variables belong to the pane it makes, not to the one it left"
    );
    assert_eq!(
        handoff["env"]["AMX_ID"], id,
        "and the agent knows who it is"
    );
}

#[test]
fn agents_started_from_inside_tmux_tile_into_one_wall() {
    let amx = Harness::new();
    let mock = amx.mock();
    let inside = amx.inside_tmux();

    let first = id_of(
        &amx.amx_command(&["new", "--no-worktree", "--agent", &mock, "the first"])
            .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
            .envs(inside.clone())
            .output()
            .unwrap(),
    );
    let second = id_of(
        &amx.amx_command(&["new", "--no-worktree", "--agent", &mock, "the second"])
            .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
            .envs(inside)
            .output()
            .unwrap(),
    );

    let window_of = |id: &str| {
        let pane = amx.meta(id)["pane"].as_str().unwrap().to_string();
        amx.tmux(&["display-message", "-p", "-t", &pane, "#{window_id}"])
    };
    let (first_window, second_window) = (window_of(&first), window_of(&second));

    assert_eq!(first_window, second_window, "one wall, not two");
    assert_eq!(
        amx.tmux(&[
            "display-message",
            "-p",
            "-t",
            &first_window,
            "#{window_name}"
        ]),
        "amx-wall"
    );
    assert_eq!(
        amx.tmux(&["list-panes", "-t", &first_window, "-F", "#{pane_id}"])
            .lines()
            .count(),
        2,
        "both agents are on it"
    );
}

#[test]
fn an_agent_started_in_the_background_is_out_of_the_way() {
    let amx = Harness::new();
    let mock = amx.mock();
    let inside = amx.inside_tmux();

    let id = id_of(
        &amx.amx_command(&["new", "--bg", "--no-worktree", "--agent", &mock, "quietly"])
            .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
            .envs(inside)
            .output()
            .unwrap(),
    );

    let pane = amx.meta(&id)["pane"].as_str().unwrap().to_string();
    let window = amx.tmux(&["display-message", "-p", "-t", &pane, "#{window_name}"]);
    assert_ne!(window, "amx-wall", "a background agent is not on the wall");

    let session = amx.tmux(&["display-message", "-p", "-t", &pane, "#{session_id}"]);
    assert_eq!(
        amx.tmux(&["show-options", "-t", &session, "-v", "destroy-unattached"]),
        "off",
        "and it outlives whoever is not watching it"
    );
    amx.until_state(&id, "idle");
}

#[test]
fn new_gives_an_agent_its_own_worktree_in_a_repository() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let meta = amx.meta(&id);
    let worktree = Path::new(meta["worktree"].as_str().expect("a worktree"));
    assert_eq!(worktree, repo.join(".amx/worktrees").join(&id));
    assert!(
        worktree.join("README.md").exists(),
        "the repository's work is in it"
    );
    assert_eq!(meta["branch"], format!("amx/{id}"));
    assert!(
        meta["base"].as_str().unwrap().len() >= 7,
        "the commit it was cut from"
    );
    assert_eq!(meta["dir"], worktree.to_string_lossy().as_ref());
}

#[test]
fn new_runs_in_the_directory_as_it_is_when_asked() {
    let amx = Harness::new();
    let mock = amx.mock();
    let repo = amx.a_repo();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--dir",
            &repo.to_string_lossy(),
            "--no-worktree",
            "--agent",
            &mock,
            "fix the login bug",
        ],
    ));

    let meta = amx.meta(&id);
    assert!(meta["worktree"].is_null());
    assert_eq!(meta["dir"], repo.to_string_lossy().as_ref());
    assert!(!repo.join(".amx").exists(), "and nothing was cut");
}

#[test]
fn new_refuses_once_the_cap_is_reached() {
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config("max_agents = 1\n");

    let first = id_of(&new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the first"],
    ));
    amx.until_state(&first, "idle");

    let refused = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the second"],
    );
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("max_agents") || said.contains('1'), "{said}");
}

#[test]
fn an_agent_that_has_ended_does_not_hold_a_place() {
    let amx = Harness::new();
    let mock = amx.mock();
    amx.config("max_agents = 1\n");

    let first = id_of(&new(
        &amx,
        "finishes",
        &["--no-worktree", "--agent", &mock, "the first"],
    ));
    amx.until_state(&first, "done");

    let second = new(
        &amx,
        "happy-turn",
        &["--no-worktree", "--agent", &mock, "the second"],
    );
    assert!(
        second.status.success(),
        "an agent that is over is not one of the five: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn new_takes_the_name_it_is_given_and_refuses_one_twice() {
    let amx = Harness::new();
    let mock = amx.mock();

    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--name",
            "importer",
            "--agent",
            &mock,
            "port it",
        ],
    ));
    assert_eq!(id, "importer");

    let again = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--name",
            "importer",
            "--agent",
            &mock,
            "port it again",
        ],
    );
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("importer"));

    let wrong = new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--name",
            "Not An Id",
            "--agent",
            &mock,
            "port it",
        ],
    );
    assert!(!wrong.status.success());
}

#[test]
fn vendor_arguments_reach_the_vendor_untouched() {
    let amx = Harness::new();
    let mock = amx.mock();
    let id = id_of(&new(
        &amx,
        "happy-turn",
        &[
            "--no-worktree",
            "--agent",
            &mock,
            "fix the login bug",
            "--",
            "--session-id",
            "abc-123",
        ],
    ));

    let handoff = amx.handoff(&id);
    let command: Vec<&str> = handoff["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert_eq!(command[0], mock);
    assert!(
        command
            .windows(2)
            .any(|pair| pair == ["--session-id", "abc-123"]),
        "{command:?}"
    );
    assert_eq!(
        command.last(),
        Some(&"fix the login bug"),
        "the task is the last word, the way a prompt is"
    );
}

#[test]
fn a_directory_that_is_not_there_is_said_so_before_anything_is_made() {
    let amx = Harness::new();
    let mock = amx.mock();
    let refused = new(
        &amx,
        "happy-turn",
        &["--dir", "/nowhere/at/all", "--agent", &mock, "fix it"],
    );

    assert!(!refused.status.success());
    assert!(
        !amx.state_root().exists() || amx.state_root().read_dir().unwrap().next().is_none(),
        "a dispatch that failed leaves no half-made agent behind"
    );
}

#[test]
fn attach_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = amx.amx(&["attach", "never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("the file")
        .permissions()
        .mode()
}
