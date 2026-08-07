//! The config-reload↔running-actors seam, over the real binary.
//!
//! U08 proved D-M1-8's semantics against a `Ctx` built in-process. What that
//! cannot say is whether an edit to a file on disk reaches *two different
//! actors* of a running server, or what happens when it arrives while the
//! persistence debounce is already counting down. Both are seams — places two
//! tasks meet that no wave owned — so both are proven here, through a file a
//! person could have edited and a socket a client could have used.
//!
//! Nothing here waits for a reload as such. `ConfigReloaded` rides the bus and
//! the bus has no CLI until M2, so every wait is on a consequence the
//! filesystem or the process table can attest to: sidecars appearing and
//! vanishing, and a pane spawned with one shell rather than another.

use rig::env::processes_with_arg;
use rig::{ALT_ENTER, Env, rasterize, shows};

use crate::fixtures::{
    COLS, ROWS, assert_snapshot_is_whole, connected, dir, marker_shell, painted, rename, sidecars,
    snapshot_mentions, split_in, workspace,
};

/// Lines each shell prints when it starts: more than a pane holds, so the
/// early ones are scrollback and a sidecar has something to hold.
const MARK_LINES: usize = 60;

/// A shell body that prints `tag`-numbered lines and then holds.
fn printing(tag: &str) -> String {
    format!("i=1; while [ $i -le {MARK_LINES} ]; do echo \"{tag}-$i \"; i=$((i+1)); done")
}

// ---------------------------------------- one edit, two actors, both directions

#[tokio::test]
async fn a_reload_reaches_the_persist_actor_and_the_next_pane_spawn() {
    // Two shells, so "which one did this pane spawn?" is a question the
    // process table answers rather than the server.
    let first = marker_shell("cfg1", &printing("one"));
    let second = marker_shell("cfg2", &printing("two"));
    let mut env = Env::new("cfg");
    env.set_var("SHELL", &first.path());
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    // A client from the start: a pane's grid is the size the attached client
    // gives it, and rows only become *history* by scrolling off a grid that
    // has a size.
    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the seeded shell's markers to scroll past", |seen| {
        shows(seen, &format!("one-{MARK_LINES} "))
    });
    term.wait_until_or(
        "the opted-in sidecar to be dumped",
        || !sidecars(&env).is_empty(),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );
    assert_eq!(
        processes_with_arg(first.marker()),
        1,
        "the session runs one pane, on the shell the environment named"
    );

    // The edit: history off, and a different shell for whatever spawns next.
    // One file write, so both actors are reacting to the same reload.
    std::fs::write(
        env.config_path(),
        format!(
            "[persist]\nhistory = false\n\n[terminal]\nshell = {:?}\n",
            second.path()
        ),
    )
    .expect("rewrite the config");

    // Place one: scrollback holds secrets, so turning it off does not wait for
    // a save — the directory goes now.
    term.wait_until_or(
        "the sidecars to be wiped the moment history is turned off",
        || sidecars(&env).is_empty(),
        || format!("the history directory still holds {:?}", sidecars(&env)),
    );

    // Place two: the *next* pane runs the newly configured shell, and the pane
    // already running keeps the process it started with — a config edit that
    // restarted a live shell would throw away whatever was in it.
    let mut wire = connected(&env).await;
    let (_, root) = workspace(&mut wire, "after").await;
    term.wait_until_or(
        "a pane spawned after the edit to run the newly configured shell",
        || processes_with_arg(second.marker()) >= 1,
        || {
            format!(
                "{} panes run the new shell, {} the old",
                processes_with_arg(second.marker()),
                processes_with_arg(first.marker())
            )
        },
    );
    assert_eq!(
        processes_with_arg(first.marker()),
        1,
        "the pane that was already running kept its own process"
    );

    // And back the other way. A reload is asynchronous — a watch event, a
    // quiet window, a re-read — so a wire call issued straight after the write
    // can outrun it, and the order below is what makes the assertion about the
    // reload rather than about who won. The split is structural dirt, which is
    // what a dump rides on; the dump appearing is the reload's *receipt*, and
    // only then is a pane spawned to ask whether the same reload reached the
    // other actor. (A wipe forgets which panes were dumped, never their
    // history windows, so every pane with scrollback is due again.)
    std::fs::write(
        env.config_path(),
        format!(
            "[persist]\nhistory = true\n\n[terminal]\nshell = {:?}\n",
            first.path()
        ),
    )
    .expect("restore the config");
    let nudge = split_in(&mut wire, root, &dir(&env.scratch(), "nudge")).await;
    rename(&mut wire, nudge, "nudge").await;
    term.wait_until_or(
        "the re-enabled sidecars to be dumped at the next save",
        || !sidecars(&env).is_empty(),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );

    let before_spawn = processes_with_arg(first.marker());
    split_in(&mut wire, root, &dir(&env.scratch(), "back")).await;
    term.wait_until_or(
        "a pane spawned after the reload landed to run the original shell again",
        || processes_with_arg(first.marker()) > before_spawn,
        || {
            format!(
                "{} panes run the original shell, {} the other",
                processes_with_arg(first.marker()),
                processes_with_arg(second.marker())
            )
        },
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}

// --------------------------------------------- a reload arriving during a save

#[tokio::test]
async fn reload_racing_a_save_corrupts_nothing() {
    let shell = marker_shell("race", ":");
    let mut env = Env::new("race");
    env.set_var("SHELL", &shell.path());

    let server = env.server();
    let mut wire = connected(&env).await;
    let (_, root) = workspace(&mut wire, "race").await;
    let pane = split_in(&mut wire, root, &dir(&env.scratch(), "race")).await;
    rename(&mut wire, pane, "round-0").await;
    rig::wait_until("the first save lands", || {
        snapshot_mentions(&env, "round-0")
    });

    // Structural churn and config rewrites interleaved, so a reload lands
    // wherever it lands relative to the debounce: before a capture, between a
    // capture and its write, after a rename that has not been saved yet. The
    // two settings are chosen to be the disruptive ones — the history toggle
    // deletes a directory the writer may be about to write into, and the shell
    // changes what the pane spawn in the same breath will run.
    for round in 1..=8 {
        let history = round % 2 == 0;
        std::fs::write(
            env.config_path(),
            format!(
                "[persist]\nhistory = {history}\n\n[terminal]\nshell = {:?}\n",
                shell.path()
            ),
        )
        .expect("rewrite the config");
        rename(&mut wire, pane, &format!("round-{round}")).await;
        // The invariant, asserted at every round rather than only at the end:
        // whatever a reader picks up mid-storm is a whole session — the old
        // one or the new one, never a blend. `assert_snapshot_is_whole`
        // panics on a file that parses into layouts pointing at panes the
        // pane table does not hold, which is what a torn write looks like
        // once JSON happens to survive it.
        assert_snapshot_is_whole(&env, &format!("during reload round {round}"));
    }

    // The session is still the session: the last rename arrives, and the
    // storm left the file readable enough to prove it.
    rig::wait_until("the last round's rename reaches the snapshot", || {
        snapshot_mentions(&env, "round-8")
    });
    let snapshot = assert_snapshot_is_whole(&env, "after the reload storm");
    assert_eq!(
        snapshot["panes"].as_array().expect("a pane table").len(),
        2,
        "the storm neither lost nor invented a pane: {snapshot}"
    );

    // And a client attaching to it sees a working session, not a wreck.
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}
