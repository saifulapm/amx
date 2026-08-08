//! DR-16's second residual, over the real binary: `amx attach --pane` rides a
//! server going away and coming back.
//!
//! The full client learned this in M3 (`wait_retry.rs` is the verb half of the
//! same contract); the single-pane attach did not, and a handoff simply ended
//! it. Process-level for the reason every suite beside it is: what is under
//! test is a killed server, a session restored in a *second* process, and a
//! third process that was drawing a pane through both.
//!
//! `SIGKILL` and not `session stop`, and for the same reason `wait_retry.rs`
//! gives: a clean stop unlinks the socket and ends the session, and what an
//! attached terminal has to survive is a server that stops mid-sentence. The
//! restart is a *cold* one — a new `SessionId`, panes restored from the
//! snapshot — which is the harsher of the two swaps this client can meet, since
//! nothing it holds is still the successor's to recognise.
//!
//! Both halves are proved by making the pane *paint*, not by watching the
//! client stay alive: a process that survived the swap without rebinding looks
//! identical from outside until something has to arrive over the stream.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use serde_json::json;
use support::Env;

/// Shared with `wait_retry.rs` rather than copied: the session-on-its-own-
/// machine harness, its `/bin/sh` pinning and its kill-and-restart are exactly
/// what this test needs, and a second copy would be a second thing to keep
/// true. `dead_code` because this binary uses a fraction of it.
#[allow(dead_code, reason = "this suite drives a fraction of the harness")]
#[path = "wait_retry/harness.rs"]
mod harness;

use harness::Session1;

/// Type `text` into the pane and hand back nothing: what the test reads is the
/// terminal the client is drawing, not this command's reply.
///
/// The newline is what makes it visible: the pane's `/bin/sh` echoes the line
/// and then answers it, and either half carries the beacon.
fn beacon(env: &Env, pane: &str, text: &str) {
    env.run(&[
        "pane",
        "send-text",
        "--params",
        &json!({ "target": pane, "text": format!("{text}\n") }).to_string(),
    ])
    .ok();
}

#[test]
fn a_single_pane_attach_rides_a_server_restart_and_keeps_drawing() {
    let mut session = Session1::new("apsw");
    let pane = session.pane.clone();

    let mut term = session
        .env
        .spawn_on_tty(&["attach", "--pane", &pane, "--takeover"], 24, 80);

    // Bound and drawing on the first server.
    beacon(&session.env, &pane, "before-swap");
    term.wait_for(b"before-swap");

    session.kill_server();
    session.serve();

    // Bound and drawing on the second one. Nothing here re-typed anything the
    // client had sent: this is a fresh beacon through a fresh stream.
    beacon(&session.env, &pane, "after-swap");
    term.wait_for(b"after-swap");

    term.kill();
}
