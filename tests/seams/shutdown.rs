//! The persist↔core shutdown seam, and the tripwire over it (R-M1-2).
//!
//! Two claims, one mechanism apart:
//!
//! - **A change made inside the debounce window survives a clean stop.**
//!   `Core::run`'s break path `try_send`s a final cheap capture to `Persist`
//!   before it drops the mailbox; `Persist`, already cancelled, arms nothing,
//!   drains to closure, writes once and returns (`docs/07-m1-plan.md` §2). A
//!   rename made a moment before `SIGTERM` is the smallest thing that proves
//!   the whole chain ran, because nothing else can have put it on disk.
//! - **Repeating that never wedges.** A rare `SIGTERM`-immune hang in the
//!   `JoinSet` drain has been seen under load and is not diagnosed (R-M1-2).
//!   M1 adds a fourth actor to that drain, so the milestone owes evidence it
//!   made the hang no more likely: [`repeated_clean_shutdowns_under_load_leave_no_hung_drain`]
//!   is a canary, not a proof — a rare flake cannot be disproven by a bounded
//!   loop, and this one is written to be *loud* rather than lucky.
//!
//! The tripwire's own rule: it must never hang the suite it is protecting.
//! Every stop runs under [`STOP_BUDGET`] rather than the rig's patience, and a
//! server that overruns it is killed, reported with the state of every one of
//! its threads, and the loop moves on to fail at the end. A wedge left running
//! would hold the socket and the panes' children against the next repetition
//! and turn one hang into a suite that never returns.

use std::time::Duration;

use rig::env::processes_with_arg;
use rig::{Env, wait_until};

use crate::fixtures::{
    connected, dir, marker_shell, rename, snapshot_mentions, split_in, workspace,
};

/// How long one clean stop may take before it counts as wedged.
///
/// Deliberately far below the rig's 60 s [`rig::PATIENCE`]: a healthy stop is
/// a cancellation token, four actors returning and a `JoinSet` emptying, which
/// is milliseconds even on a loaded shared runner, and the thing being watched
/// for is a drain that never finishes at all. Generous enough that a stalled
/// scheduler cannot redden it, short enough that ten wedges cost a minute.
const STOP_BUDGET: Duration = Duration::from_secs(15);

/// How many populate-and-stop cycles the tripwire runs.
///
/// Bounded on purpose. The wedge is rare and a loop long enough to catch it
/// reliably would be a loop too long to run on every commit; what this number
/// buys is that M1's shutdown path is exercised end to end, repeatedly, with a
/// live session behind it, on every CI run on both tier-1 platforms.
const CYCLES: usize = 6;

// ------------------------------------------- the final capture on the way down

#[tokio::test]
async fn a_change_inside_the_debounce_window_survives_a_clean_stop() {
    let shell = marker_shell("fina", ":");
    let mut env = Env::new("fina");
    env.set_var("SHELL", &shell.path());

    let server = env.server();
    let mut wire = connected(&env).await;
    let (_, root) = workspace(&mut wire, "work").await;
    let pane = split_in(&mut wire, root, &dir(&env.scratch(), "work")).await;
    rename(&mut wire, pane, "before").await;
    // Let the ordinary debounced save land first, so the state on disk is
    // known and the rename below is the only thing that can be missing from
    // it. Waiting on the consequence, not on the clock.
    wait_until("the debounced save records the session", || {
        snapshot_mentions(&env, "before")
    });

    // The change under test, and then the signal, with nothing in between: at
    // 500 ms of quiet before a save fires and 5 s from the first unsaved
    // change (D-M1-7), the debounce cannot have run in the microseconds a
    // reply and a kill take. If a stalled scheduler ever does let it beat the
    // signal, the claim being asserted — the change is durable across a clean
    // stop — is the same one, reached by the other path.
    rename(&mut wire, pane, "after").await;
    server.shutdown();

    assert!(
        snapshot_mentions(&env, "after"),
        "a rename made inside the debounce window was lost on a clean stop; \
         the snapshot holds {:?}",
        crate::fixtures::snapshot(&env)
    );
    assert!(
        !snapshot_mentions(&env, "before"),
        "the final capture wrote the session as it was, not as it had been saved"
    );

    // And it comes back, which is the only form of "saved" that matters.
    let server = env.server();
    let mut wire = connected(&env).await;
    let state = crate::fixtures::session_state(&mut wire).await;
    let labels: Vec<Option<&str>> = state["panes"]
        .as_array()
        .expect("session.state carries panes")
        .iter()
        .map(|pane| pane["label"].as_str())
        .collect();
    assert!(
        labels.contains(&Some("after")),
        "the restored session carries the label the final capture saved: {labels:?}"
    );
    server.shutdown();
}

// ----------------------------------------------------------------- the tripwire

#[tokio::test]
async fn repeated_clean_shutdowns_under_load_leave_no_hung_drain() {
    let shell = marker_shell("wedg", ":");
    let mut env = Env::new("wedg");
    env.set_var("SHELL", &shell.path());
    // Sidecars on, so every cycle's stop has a `history/` dump to serialize
    // and a blocking-pool write to finish: the drain being watched is the one
    // with work in it, not an idle one.
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    let mut wedged = Vec::new();
    for cycle in 0..CYCLES {
        let server = env.server();
        let mut wire = connected(&env).await;

        // Load, in the sense the wedge was seen under: a session with panes,
        // structural churn dirtying the persistence debounce, and live shells
        // whose pty readers and parsers are threads the drain has to outlive.
        let (_, root) = workspace(&mut wire, &format!("w{cycle}")).await;
        let first = split_in(&mut wire, root, &env.scratch()).await;
        let second = split_in(&mut wire, first, &env.scratch()).await;
        rename(&mut wire, first, &format!("first-{cycle}")).await;
        rename(&mut wire, second, &format!("second-{cycle}")).await;
        wait_until("the cycle's panes are all running", || {
            processes_with_arg(shell.marker()) >= 3
        });
        // A save in flight when the signal arrives is the interesting case:
        // `Persist` is then between a capture and a blocking write, which is
        // where a drain that waited on a sibling would deadlock.
        wait_until("the cycle's save lands", || {
            snapshot_mentions(&env, &format!("second-{cycle}"))
        });
        drop(wire);

        if let Err(stuck) = server.shutdown_within(STOP_BUDGET) {
            wedged.push(format!("cycle {cycle}: {stuck}"));
        }
        // Between cycles: the next server must not inherit the last one's
        // orphans, or its own "the panes are running" wait passes on someone
        // else's processes.
        wait_until("the stopped server's shells are reaped", || {
            processes_with_arg(shell.marker()) == 0
        });
    }

    assert!(
        wedged.is_empty(),
        "{} of {CYCLES} clean shutdowns did not finish draining — this is the \
         R-M1-2 wedge, caught:\n\n{}",
        wedged.len(),
        wedged.join("\n\n")
    );
}
