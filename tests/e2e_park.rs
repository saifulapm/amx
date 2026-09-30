//! Parking: closing an idle agent's pane, and resuming it later.
//!
//! No amx process watches for idle agents. The hook that ends a turn asks the
//! tmux server to run `_park` once, `park_after` seconds later, and `_park`
//! decides from what it finds then. These tests run the real hook, server,
//! record and pane.

mod common;

use common::{Harness, clients_on, ls, now, something_else_on_the_server, watching};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The session id mock-claude announces when asked to continue a session.
const CONTINUED: &str = "b7d2a5c8-3e14-4f9a-8c26-0d5b1a7e3f42";

/// Configure `park_after = 1`, the shortest timer that still exercises the
/// whole path.
fn parks_after_a_second(amx: &Harness) {
    amx.config("park_after = 1\n");
}

/// Start an agent with `amx new`, playing `scenario`.
///
/// Uses `new` so the record holds the session and command a resume needs, and
/// so the server that runs the timer gets this harness's state and home.
fn start(amx: &Harness, id: &str, scenario: &str) {
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
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

/// Pin agent `id` by writing the `view.json` a view's ctrl+t would write.
fn pinned_over_the_wall(amx: &Harness, id: &str) {
    let path = amx
        .state_root()
        .parent()
        .expect("the state root sits under the state directory")
        .join("view.json");
    let held = json!({ "arrangement": { "held": [id] } });
    std::fs::write(&path, serde_json::to_string_pretty(&held).expect("json"))
        .expect("writing what a view remembers");
}

/// Agent `id`'s row in `amx ls --json`.
fn row(amx: &Harness, id: &str) -> Value {
    ls(amx)
        .into_iter()
        .find(|agent| agent["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in the listing"))
}

/// Wait up to ten seconds for the pane to go.
///
/// The timer is set for one second; ten allows for a loaded machine while
/// still failing well before the harness's general patience.
fn until_the_pane_goes(amx: &Harness, pane: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !amx.pane_alive(pane) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{pane} was still there ten seconds after the turn ended");
}

/// Wait until the record has been idle for `park_after`'s one second, so
/// `_park` decides on the pane and not the clock.
fn until_the_second_is_up(amx: &Harness, id: &str) {
    let since = amx.state(id)["since"]
        .as_u64()
        .unwrap_or_else(|| panic!("no idle moment recorded for {id}"));
    amx.until("the idle second to pass", || (now() > since).then_some(()));
}

#[test]
fn an_idle_agent_nobody_is_watching_loses_its_pane_and_keeps_everything_else() {
    let amx = Harness::new();
    parks_after_a_second(&amx);
    let id = "fix-login-a1b";
    start(&amx, id, "happy-turn");
    something_else_on_the_server(&amx);
    amx.until_state(id, "idle");
    let pane = amx.pane_of(id);

    // No input and no client: the turn's end hook set the timer, and the
    // server fired it.
    until_the_pane_goes(&amx, &pane);

    let agent = row(&amx, id);
    assert_eq!(
        agent["state"], "idle",
        "the agent is where it was; only the pane went: {agent}"
    );
    assert_eq!(agent["evidence"], "parked", "{agent}");
    assert_eq!(
        agent["result"], "the tests pass now",
        "and what it answered is still on the record: {agent}"
    );
    assert!(
        amx.meta(id)["session"].is_string(),
        "with the conversation a resume picks up: {}",
        amx.meta(id)
    );
}

#[test]
fn resume_gives_a_parked_agent_its_pane_back() {
    let amx = Harness::new();
    parks_after_a_second(&amx);
    let id = "fix-login-a1b";
    start(&amx, id, "happy-turn");
    something_else_on_the_server(&amx);
    amx.until_state(id, "idle");
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();
    until_the_pane_goes(&amx, &amx.pane_of(id));

    let out = amx
        .amx_command(&["resume", id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
        .output()
        .expect("running amx resume");
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The vendor resumes the parked conversation in a new pane.
    let pane = amx.pane_of(id);
    let called = amx.until("the vendor to say how it was called", || {
        let screen = amx.capture(&pane);
        screen.contains("argv:").then_some(screen)
    });
    assert!(called.contains(&format!("--resume={session}")), "{called}");
    amx.until("the agent on its continued session", || {
        (amx.meta(id)["session"] == CONTINUED).then_some(())
    });
    assert!(amx.pane_alive(&pane));

    // With no message sent, the agent is back at its prompt on the continued
    // session, and no longer parked.
    let agent = row(&amx, id);
    assert_eq!(agent["state"], "idle", "{agent}");
    assert_ne!(
        agent["evidence"], "parked",
        "nothing amx let go is outstanding any more: {agent}"
    );
    assert_eq!(
        amx.state(id)["parked_at"],
        0,
        "and the stamp went with the pane it stood for"
    );
}

#[test]
fn a_pane_somebody_is_looking_at_is_left_where_it_is() {
    let amx = Harness::new();
    parks_after_a_second(&amx);
    let id = "fix-login-a1b";
    start(&amx, id, "happy-turn");
    let pane = amx.pane_of(id);

    // Attach a client before the turn ends, so `_park` finds one.
    let session = amx.tmux(&["display-message", "-p", "-t", &pane, "#{session_id}"]);
    watching(&amx, &session);
    amx.until("somebody looking at it", || {
        (!clients_on(&amx, &session).is_empty()).then_some(())
    });

    amx.until_state(id, "idle");
    until_the_second_is_up(&amx, id);
    let out = amx.amx(&["_park", id]);
    assert!(
        out.status.success(),
        "amx _park: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        amx.pane_alive(&pane),
        "the pane somebody is looking at is theirs"
    );
    assert_eq!(
        amx.state(id)["parked_at"],
        0,
        "and nothing on the record says amx let it go"
    );
}

#[test]
fn an_agent_pinned_over_the_wall_is_left_where_it_is() {
    let amx = Harness::new();
    parks_after_a_second(&amx);
    let id = "fix-login-a1b";
    // Pin before starting, so `_park` finds the pin. A pinned agent keeps its
    // pane.
    pinned_over_the_wall(&amx, id);
    start(&amx, id, "happy-turn");
    let pane = amx.pane_of(id);

    amx.until_state(id, "idle");
    until_the_second_is_up(&amx, id);
    let out = amx.amx(&["_park", id]);
    assert!(
        out.status.success(),
        "amx _park: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        amx.pane_alive(&pane),
        "the pinned agent is still in its pane"
    );
    assert_eq!(
        amx.state(id)["parked_at"],
        0,
        "and nothing on the record says amx let it go"
    );
}
