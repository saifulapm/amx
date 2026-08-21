//! Bringing an agent back: the pane goes, the session does not.

mod common;

use common::Harness;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Output;

/// The id the vendor's stand-in announces for a session it was asked to
/// continue. A resume changes exactly this about an agent, so it is what the
/// tests watch for.
const CONTINUED: &str = "b7d2a5c8-3e14-4f9a-8c26-0d5b1a7e3f42";

/// An agent started the way a person starts one, playing `scenario`.
fn start(amx: &Harness, id: &str, dir: &Path, scenario: &str) {
    start_with(amx, id, dir, scenario, &[]);
}

/// The same, with arguments of the vendor's own after the separator.
fn start_with(amx: &Harness, id: &str, dir: &Path, scenario: &str, vendor: &[&str]) {
    let out = amx
        .amx_command(
            &[
                &[
                    "new",
                    "--name",
                    id,
                    "--dir",
                    &dir.to_string_lossy(),
                    "--agent",
                    &amx.mock(),
                    "fix the login bug",
                ][..],
                vendor,
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

/// How the vendor's stand-in says it was called, once it has said it.
fn argv_of(amx: &Harness, id: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until("the vendor to say how it was called", || {
        amx.capture(&pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    })
}

/// `amx resume`, with the stand-in ready to play a continued session.
fn resume(amx: &Harness, args: &[&str]) -> Output {
    amx.amx_command(&[&["resume"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
        .output()
        .expect("running amx resume")
}

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Wait for the vendor's stand-in to announce the session it was given.
fn until_continued(amx: &Harness, id: &str) {
    amx.until(&format!("{id} to be on its continued session"), || {
        (amx.meta(id)["session"] == CONTINUED).then_some(())
    });
}

#[test]
fn resume_brings_a_stopped_agent_back_on_the_session_it_had() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();

    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let out = resume(&amx, &[id]);
    assert!(said(&out).contains(id), "it says what came back");

    // The vendor was handed the session the agent already had, and not the
    // task it was started on: that work was asked for once.
    let pane = amx.pane_of(id);
    let called = amx.until("the vendor to say how it was called", || {
        let screen = amx.capture(&pane);
        screen.contains("argv:").then_some(screen)
    });
    assert!(called.contains(&format!("--resume={session}")), "{called}");
    assert!(!called.contains("fix the login bug"), "{called}");

    // And the record is the same agent, back at the beginning of a turn.
    until_continued(&amx, id);
    assert!(amx.pane_alive(&pane));
    assert_ne!(amx.state(id)["state"], "stopped");
    assert_eq!(amx.state(id)["exit"], json!(null), "it is running again");
}

#[test]
fn resume_refuses_an_agent_that_has_not_ended() {
    let amx = Harness::new();
    let id = "watch-log-c3d";
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    let pane = amx.pane_of(id);

    let out = resume(&amx, &[id]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an agent that is still going is not something to start again"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains(id));
    assert_eq!(amx.pane_of(id), pane, "and it was left where it was");
    assert_eq!(amx.state(id)["state"], "working");
}

#[test]
fn resume_two_racers_bring_back_one_agent_and_not_two() {
    // Two `amx resume <id>` in flight at once. One session may only be
    // continued once: two panes both running `--resume=<same session>` would
    // fight over one record, so the loser has to hear that the agent is
    // already going again.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let state_dir = amx
        .state_root()
        .parent()
        .expect("the state root has a parent")
        .to_path_buf();
    // Both racers are up and spinning before the starting gun fires, so they
    // reach the has-it-ended gate together instead of one whole run apart.
    let go = state_dir.join("go");
    let racers: Vec<_> = (0..2)
        .map(|_| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "until [ -e '{go}' ]; do :; done; exec '{amx}' resume '{id}'",
                    go = go.display(),
                    amx = common::AMX,
                ))
                .env("AMX_STATE_DIR", &state_dir)
                .env("HOME", amx.home())
                .env("XDG_CONFIG_HOME", amx.home().join(".config"))
                .env("AMX_TMUX_SOCKET", amx.socket())
                .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
                .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
                .env_remove("TMUX")
                .env_remove("TMUX_PANE")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("starting a racer")
        })
        .collect();
    std::fs::write(&go, b"").expect("the starting gun");
    let done: Vec<Output> = racers
        .into_iter()
        .map(|racer| racer.wait_with_output().expect("waiting for a racer"))
        .collect();

    let winners = done.iter().filter(|out| out.status.success()).count();
    assert_eq!(
        winners,
        1,
        "one resume owns the comeback: {}",
        done.iter()
            .map(|out| format!(
                "[exit {:?} out {:?} err {:?}]",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ))
            .collect::<Vec<_>>()
            .join(" ")
    );
    until_continued(&amx, id);
    assert!(
        amx.pane_alive(&amx.pane_of(id)),
        "and the record names the pane that is actually running"
    );
}

#[test]
fn resume_picks_up_an_agent_whose_command_ran_to_the_end() {
    // How an agent ended is not whether there is a session behind it. One that
    // finished has an answer and a session, and picking that session up is how
    // somebody carries on from it.
    let amx = Harness::new();
    let id = "say-hello-b2c";
    start(&amx, id, amx.home(), "finishes");
    amx.until_state(id, "done");

    said(&resume(&amx, &[id]));
    until_continued(&amx, id);
    assert_eq!(amx.state(id)["state"], "starting");
}

#[test]
fn resume_puts_a_background_agent_back_in_the_background() {
    // An agent started out of sight was put there on purpose — it is an
    // orchestrator's worker — and a resume must not move it onto the wall.
    let amx = Harness::new();
    let id = "quiet-fix-a1b";
    let out = amx
        .amx_command(&[
            "new",
            "--bg",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let out = resume(&amx, &[id]);
    assert!(said(&out).contains(id));
    let session = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &amx.pane_of(id),
        "#{session_name}",
    ]);
    assert_eq!(
        session,
        format!("amx-{id}"),
        "back where it was started, not on the wall"
    );
}

#[test]
fn resume_all_brings_back_everything_a_dead_server_took() {
    let amx = Harness::new();
    let ids = ["fix-login-a1b", "port-importer-c3d"];
    for id in ids {
        start(&amx, id, amx.home(), "happy-turn");
    }
    for id in ids {
        amx.until_state(id, "idle");
    }

    // The server dies, and every pane on it goes with it.
    amx.tmux(&["kill-server"]);

    let out = resume(&amx, &["--all"]);
    let printed = said(&out);
    for id in ids {
        assert!(printed.contains(id), "{printed}");
        until_continued(&amx, id);
        assert!(amx.pane_alive(&amx.pane_of(id)), "{id} has a pane again");
    }
}

#[test]
fn resume_all_says_so_when_there_is_nothing_to_bring_back() {
    let amx = Harness::new();
    let id = "watch-log-c3d";
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");

    let printed = said(&resume(&amx, &["--all"]));
    assert!(
        !printed.contains(id),
        "a running agent is not swept up: {printed}"
    );
    assert!(printed.contains("nothing"), "{printed}");
}

#[test]
fn resume_puts_back_the_tree_that_stopping_took_away() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let id = "fix-login-a1b";
    start(&amx, id, &repo, "happy-turn");
    amx.until_state(id, "idle");
    let worktree = PathBuf::from(
        amx.meta(id)["worktree"]
            .as_str()
            .expect("a worktree of its own"),
    );

    amx.amx(&["stop", id, "--force"]);
    assert!(!worktree.exists(), "stopping cleared the tree away");

    said(&resume(&amx, &[id]));
    assert!(worktree.exists(), "and resuming needs it back");
    assert!(
        worktree.join("README.md").exists(),
        "on the branch it was working on"
    );
    until_continued(&amx, id);
}

#[test]
fn clibatch_resume_hands_the_vendor_what_the_agent_was_started_with() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start_with(
        &amx,
        id,
        amx.home(),
        "happy-turn",
        &["--", "--add-dir", "/srv/data"],
    );
    amx.until_state(id, "idle");
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();

    amx.amx(&["stop", id, "--force"]);
    said(&resume(&amx, &[id]));

    let called = argv_of(&amx, id);
    assert!(
        called.contains("--add-dir /srv/data"),
        "a directory the agent was given access to is one it still needs: {called}"
    );
    assert!(called.contains(&format!("--resume={session}")), "{called}");
}

#[test]
fn clibatch_resuming_twice_over_asks_for_one_session_and_not_two() {
    // Each resume records what it launched, so the second one reads a command
    // that already names a session. Two of them would leave which session the
    // vendor opens up to the vendor.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start_with(
        &amx,
        id,
        amx.home(),
        "happy-turn",
        &["--", "--add-dir", "/srv/data"],
    );
    amx.until_state(id, "idle");

    for _ in 0..2 {
        amx.amx(&["stop", id, "--force"]);
        said(&resume(&amx, &[id]));
        until_continued(&amx, id);
    }

    let called = argv_of(&amx, id);
    assert_eq!(
        called.matches("--resume").count(),
        1,
        "one flag, whatever the vendor was launched with before: {called}"
    );
    assert!(
        called.contains(&format!("--resume={CONTINUED}")),
        "{called}"
    );
    assert!(called.contains("--add-dir /srv/data"), "{called}");
}

#[test]
fn resume_says_so_when_there_is_no_session_to_continue() {
    let amx = Harness::new();
    let id = "never-hooked-a1b";
    // A record whose agent never announced a session: nothing was ever
    // started that could be picked up again.
    amx.record(id, "%99");
    amx.set_state(id, json!({ "state": "stopped" }));

    let out = resume(&amx, &[id]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("session"), "{why}");
    assert!(why.contains("amx new"), "and what to do instead: {why}");
}

#[test]
fn resume_will_not_take_the_machine_past_max_agents() {
    let amx = Harness::new();
    amx.config("max_agents = 1\n");
    amx.play("watch-log-c3d", "works-without-end");
    amx.until_state("watch-log-c3d", "working");

    let id = "fix-login-a1b";
    amx.record(id, "%99");
    amx.set_state(id, json!({ "state": "stopped" }));

    // The cap is about what the machine is already running, so it is answered
    // before anything about this agent is.
    let out = resume(&amx, &[id]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("max_agents"));
}

#[test]
fn resume_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = resume(&amx, &["never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}
