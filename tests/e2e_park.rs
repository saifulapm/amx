//! Letting an idle agent's pane go, and giving it back.
//!
//! Nothing in amx watches for the moment to do this: a hook asks the tmux
//! server holding the pane to run `_park` once, `park_after` seconds later, and
//! the verb decides against whatever it finds then. Every part of that is
//! somewhere else — the hook, the server, the record, the pane — so the whole
//! of it is only true here, where all four are real.

mod common;

use common::Harness;
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The id the vendor's stand-in announces for a session it was asked to
/// continue, which is what says a resume reached the vendor.
const CONTINUED: &str = "b7d2a5c8-3e14-4f9a-8c26-0d5b1a7e3f42";

/// A machine where an idle agent keeps its pane for a second.
///
/// The person's own file, which is where somebody sets this. One second is the
/// shortest wait that is still the whole path: the hook sets a timer, the
/// server fires it, and the verb reads the record again before it takes
/// anything.
fn parks_after_a_second(amx: &Harness) {
    amx.config("park_after = 1\n");
}

/// An agent started the way a person starts one, playing `scenario`.
///
/// `new` rather than a pane of the harness's own: what a park leaves behind has
/// to be an agent a resume can pick up, and the session and the command it
/// picks up are `new`'s to write. Starting the server is `new`'s too, so
/// everything the server hands the timer — the state directory, the home the
/// config is read from — is this harness's and not the developer's.
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

/// Something on the server that is not the agent under test, the way a machine
/// somebody works on has something else on it.
///
/// Losing the last pane takes the server with it, and a park is a test that
/// means to lose a pane: the server that starts again hands the next pane the
/// id the dead one had, which is a test measuring tmux rather than amx.
fn something_else_on_the_server(amx: &Harness) {
    amx.tmux(&[
        "new-session",
        "-d",
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
}

/// A person looking at the agent's session: a tmux client of their own, on a
/// terminal of its own, the way somebody who typed `tmux attach` has one.
///
/// tmux's two variables are cleared for it, because the pane the client starts
/// in is itself inside tmux and a client that knows that declines to nest.
fn watching(amx: &Harness, session: &str) {
    amx.tmux(&[
        "new-session",
        "-d",
        "--",
        "env",
        "-u",
        "TMUX",
        "-u",
        "TMUX_PANE",
        "tmux",
        "-L",
        amx.socket(),
        "-f",
        "/dev/null",
        "attach-session",
        "-t",
        session,
    ]);
}

/// What a view leaves behind when somebody pins a row with `ctrl+t`.
///
/// Written rather than typed into a view: a pin outlives the view that made it,
/// and this file is where a verb that is not a view reads it.
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

/// Every agent `ls --json` knows about.
fn ls(amx: &Harness) -> Vec<Value> {
    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the listing is json")
}

/// The one row this agent has in that listing.
fn row(amx: &Harness, id: &str) -> Value {
    ls(amx)
        .into_iter()
        .find(|agent| agent["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in the listing"))
}

/// Wait for the pane to go, and say so if it does not.
///
/// Ten seconds for a timer set for one: the wait is long enough that a loaded
/// machine is not what fails it, and short enough that a pane nobody comes for
/// is still a failure rather than the suite's own patience.
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

/// Wait until the record has been idle for the second `park_after` asks for, so
/// what `_park` decides on next is the pane rather than the clock.
fn until_the_second_is_up(amx: &Harness, id: &str) {
    let since = amx.state(id)["since"]
        .as_u64()
        .unwrap_or_else(|| panic!("no idle moment recorded for {id}"));
    amx.until("the idle second to pass", || (now() > since).then_some(()));
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs()
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

    // Nobody typed anything and nobody is attached: the hook that ended the
    // turn set the timer, and the server fired it.
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

    // The vendor was handed the conversation the agent was parked on, and it
    // is in a pane of its own again.
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

    // The idle the agent was parked in belonged to the turn before this one.
    // What the record reads now is a run at its beginning, on the session the
    // vendor announced when it picked the conversation up.
    let agent = row(&amx, id);
    assert_eq!(agent["state"], "starting", "{agent}");
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

    // Attached before the turn ends, so the timer the hook sets has somebody
    // to find at the pane it was set over.
    let session = amx.tmux(&["display-message", "-p", "-t", &pane, "#{session_id}"]);
    watching(&amx, &session);
    amx.until("somebody looking at it", || {
        (!amx
            .tmux(&["list-clients", "-t", &session, "-F", "#{client_tty}"])
            .is_empty())
        .then_some(())
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
    // Pinned before the agent is started, so the timer the first idle sets has
    // the pin to find: pinning a row is having said you want it in front of
    // you, and the pane it is in is what that means.
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
