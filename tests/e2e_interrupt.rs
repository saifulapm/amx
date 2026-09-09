//! Cutting a turn short at the pane, and what amx says about it afterwards.
//!
//! `amx interrupt` types one key at a pane and hears nothing back. Everything
//! that makes it a verb rather than a keystroke happens after that, in three
//! places at once: the vendor stops, the log says a turn was cut short, and a
//! caller waiting on the answer is told none is coming. So the stand-in blocks
//! until the key it is waiting for actually arrives on its stdin — a scenario
//! that drew its prompt on a timer would prove the pane was there and nothing
//! about the key reaching it.

mod common;

use common::Harness;
use std::time::{Duration, Instant};

/// How long the row is given to come off `working` once the key has landed.
///
/// The vendor says nothing about a turn it was interrupted out of, so the only
/// account of this one ending is a reader's, off the screen the vendor drew
/// when it went back to its prompt. That screen is the one the idle rule may
/// not decide from until it has held still — thirty seconds, since an idle
/// pane and a pause mid-turn are the same bytes — and this is that wait with
/// room around it for a loaded machine.
const SETTLES: Duration = Duration::from_secs(60);

/// Wait for the reader to call the row idle, and answer with what it said.
///
/// The harness's own `until` is not this wait: it is patient in seconds an
/// agent takes to speak, and this one is measured by how long a screen has to
/// stand still before a rule may end a turn nobody sent a hook for.
fn until_idle(amx: &Harness, id: &str) -> serde_json::Value {
    let deadline = Instant::now() + SETTLES;
    loop {
        let out = amx.amx(&["status", id, "--json"]);
        assert!(
            out.status.success(),
            "amx status: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let agent: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("the status is json");
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

    // The key reached the vendor and not only the record: this scenario sits
    // on its stdin until byte 27 arrives, and draws its prompt only then.
    amx.until("the vendor to go back to its prompt", || {
        amx.capture(&pane).contains("⏵⏵").then_some(())
    });

    // Which is the whole of what says the turn is over, so it is a reader at
    // the pane that ends it rather than anything the agent said.
    let agent = until_idle(&amx, "port-importer-c3d");
    assert_eq!(agent["evidence"], "screen", "{agent}");
    assert_eq!(agent["rule"], "idle_prompt", "{agent}");
    let kinds = amx.event_kinds("port-importer-c3d");
    assert!(
        !kinds.iter().any(|kind| kind == "Stop"),
        "the hooks are written around turns that run to their end: {kinds:?}"
    );

    // And a caller that was waiting on the answer is told at once that there
    // is none, rather than being handed another turn's.
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

#[test]
fn an_agent_sitting_at_its_prompt_has_no_turn_to_interrupt() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");

    let out = amx.amx(&["interrupt", "fix-login-a1b"]);
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("nothing is running to interrupt"), "{said}");

    // Nothing was cut short, so nothing on the log says one was: a `result`
    // reading this agent must not find an ending that never happened.
    let kinds = amx.event_kinds("fix-login-a1b");
    assert!(
        !kinds.iter().any(|kind| kind == "interrupt"),
        "a refusal writes nothing down: {kinds:?}"
    );
}
