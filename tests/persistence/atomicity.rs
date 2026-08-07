//! The write discipline, from outside the process: a kill storm aimed at the
//! save window, and the debounce watched through the only evidence it leaves
//! on a filesystem — when `session.json` changes, and when it does not.
//!
//! Both tests are about the same promise from opposite sides. Atomicity says a
//! reader never sees a half-written session no matter when the writer dies;
//! the debounce says the window a `kill -9` can cost is bounded without
//! writing on every keystroke. Neither can be shown by asking the server: the
//! server is the accused.

use std::time::{Duration, Instant};

use rig::env::processes_with_arg;
use rig::wire::result_of;
use rig::{Env, PATIENCE, TICK, Wire, wait_until};
use serde_json::json;

use crate::fixtures::{
    assert_snapshot_is_whole, connected, dir, inode_of, marker_shell, rename, session_state,
    snapshot_bytes, snapshot_mentions, split_in, staging_files, workspace,
};

// -------------------------------------------------------- kill -9 mid-write

/// How many kill/restart rounds the storm runs.
///
/// Bounded, like every repetition in this rig: the claim is an invariant that
/// must hold at every kill, and rounds buy different moments to kill at, not
/// statistical confidence.
const ROUNDS: usize = 6;

/// How long one round watches the state directory before killing anyway.
///
/// Not load-bearing: the watch only steers *where* the kill lands relative to
/// a write, and the invariant the round asserts has to hold wherever it lands.
/// Its expiry weakens the round, it cannot redden it.
const WATCH_FOR_A_WRITE: Duration = Duration::from_secs(3);

/// Where a kill landed relative to the save it was aimed at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Landed {
    /// Inside the write: a staging file was on disk, so the payload had been
    /// written and not yet renamed over the live snapshot.
    MidWrite,
    /// Microseconds after the rename committed, caught by the same spin.
    JustAfter,
    /// The disk showed neither within the watch; the kill fell somewhere else
    /// in the cycle, which is a moment worth killing at too.
    Elsewhere,
}

#[tokio::test]
async fn kill_dash_9_mid_write_always_leaves_a_restorable_snapshot() {
    let shell = marker_shell("torn", ":");
    let mut env = Env::new("torn");
    env.set_var("SHELL", &shell.path());
    let work = dir(&env.scratch(), "work");

    // A session worth tearing, and one clean save to establish the "old" state
    // every later kill must be able to fall back to.
    let server = env.server();
    let mut wire = connected(&env).await;
    let (_, root) = workspace(&mut wire, "storm").await;
    let pane = split_in(&mut wire, root, &work).await;
    rename(&mut wire, pane, "settled").await;
    wait_until("the first save lands", || {
        snapshot_mentions(&env, "settled")
    });
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });

    // A staging file left behind by a writer that died before its rename —
    // exactly what a kill during step 2 of `write_atomic` leaves. It must be
    // invisible to the reader and swept by the next writer (U02's contract,
    // asserted here at rig level, over the real files).
    let stray = env.state_dir().join("session.json.999999.0.tmp");
    std::fs::write(&stray, b"{ this is not a snapshot").expect("plant a staging file");
    let mut server = env.server();
    let mut wire = connected(&env).await;
    let state = session_state(&mut wire).await;
    assert_eq!(
        state["restore"]["lost"], 0,
        "a stray staging file must be ignored by load, not mistaken for state: {state}"
    );
    // The same save answers the other half of D-M1-1 that a black-box test can
    // reach. The *ordering* inside `write_atomic` — staging fsync, rename,
    // directory fsync — is pinned in-process by U02's recording seam, because
    // no test outside the process can watch an fsync happen. What is visible
    // from out here is the shape the ordering exists to protect: the snapshot
    // is a new inode each time, so it was renamed into place and never
    // rewritten under a reader. A writer that opened the live file and wrote
    // through it would keep the inode and pass every other test in this suite.
    let inode = inode_of(&env);
    rename(&mut wire, pane, "swept").await;
    wait_until("the next save lands", || snapshot_mentions(&env, "swept"));
    assert_ne!(
        inode_of(&env),
        inode,
        "the snapshot kept its inode across a save; a durable write replaces \
         the file by rename, it never writes through the live one (D-M1-1)"
    );
    assert!(
        staging_files(&env).is_empty(),
        "the next save left staging files behind: {:?}",
        staging_files(&env)
    );

    // The storm. Every round grows the session — so the snapshot being written
    // grows, and with it the window a kill can land inside — then kills the
    // server as close to a save as a black-box test can aim, and demands that
    // what is left on disk is a whole session: the old one or the new one,
    // never a torn one.
    let mut landings = Vec::new();
    for round in 0..ROUNDS {
        let (_, root) = workspace(&mut wire, &format!("round-{round}")).await;
        let pane = split_in(&mut wire, root, &work).await;
        // A tight write loop: renames back to back, so the snapshot the
        // debounce is about to commit differs from the one on disk in every
        // round and grows round on round.
        for label in 0..=round {
            rename(&mut wire, pane, &format!("r{round}-{label}")).await;
        }

        // Aim the kill. The spin is deliberate and not a wait: it samples the
        // state directory tens of thousands of times a second, which is the
        // only sampling rate that can see a staging file whose whole life is
        // a write, an fsync and a rename. Whichever it sees first — the
        // staging file (the write is in flight) or the snapshot's bytes
        // changing (the rename has just committed) — the kill follows
        // immediately.
        let before = snapshot_bytes(&env);
        let until = Instant::now() + WATCH_FOR_A_WRITE;
        let mut landed = Landed::Elsewhere;
        while Instant::now() < until {
            if !staging_files(&env).is_empty() {
                landed = Landed::MidWrite;
                break;
            }
            if snapshot_bytes(&env) != before {
                landed = Landed::JustAfter;
                break;
            }
        }
        if landed == Landed::Elsewhere {
            // No save was observed inside the watch window — a loaded runner
            // can schedule the debounce timer later than any aim. The
            // post-restart assertions demand this round's growth, so they are
            // only meaningful once it has reached disk: wait for the round's
            // workspace label to appear in the snapshot (content, not bytes —
            // a save may already have slipped in before `before` was taken),
            // then kill. The aimed rounds keep their mid-write and
            // just-after landings; this one degrades to a kill between
            // saves, which the whole-or-old property must survive too.
            let label = format!("round-{round}");
            wait_until("the round's workspace reaches the snapshot", || {
                snapshot_bytes(&env)
                    .is_some_and(|bytes| String::from_utf8_lossy(&bytes).contains(&label))
            });
        }
        landings.push(landed);
        server.kill_dash_9();
        assert_snapshot_is_whole(&env, &format!("after the kill in round {round}"));
        wait_until("the killed server's shells are reaped", || {
            processes_with_arg(shell.marker()) == 0
        });

        server = env.server();
        wire = connected(&env).await;
        let state = session_state(&mut wire).await;
        assert_eq!(
            state["restore"]["lost"], 0,
            "round {round} restarted onto a snapshot it could not use: {state}"
        );
        assert!(
            state["workspaces"].as_array().unwrap().len() >= 2,
            "round {round} restored an emptied session: {state}"
        );
    }
    // Where the kills landed is reported, not asserted: how often a spin can
    // see a staging file is a property of the filesystem under the state
    // directory — on a tmpfs the fsync is free and the file's whole life is a
    // few microseconds — and a test that demanded a number here would be
    // asserting about the disk rather than about amx. What *is* asserted is
    // the invariant, at every landing: the snapshot is whole.
    eprintln!("kill storm landings: {landings:?}");
    assert!(
        landings.iter().any(|&where_| where_ != Landed::Elsewhere),
        "no kill in the storm landed near a write at all; the storm proved nothing"
    );

    // The litter a killed writer leaves behind is the next writer's to clear,
    // so the storm ends by making one more save happen and finding the state
    // directory clean afterwards.
    workspace(&mut wire, "after-the-storm").await;
    wait_until("the save after the storm lands", || {
        snapshot_mentions(&env, "after-the-storm")
    });
    assert!(
        staging_files(&env).is_empty(),
        "the storm's staging files outlived the next save: {:?}",
        staging_files(&env)
    );
    server.shutdown();
}

// ------------------------------------------------ the debounce, from outside

/// Rows of pane output the damage test waits for before it judges.
const DAMAGE_ROWS: u64 = 1_500;

/// How often the continuous-change loop mutates the session.
///
/// Well inside the 500 ms quiet window, so the quiet deadline is pushed out by
/// every call and can never fire: a save observed while this loop runs can
/// only have come from the staleness cap.
const KEEP_DIRTY: Duration = Duration::from_millis(50);

#[tokio::test]
async fn saves_are_debounced_by_structure_not_by_damage_and_capped_under_change() {
    let shell = marker_shell("deb", ":");
    let mut env = Env::new("deb");
    env.set_var("SHELL", &shell.path());

    let server = env.server();
    let mut wire = connected(&env).await;
    let (_, root) = workspace(&mut wire, "debounce").await;
    // A pane that prints a great deal and then holds: thousands of damage
    // events, and — after the split itself settles — not one structural one.
    let printer = split_in(&mut wire, root, &env.scratch()).await;
    rename(&mut wire, printer, "printer").await;
    let noisy = result_of(
        &wire
            .request(
                "pane.split",
                json!({
                    "pane": printer,
                    "direction": "horizontal",
                    "cwd": env.scratch(),
                    "command": ["sh", "-c",
                        format!("i=0; while [ $i -lt {DAMAGE_ROWS} ]; do echo damage-$i; \
                                 i=$((i+1)); done; read _held")],
                }),
            )
            .await,
    )["pane"]
        .as_str()
        .expect("the noisy pane's id")
        .to_owned();

    // The quiet window, observed from outside: nobody asked for a save, and
    // one landed anyway once the burst of structural changes stopped. Waited
    // for by the *last* change of the burst, so nothing structural is still
    // owed to the disk when the baseline below is taken.
    wait_until("the quiet window fires a save nobody asked for", || {
        snapshot_mentions(&env, &noisy)
    });
    wait_for_history(&mut wire, 100).await;
    let quiet = snapshot_bytes(&env).expect("a snapshot on disk");

    // Thousands of rows of pane output later, the snapshot has not moved:
    // damage is not structure, and folding it in would put the quiet window
    // permanently out of reach of any pane producing output (D-M1-7).
    wait_for_history(&mut wire, DAMAGE_ROWS - 100).await;
    assert_eq!(
        snapshot_bytes(&env).as_ref(),
        Some(&quiet),
        "pane output rewrote the snapshot; damage must never schedule a save"
    );

    // And the ceiling: a session that never goes quiet still saves. Every
    // iteration is a structural change well inside the quiet window, so the
    // save this waits for can only be the staleness cap firing.
    let mut ticker = tokio::time::interval(KEEP_DIRTY);
    let mut capped = false;
    for n in 0..200_u32 {
        ticker.tick().await;
        rename(&mut wire, printer, &format!("busy-{n}")).await;
        if snapshot_bytes(&env).as_ref() != Some(&quiet) {
            capped = true;
            break;
        }
    }
    assert!(
        capped,
        "a continuously changing session never saved; the staleness cap is the \
         only thing that bounds what a kill -9 costs (D-M1-7)"
    );

    server.shutdown();
}

/// Wait until some pane's history has committed `rows` rows.
///
/// The condition-and-deadline shape of [`rig::wait_until`], written out here
/// because the question is asked over the wire and the answer has to be
/// awaited. The tick is the rig's own, so this polls no faster than any other
/// wait in the package.
async fn wait_for_history(wire: &mut Wire, rows: u64) {
    let mut ticker = tokio::time::interval(TICK);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let head = head_of(&session_state(wire).await);
        if head >= rows {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {rows} rows of pane output; the history head is at {head}"
        );
        ticker.tick().await;
    }
}

/// The history head of the busiest pane in a `session.state` reply.
fn head_of(state: &serde_json::Value) -> u64 {
    state["panes"]
        .as_array()
        .map(|panes| {
            panes
                .iter()
                .filter_map(|pane| pane["history_head"].as_u64())
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}
