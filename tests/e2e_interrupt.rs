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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long the row is given to come off `working` once the key has landed.
///
/// The vendor says nothing about a turn it was interrupted out of, so the only
/// account of this one ending is a reader's, off the screen the vendor drew
/// when it went back to its prompt. Nothing about that screen has to hold
/// still first: amx ended the turn itself and the record says when, so the
/// prompt is read on the first look. This is that look, and room around it for
/// a loaded machine.
const SETTLES: Duration = Duration::from_secs(10);

/// How soon after the prompt is drawn the row has to say so.
///
/// The wait a turn nobody cut short is owed is half a minute, because a prompt
/// and a pause mid-turn are the same bytes. This one is owed none of it, and
/// five seconds is short enough that sitting any of it out would fail here.
const AT_ONCE: Duration = Duration::from_secs(5);

/// The clock the record keeps its own times by.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock set later than 1970")
        .as_secs()
}

/// Wait for the reader to call the row idle, and answer with what it said.
///
/// The harness's own `until` is not this wait: it is patient in seconds an
/// agent takes to speak, and this one is measured by how long a reader takes
/// to go to a pane and take what is on it for the end of a turn.
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
    let started = amx.until_state("port-importer-c3d", "working");

    // What amx writes down about the turn it is about to end outranks the
    // vendor's last word by being newer than it, and the record keeps both in
    // whole seconds. Coming up, starting a turn and being interrupted inside
    // one of them is this suite rather than anybody's afternoon, so the key
    // waits for the second the hooks landed in to pass.
    let spoke = started["last_event"]
        .as_u64()
        .expect("the record says when it last heard from the vendor");
    amx.until("the second the vendor last spoke in to pass", || {
        (now() > spoke).then_some(())
    });

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
    // the pane that ends it rather than anything the agent said — and it says
    // so on the look that finds the prompt, with no screen to sit out.
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
