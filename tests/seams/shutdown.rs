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
//!   M1 added a fourth actor to that drain and M2 a fifth, so each milestone
//!   owes evidence it made the hang no more likely:
//!   [`repeated_clean_shutdowns_under_load_leave_no_hung_drain`] is a canary,
//!   not a proof — a rare flake cannot be disproven by a bounded loop, and this
//!   one is written to be *loud* rather than lucky.
//!
//! **What M2 added to the canary's load** (R-M2-6). A canary that kept
//! exercising M1's four actors while a fifth sat idle beside them would be a
//! canary that had stopped tracking the thing it watches. So every cycle now
//! also starts a scripted agent and blocks it, which leaves `AgentHub` at the
//! signal with a live tracker, a compiled manifest, an entry in the attention
//! queue and an **armed deadline** — the state its drain has to let go of. Its
//! discipline is Persist's to the letter (receive-only after cancel, no sibling
//! request, nothing to flush), and this is where that claim is put under load.
//!
//! **What M3 added** (W01, `docs/09-m3-plan.md` §2). Two things the canary was
//! missing, both of them conditions the wedge has only ever been seen under.
//! First, a **client attached at the signal**: every sighting came out of the
//! T17 CLI suite, where a real client is on a real terminal when the server is
//! told to stop, and the gateway therefore has live connection tasks — with
//! grid streams inside them — to close before its own task can return. The
//! tripwire above signals a server nobody is looking at, which is the easy
//! case. Second, **repetition on demand**: both loops read their cycle count
//! from the environment ([`AMX_SHUTDOWN_CYCLES`], [`AMX_STORM_CYCLES`]) so the
//! spike harness can ask for thousands of stops without CI paying for them.
//! The defaults are what every commit runs; `scripts/spike/wedge.py` is what
//! turns them up.
//!
//! The tripwire's own rule: it must never hang the suite it is protecting.
//! Every stop runs under [`STOP_BUDGET`] rather than the rig's patience, and a
//! server that overruns it is killed, reported with the state of every one of
//! its threads, and the loop moves on to fail at the end. A wedge left running
//! would hold the socket and the panes' children against the next repetition
//! and turn one hang into a suite that never returns.

use std::time::Duration;

use amx_core::WorkspaceId;
use rig::agent::{self, FakeAgents};
use rig::env::processes_with_arg;
use rig::{ALT_ENTER, Env, Wire, result_of, wait_until};
use serde_json::json;

use crate::fixtures::{
    COLS, ROWS, connected, dir, focused_pane, marker_shell, rename, snapshot_mentions, split_in,
    workspace,
};

/// How long one clean stop may take before it counts as wedged.
///
/// Deliberately far below the rig's 60 s [`rig::PATIENCE`]: a healthy stop is
/// a cancellation token, four actors returning and a `JoinSet` emptying, which
/// is milliseconds even on a loaded shared runner, and the thing being watched
/// for is a drain that never finishes at all. Generous enough that a stalled
/// scheduler cannot redden it, short enough that ten wedges cost a minute.
const STOP_BUDGET: Duration = Duration::from_secs(15);

/// How many populate-and-stop cycles the tripwire runs by default.
///
/// Bounded on purpose. The wedge is rare and a loop long enough to catch it
/// reliably would be a loop too long to run on every commit; what this number
/// buys is that M1's shutdown path is exercised end to end, repeatedly, with a
/// live session behind it, on every CI run on both tier-1 platforms.
const CYCLES: usize = 6;

/// How many lean attach-and-stop cycles [`a_shutdown_storm_with_a_client_attached_never_wedges`]
/// runs by default.
///
/// Higher than [`CYCLES`] because the cycle is cheaper: no agent to block, no
/// save to wait out, so the wall clock per repetition is a server start and a
/// client's first frame. Same bargain, more samples of the same drain.
const STORM_CYCLES: usize = 20;

/// Overrides [`CYCLES`]. Read by the spike harness, unset everywhere else.
const AMX_SHUTDOWN_CYCLES: &str = "AMX_SHUTDOWN_CYCLES";

/// Overrides [`STORM_CYCLES`]. See [`AMX_SHUTDOWN_CYCLES`].
const AMX_STORM_CYCLES: &str = "AMX_STORM_CYCLES";

/// How many cycles to run: `default`, unless `var` names another number.
///
/// A value that is not a number is a typo in a spike invocation, and running
/// the default after one would waste the hours the invocation was asking for —
/// so it fails loudly instead.
fn cycles(var: &str, default: usize) -> usize {
    match std::env::var(var) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("${var} must be a number, not {value:?}")),
        Err(_) => default,
    }
}

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
    // The fifth actor's load (R-M2-6): a scripted agent per cycle, so the hub
    // is holding a tracker, a manifest, a queue entry and a timer when the
    // signal lands.
    let agents = FakeAgents::install(&mut env);
    let _ = agents.program();
    // Sidecars on, so every cycle's stop has a `history/` dump to serialize
    // and a blocking-pool write to finish: the drain being watched is the one
    // with work in it, not an idle one.
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    let rounds = cycles(AMX_SHUTDOWN_CYCLES, CYCLES);
    let mut wedged = Vec::new();
    for cycle in 0..rounds {
        let server = env.server();
        let mut wire = connected(&env).await;

        // Load, in the sense the wedge was seen under: a session with panes,
        // structural churn dirtying the persistence debounce, and live shells
        // whose pty readers and parsers are threads the drain has to outlive.
        let (space, root) = workspace(&mut wire, &format!("w{cycle}")).await;
        let first = split_in(&mut wire, root, &env.scratch()).await;
        let second = split_in(&mut wire, first, &env.scratch()).await;
        rename(&mut wire, first, &format!("first-{cycle}")).await;
        rename(&mut wire, second, &format!("second-{cycle}")).await;
        blocked_agent(&mut wire, space, &format!("agent-{cycle}")).await;
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
        "{} of {rounds} clean shutdowns did not finish draining — this is the \
         R-M1-2 wedge, caught:\n\n{}",
        wedged.len(),
        wedged.join("\n\n")
    );
}

// -------------------------------------------------------------------- the storm

/// The same drain, signalled with a client still attached.
///
/// The distinction from the tripwire above is the gateway. Every sighting of
/// the wedge came from a suite where a real client was attached to a real
/// terminal at the moment the server was told to stop, so the gateway's own
/// task had connections to cancel, drain and join before it could return —
/// each of them holding a grid stream that reads a pane's published frames.
/// The tripwire signals a server whose only wire connection has already been
/// dropped, which is a strictly easier drain, and a canary that only ever runs
/// the easy case is not watching the thing that broke.
///
/// A fresh session per cycle, unlike the tripwire: this loop is meant to be
/// turned up to thousands of repetitions by the spike harness, and the
/// tripwire's shared state directory makes each cycle restore everything the
/// last one saved — pane count, and so cycle cost, growing without bound.
/// Restore under repetition is the tripwire's business; raw stop count is
/// this one's.
#[tokio::test]
async fn a_shutdown_storm_with_a_client_attached_never_wedges() {
    let shell = marker_shell("strm", ":");
    let rounds = cycles(AMX_STORM_CYCLES, STORM_CYCLES);
    let mut wedged = Vec::new();

    for cycle in 0..rounds {
        let mut env = Env::new("strm");
        env.set_var("SHELL", &shell.path());

        let server = env.server();
        // The client seeds the session's first workspace and the shell behind
        // it, so this one process is both the load and the reason there is
        // anything to shut down.
        let mut term = env.attach_on_tty(&[], ROWS, COLS);
        term.wait_for(ALT_ENTER);

        // A second pane over a second connection: two connection tasks in the
        // gateway, two panes in `Core`, at the signal.
        let mut wire = connected(&env).await;
        let root = focused_pane(&env).await;
        let split = split_in(&mut wire, root, &env.scratch()).await;
        wait_until("the cycle's panes are both running", || {
            processes_with_arg(shell.marker()) >= 2
        });

        // Neither connection is dropped and the client is never detached: the
        // gateway meets the signal with everything still open, which is the
        // state the wedge was seen in.
        if let Err(stuck) = server.shutdown_within(STOP_BUDGET) {
            wedged.push(format!("cycle {cycle} (pane {split}): {stuck}"));
        }
        drop(wire);
        // Dropping the terminal kills whatever is left of the client. What a
        // client does when its server disappears is D-M3-7's subject, not this
        // test's, and asserting it here would make a storm fail for a reason
        // that has nothing to do with the drain.
        drop(term);
        wait_until("the stopped server's shells are reaped", || {
            processes_with_arg(shell.marker()) == 0
        });
    }

    assert!(
        wedged.is_empty(),
        "{} of {rounds} attached-client shutdowns did not finish draining — this \
         is the R-M1-2 wedge, caught:\n\n{}",
        wedged.len(),
        wedged.join("\n\n")
    );
}

// --------------------------------------------------------------- the wedge

/// The drain wedge, reproduced and closed (W01, `docs/notes/m3-shutdown-wedge.md`).
///
/// A connection's handshake — the hello, the attach seed, the ping, the welcome
/// — used to be the one stretch of its life that watched no cancellation token.
/// A peer that connected and then went quiet held that task open for as long as
/// it stayed connected, so the gateway's join never finished; and because the
/// task still held an `AgentHandle`, the hub's mailbox could never close either,
/// so its drain-to-closure never finished. Everything else — `Core`, the panes,
/// `Persist`, the config watcher — completed and closed its descriptors, which
/// is exactly what the six wedged servers found on this machine looked like:
/// one listening socket, one accepted connection, nothing else left.
///
/// Not a race, once you know where to stand: this failed on every run before
/// the fix and passes on every run after it. The three peers are the three ways
/// to be silent, because `read_hello` cannot tell them apart and neither should
/// the drain.
#[tokio::test]
async fn a_peer_that_never_finishes_saying_hello_does_not_wedge_the_drain() {
    let shell = marker_shell("hush", ":");
    let mut env = Env::new("hush");
    env.set_var("SHELL", &shell.path());
    let server = env.server();

    // An ordinary session first, so the drain has real work beside the stalled
    // connections and a wedge cannot be blamed on an empty server.
    let mut live = connected(&env).await;
    let (_, root) = workspace(&mut live, "w").await;
    let _ = split_in(&mut live, root, &env.scratch()).await;

    let _silent = Wire::connect(&env.socket()).await;
    let mut half_header = Wire::connect(&env.socket()).await;
    half_header.send_bytes(&[0]).await;
    let mut half_hello = Wire::connect(&env.socket()).await;
    half_hello.send_partial_frame(64, br#"{"proto""#).await;

    // Every one of them still connected when the signal lands, which is the
    // whole point: a peer that closes gives the reader an end of file and the
    // task ends on its own.
    if let Err(stuck) = server.shutdown_within(STOP_BUDGET) {
        panic!("{stuck}");
    }
    wait_until("the stopped server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });
}

/// Start a scripted agent in `space` and leave it blocked.
///
/// Blocked and not merely running: `Blocked` is the state that puts a pane on
/// the attention queue *and* arms the staleness deadline, so the hub the signal
/// arrives at has both a queue to abandon and a timer to drop. An agent sitting
/// idle would leave the wheel disarmed, which is the easy case.
///
/// Failures here are assertions rather than skips: a cycle whose agent never
/// blocked is a cycle that did not put the fifth actor under load, and a canary
/// that quietly stopped watching is worse than no canary.
async fn blocked_agent(wire: &mut Wire, space: WorkspaceId, name: &str) {
    let started = wire
        .request(
            "agent.start",
            json!({ "name": name, "kind": agent::KIND, "workspace": space }),
        )
        .await;
    let pane = result_of(&started)["pane"]
        .as_str()
        .unwrap_or_else(|| panic!("agent.start answered {started:?}"))
        .to_owned();
    wire.request("pane.run", json!({ "target": pane, "text": agent::ASK }))
        .await;

    // The block, waited for on the wire rather than on a clock: `wait` is the
    // verb a script would use, and its timeout is an answer this test refuses
    // to accept.
    let waited = wire
        .request(
            "wait",
            json!({ "until": "blocked", "target": pane, "timeout_ms": 30_000 }),
        )
        .await;
    assert_eq!(
        result_of(&waited)["satisfied"],
        true,
        "the cycle's agent never blocked, so the hub is not under the load this \
         canary is about: {waited:?}"
    );
}
