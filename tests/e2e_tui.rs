//! The agent view: what a person sees in it, and what looking does.
//!
//! Every one of these drives the view in a real tmux pane, because a view is
//! only a view when something is drawing it on a terminal: what it puts on the
//! screen, what a keypress does to that, and what it leaves behind when it
//! closes are all questions a pty answers and nothing else does.

mod common;

use common::Harness;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch seconds, for the records a test writes as though they had just
/// happened.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

/// An agent whose command has ended: no pane, and the record is the whole
/// story. `ago` is how long since it ended, which is what orders them.
fn finished(amx: &Harness, id: &str, state: &str, ago: u64) {
    let at = now() - ago;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": state,
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "did what it was asked",
        }),
    );
}

/// What is on the view's screen now.
fn screen(amx: &Harness, pane: &str) -> String {
    amx.capture(pane)
}

#[test]
fn the_view_gathers_the_agents_under_what_they_need() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-importer-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-importer-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("every group", || {
        let drawn = screen(&amx, &view);
        ["needs input", "working", "idle", "completed"]
            .iter()
            .all(|group| drawn.contains(group))
            .then_some(drawn)
    });

    for id in [
        "ask-a1b",
        "port-importer-b2c",
        "fix-login-c3d",
        "old-job-d4e",
    ] {
        assert!(drawn.contains(id), "{id} is missing from:\n{drawn}");
    }
    // A row says what the agent is up to: what it is asking, else what it is
    // doing, else what it answered.
    assert!(drawn.contains("Claude needs your permission"), "{drawn}");
    assert!(drawn.contains("Running Bash"), "{drawn}");
    assert!(drawn.contains("did what it was asked"), "{drawn}");

    // Twice over: the count at the top, and the group the rows sit under.
    for group in ["needs input", "completed"] {
        assert_eq!(
            drawn.matches(group).count(),
            2,
            "{group} is counted at the top and named over its rows:\n{drawn}"
        );
    }
}

#[test]
fn the_view_says_when_there_is_nothing_to_show() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);

    amx.until("the empty view", || {
        screen(&amx, &view).contains("no agents").then_some(())
    });
}

#[test]
fn completed_agents_fold_into_a_count_until_they_are_opened() {
    let amx = Harness::new();
    for (n, id) in ["one-a1b", "two-b2c", "three-c3d", "four-d4e", "five-e5f"]
        .iter()
        .enumerate()
    {
        finished(&amx, id, "done", n as u64 * 60);
    }

    let view = amx.in_a_terminal(&[], &[]);
    let folded = amx.until("the fold", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("2 more")).then_some(drawn)
    });
    assert!(
        !folded.contains("five-e5f"),
        "the oldest are behind the count:\n{folded}"
    );

    // Down onto the fold — three agents are shown, so it is the fourth row —
    // and open it.
    amx.tmux(&["send-keys", "-t", &view, "Down", "Down", "Down", "Enter"]);
    amx.until("the rest of them", || {
        screen(&amx, &view).contains("five-e5f").then_some(())
    });
}

#[test]
fn space_peeks_at_the_pane_and_at_what_it_is_asking() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    amx.tmux(&["send-keys", "-t", &view, "Space"]);
    let peeked = amx.until("the peek", || {
        let drawn = screen(&amx, &view);
        drawn.contains("rm -rf build").then_some(drawn)
    });

    // The screen the agent is sitting on, and the question it is sitting on it
    // for — which is on its row as well, so the peek is the second of the two.
    assert!(peeked.contains("Do you want to proceed?"), "{peeked}");
    assert_eq!(
        peeked.matches("Claude needs your permission").count(),
        2,
        "the row asks it and the peek asks it again:\n{peeked}"
    );
}

#[test]
fn enter_puts_the_agent_in_front_of_the_terminal() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let session = amx.tmux(&["display-message", "-p", "-t", &view, "#{session_id}"]);

    // An agent in a window beside the view's, which nobody is looking at.
    let pane = amx.tmux(&[
        "new-window",
        "-d",
        "-t",
        &session,
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "printf 'the agent at work\\n'; while :; do sleep 0.05; done",
    ]);
    amx.record("fix-login-a1b", &pane);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    amx.tmux(&["send-keys", "-t", &view, "Enter"]);
    amx.until("the agent's window to come forward", || {
        let active = amx.tmux(&["display-message", "-p", "-t", &pane, "#{window_active}"]);
        (active == "1").then_some(())
    });
    assert_eq!(
        amx.tmux(&["display-message", "-p", "-t", &pane, "#{pane_active}"]),
        "1",
        "and the agent's own pane within it"
    );
    assert!(
        amx.pane_alive(&view),
        "the view is still there to come back to"
    );
}

#[test]
fn q_closes_the_view_and_gives_the_screen_back() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the view", || {
        screen(&amx, &view).contains("no agents").then_some(())
    });

    // Keep the pane after its command ends, so what it ended with can be read.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    amx.tmux(&["send-keys", "-t", &view, "q"]);

    amx.until("the view to close", || {
        let dead = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead}"]);
        (dead == "1").then_some(())
    });
    assert_eq!(
        amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead_status}"]),
        "0",
        "closing a view is not a failure"
    );
    assert!(
        !screen(&amx, &view).contains("no agents"),
        "the screen the view borrowed is handed back"
    );
}
