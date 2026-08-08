//! W09 acceptance, over the real binary: a standing verb rides a server going
//! away and coming back (`docs/09-m3-plan.md` D-M3-7).
//!
//! Process-level, like every suite beside it, and for the reason this one
//! makes sharpest: what is under test is a `SIGKILL`ed server, a session
//! restored from disk in a *second* process, and a third process that was
//! waiting through both. None of that can be observed from inside one test
//! pretending to be several.
//!
//! # What the predicate contract buys, and how it is proved here
//!
//! 04 §2: waits are **state** predicates, never event predicates. That is what
//! makes the re-issue after a reconnect exact rather than lucky — a transition
//! that fired while nothing was connected is simply *true* when the call is
//! made again, so a wait cannot miss it. Two tests, because the claim has two
//! halves: the transition fires after the server is back
//! (`a_standing_wait_survives_a_server_restart_and_returns_on_the_predicate`),
//! and the transition fires while nothing is running at all
//! (`a_transition_that_fires_during_the_gap_still_returns_the_wait`). The
//! second is the one the contract exists for.
//!
//! Firing a transition inside a gap needs a hand that is not the server's, and
//! the honest one is `[terminal] shell`: the config is read per spawn
//! (`config_rt::shell`), so pointing it at a program that exits immediately
//! makes the restored pane's process die during the restore — which
//! `session/serve.rs` places before the accept loop ("the first client that can
//! possibly connect already sees the restored session"). The pane survives, its
//! process does not, and nothing that reconnects afterwards could have caused
//! it, because there is no process left to exit. The program is
//! [`exits_immediately`]'s, written into the environment, because a shell the
//! restore cannot spawn is a *pruned pane* rather than a pane whose process
//! exited — a different test entirely, and the one a hard-coded `/bin/true`
//! quietly became on macOS.
//!
//! Every "the standing verb is connected now" is *observed*, never slept
//! through: a second connection subscribes to the bus and waits for the
//! `client_attached` the standing verb's own connection publishes.
//!
//! `amx events --json` is here rather than in `events_cli.rs` because the
//! redial it grew is this task's, and the two halves of its contract — resume
//! from the cursor, or report the gap — are one test.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::time::Duration;

use amx_server::session::probe::probe;
use serde_json::{Value, json};

use harness::{
    Session1, Standing, await_attach, exits_immediately, flood, rename, reply, shell, state,
    watcher,
};
use support::wait_until;

#[path = "wait_retry/harness.rs"]
mod harness;

#[tokio::test(flavor = "multi_thread")]
async fn a_standing_wait_survives_a_server_restart_and_returns_on_the_predicate() {
    let mut session = Session1::new("wr-a");
    let pane = session.pane.clone();

    let mut watch = watcher(&session.env).await;
    let mut waiting = Standing::start(
        &session.env,
        "wait",
        &[
            "wait",
            "--until",
            "exited",
            "--target",
            &pane,
            "--timeout",
            "25s",
        ],
    );
    await_attach(&mut watch).await;
    drop(watch);
    assert!(
        waiting.running(),
        "the shell has not exited, so nor has the wait"
    );

    session.kill_server();
    session.serve();
    // The pane came back with the id it had, which is what makes re-issuing
    // the caller's own params the right thing to do.
    assert_eq!(
        state(&session.env)["panes"][0]["pane"].as_str(),
        Some(pane.as_str()),
        "restore keeps pane identity"
    );
    assert!(
        waiting.running(),
        "the wait redialled instead of dying with its connection"
    );

    // Now fire the transition, on the far side of the gap.
    session
        .env
        .run(&[
            "pane",
            "run",
            "--params",
            &json!({ "target": pane, "text": "exit" }).to_string(),
        ])
        .ok();

    let done = waiting.finish();
    let answer = reply(&done);
    assert_eq!(answer["satisfied"], json!(true), "{answer}");
    assert_eq!(answer["pane"].as_str(), Some(pane.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transition_that_fires_during_the_gap_still_returns_the_wait() {
    let mut session = Session1::new("wr-b");
    let pane = session.pane.clone();

    let mut watch = watcher(&session.env).await;
    let mut waiting = Standing::start(
        &session.env,
        "wait",
        &[
            "wait",
            "--until",
            "exited",
            "--target",
            &pane,
            "--timeout",
            "25s",
        ],
    );
    await_attach(&mut watch).await;
    drop(watch);
    assert!(waiting.running());

    session.kill_server();
    // The restored pane's process exits the moment it is spawned: started by
    // the restore and over before the successor's accept loop starts, so the
    // wait's predicate becomes true at a moment when the wait has no connection
    // to anything.
    shell(&session.env, &exits_immediately(&session.env));
    session.serve();

    // Stated, because the wait below depends on it and the two failures read
    // nothing alike: a pane the restore could not respawn is pruned with its
    // workspace (D-M1-9), and the re-issued wait then reports a caller error —
    // "no pane … in this session" — which looks like a client that lost track
    // of its own question rather than a session that lost the pane.
    assert_eq!(
        state(&session.env)["panes"][0]["pane"].as_str(),
        Some(pane.as_str()),
        "the restored session must still hold the pane the wait is about"
    );

    let done = waiting.finish();
    let answer = reply(&done);
    assert_eq!(
        answer["satisfied"],
        json!(true),
        "a state predicate is true when it is re-asked, whenever it became \
         true: {answer}"
    );
    assert_eq!(answer["pane"].as_str(), Some(pane.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_retry_gives_up_at_the_deadline_with_an_honest_error() {
    let mut session = Session1::new("wr-c");
    let pane = session.pane.clone();

    // Two seconds of patience, stated by the caller. The redial spends that
    // and no more: a verb that outlasted its own `--timeout` because it was
    // busy redialling would be answering a question nobody asked.
    let mut watch = watcher(&session.env).await;
    let mut waiting = Standing::start(
        &session.env,
        "wait",
        &[
            "wait",
            "--until",
            "exited",
            "--target",
            &pane,
            "--timeout",
            "2s",
        ],
    );
    await_attach(&mut watch).await;
    drop(watch);
    assert!(waiting.running());

    let started = std::time::Instant::now();
    session.kill_server();

    let done = waiting.finish();
    let took = started.elapsed();
    let why = done.failed();
    assert!(
        why.contains("stopped answering") && why.contains("did not come back"),
        "the error names the session going away, not a broken pipe: {why}"
    );
    assert!(
        why.contains(&session.env.session),
        "and names which session: {why}"
    );
    assert!(
        took < Duration::from_secs(15),
        "it gave up near its own deadline, not at some constant: {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn events_json_resumes_from_its_cursor_or_reports_the_gap() {
    let mut session = Session1::new("wr-d");
    let pane = session.pane.clone();

    // Half one: the cursor. A relay reading the stream keeps its own place and
    // resubscribes there when the socket changes hands, so a swap is invisible
    // on stdout except that the sequence numbers keep going.
    let mut watch = watcher(&session.env).await;
    let mut relay = Standing::start(&session.env, "relay", &["events", "--json"]);
    await_attach(&mut watch).await;
    drop(watch);

    rename(&session.env, &pane, "before");
    wait_until("the pre-swap rename reached the relay", || {
        relay.stdout().contains("\"before\"")
    });

    let exe = session.env.exe();
    session
        .env
        .run(&[
            "session",
            "handoff",
            "--params",
            &json!({ "binary": exe.to_string_lossy() }).to_string(),
        ])
        .ok();
    // The exporter exits once the successor owns the session; the successor is
    // its child and not this harness's to reap.
    let mut exporter = session.server.take().expect("the exporter");
    let _ = exporter.wait();
    let socket = session.env.socket();
    wait_until("the successor answers", || {
        probe(&socket).is_ok_and(|state| state.is_running())
    });

    rename(&session.env, &pane, "after");
    wait_until("the post-swap rename reached the relay", || {
        relay.stdout().contains("\"after\"")
    });

    assert!(relay.running(), "the relay rode the swap");
    let streamed = relay.stop();
    let lines: Vec<Value> = streamed
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("a delivery per line"))
        .collect();
    let seqs: Vec<u64> = lines
        .iter()
        .filter_map(|line| line["seq"].as_u64())
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "the sequence space continued across the swap without a hole: \
         {seqs:?}\n{}",
        streamed.stderr
    );
    assert!(
        streamed.stderr.matches("subscribed at seq").count() >= 2,
        "and it resubscribed rather than starting over: {}",
        streamed.stderr
    );

    // Half two: the gap. A cursor the replay ring has already dropped is not
    // an error and not a silent skip — the first delivery says exactly which
    // sequences are gone.
    flood(&session.env).await;
    let behind = Standing::start(
        &session.env,
        "behind",
        &["events", "--json", "--after-seq", "1"],
    );
    wait_until("the resuming relay printed its first delivery", || {
        behind.stdout().lines().next().is_some()
    });
    let printed = behind.stop();
    let first: Value = serde_json::from_str(printed.stdout.lines().next().expect("a first line"))
        .expect("a delivery");
    assert_eq!(
        first["delivery"],
        json!("gap"),
        "resuming past the ring opens with a gap: {first}"
    );
    assert_eq!(first["from"], json!(2), "and says where the hole starts");
}
