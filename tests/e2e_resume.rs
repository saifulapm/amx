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
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &dir.to_string_lossy(),
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
