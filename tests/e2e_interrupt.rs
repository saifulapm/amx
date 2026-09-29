//! `amx interrupt`: cutting a turn short at the pane.
//!
//! The vendor sends nothing back for an interrupted turn. After the key, the
//! vendor must stop, the log must record the interrupt, and a caller waiting
//! on `result` must hear that no answer is coming. The `interrupted` scenario
//! blocks until Escape arrives on its stdin, so its prompt proves the key
//! reached the vendor.

mod common;

use common::{Harness, status};
use std::time::{Duration, Instant};

/// How long the row may take to leave `working` once the key has landed.
///
/// Only a reader looking at the pane can see the turn end. Since amx recorded
/// the interrupt, the prompt counts on the first look, with no settle time.
const SETTLES: Duration = Duration::from_secs(10);

/// How soon after the prompt is drawn the row must read idle.
///
/// An uninterrupted turn needs the screen to hold still for 30 seconds before
/// a prompt can end it. An interrupted one needs none of that, and five
/// seconds is short enough to catch a wait that should not happen.
const AT_ONCE: Duration = Duration::from_secs(5);

/// Poll `amx status` until the agent reads idle, failing after [`SETTLES`].
///
/// Shorter than [`Harness::until`], which allows for a vendor's reply time.
fn until_idle(amx: &Harness, id: &str) -> serde_json::Value {
    let deadline = Instant::now() + SETTLES;
    loop {
        let agent = status(amx, id);
        if agent["state"] == "idle" {
            return agent;
        }
        assert!(Instant::now() < deadline, "the row still reads {agent}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
fn interrupting_a_turn_ends_it_at_the_pane_with_no_word_from_the_vendor() {
    let amx = Harness::new();
    let pane = amx.play("port-importer-c3d", "interrupted");
    amx.until_state("port-importer-c3d", "working");

    // The key lands within a second of the turn starting. The interrupt amx
    // records must stand on its own against the hook just before it.
    let out = amx.amx(&["interrupt", "port-importer-c3d"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx interrupt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let kinds = amx.event_kinds("port-importer-c3d");
    assert!(
        kinds.iter().any(|kind| kind == "interrupt"),
        "the turn amx cut short is on the log: {kinds:?}"
    );

    // The scenario draws its prompt only after Escape reaches its stdin.
    amx.until("the vendor to go back to its prompt", || {
        amx.capture(&pane).contains("⏵⏵").then_some(())
    });

    // Only the screen says the turn is over, and the reader must say so on
    // the first look that finds the prompt.
    let drew = Instant::now();
    let agent = until_idle(&amx, "port-importer-c3d");
    assert!(
        drew.elapsed() < AT_ONCE,
        "the prompt was up {:?} before the row read idle",
        drew.elapsed()
    );
    assert_eq!(agent["evidence"], "screen", "{agent}");
    assert_eq!(agent["rule"], "idle_prompt", "{agent}");
    let kinds = amx.event_kinds("port-importer-c3d");
    assert!(
        !kinds.iter().any(|kind| kind == "Stop"),
        "the hooks are written around turns that run to their end: {kinds:?}"
    );

    // The vendor will never send the turn's end, so the reader records it,
    // once.
    let record = amx.state("port-importer-c3d");
    assert_eq!(record["state"], "idle", "{record}");
    assert!(record["question"].is_null(), "{record}");
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "read.turn-end").count(),
        1,
        "{kinds:?}"
    );

    // `result` says at once that there is no answer, instead of returning the
    // previous turn's.
    let asked = Instant::now();
    let waited = amx.amx(&["result", "port-importer-c3d", "--timeout", "10"]);
    assert_eq!(
        waited.status.code(),
        Some(1),
        "amx result: {}",
        String::from_utf8_lossy(&waited.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&waited.stdout),
        "",
        "a turn nobody let finish leaves nothing on stdout"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(5),
        "the wait was over before it started: {:?}",
        asked.elapsed()
    );
}

/// Start the harness's tmux server with `XDG_CONFIG_HOME` under this
/// harness's home.
///
/// The server's park timer runs `_park`, which reads `park_after` from the
/// config, and a server otherwise inherits the suite's `XDG_CONFIG_HOME`. The
/// extra session keeps the server alive after the park.
fn a_server_reading_this_config(amx: &Harness) {
    let out = std::process::Command::new("tmux")
        .args(["-L", amx.socket(), "-f", "/dev/null", "new-session", "-d"])
        .args(["--", "sh", "-c", "while :; do sleep 1; done"])
        .env("AMX_STATE_DIR", amx.state_root().parent().unwrap())
        .env("HOME", amx.home())
        .env("XDG_CONFIG_HOME", amx.home().join(".config"))
        .output()
        .expect("running tmux");
    assert!(
        out.status.success(),
        "tmux: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_turn_cut_short_at_the_pane_runs_the_idle_command_and_parks() {
    // A turn's end hook normally runs `on_idle` and sets the park timer. No
    // hook comes here, so the reader that records the end must do both, once.
    let amx = Harness::new();
    let said = amx.home().join("said-on_idle");
    amx.config(&format!(
        "park_after = 1\non_idle = \"{{ echo $AMX_STATE; cat; }} >> '{}'\"\n",
        said.display()
    ));
    a_server_reading_this_config(&amx);
    let id = "port-importer-c3d";
    let pane = amx.play(id, "interrupted");
    amx.until_state(id, "working");

    let out = amx.amx(&["interrupt", id]);
    assert_eq!(out.status.code(), Some(0));
    amx.until("the vendor to go back to its prompt", || {
        amx.capture(&pane).contains("⏵⏵").then_some(())
    });
    until_idle(&amx, id);

    let text = amx.until("the idle command to have run", || {
        let text = std::fs::read_to_string(&said).ok()?;
        (text.lines().count() >= 2).then_some(text)
    });
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "idle", "{text}");
    let event: serde_json::Value = serde_json::from_str(lines[1]).expect("the event");
    assert_eq!(event["kind"], "read.turn-end", "{text}");

    amx.until("the park timer to take the pane", || {
        (!amx.pane_alive(&pane)).then_some(())
    });
    for _ in 0..3 {
        amx.amx(&["status", id, "--json"]);
    }
    assert_eq!(
        std::fs::read_to_string(&said).unwrap().lines().count(),
        2,
        "one idle command for one turn's end"
    );
}

#[test]
fn an_agent_sitting_at_its_prompt_has_no_turn_to_interrupt() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["interrupt", "fix-login-a1b"]);
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("nothing is running to interrupt"), "{said}");

    // A refused interrupt must not leave an interrupt on the log for `result`
    // to find.
    let kinds = amx.event_kinds("fix-login-a1b");
    assert!(
        !kinds.iter().any(|kind| kind == "interrupt"),
        "a refusal writes nothing down: {kinds:?}"
    );
}
