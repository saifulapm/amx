//! U10: the places M1's waves met, proven as one system.
//!
//! Waves 0–3 were kept parallel by exclusive file ownership, and the T19
//! lesson from M0 is what that costs: nobody owns the seams. This suite owns
//! them, one module each, and every test here drives the real `amx` binary
//! over the real socket — a seam asserted in-process is a seam asserted
//! against a `Ctx` some test built, not against the server a person runs.
//!
//! - [`shutdown`] — persist↔core: the final capture Core pushes on its way
//!   down, and the bounded-repetition tripwire over the drain wedge (R-M1-2).
//! - [`restore`] — restore↔pane-spawn: a restored session against the
//!   first-attach seed, across a real process boundary.
//! - [`config`] — config-reload↔running-actors: one edit reaching two actors,
//!   and a reload storm arriving during saves.
//!
//! The test in this file is the scripted pass over all of them at once: one
//! session that is populated, killed, restored lossily, reported on, renamed,
//! reconfigured and stopped cleanly, with every M1 surface asserted where it
//! is supposed to appear. The modules prove the seams one at a time; this
//! proves they compose.
//!
//! No wait here is a nap. Saves are waited for by the bytes they leave on
//! disk, restores by what the restored session says about itself, and reloads
//! by consequences the filesystem and the process table can attest to.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

// A test crate root's module directory is the package root, so each module of
// this suite needs its path spelled out to live beside it. The fixtures are
// the crash suite's — the same planted shells, the same reading of the
// snapshot from outside the server — because a second copy of them would be a
// second thing to keep true.
#[path = "seams/config.rs"]
mod config;
#[path = "persistence/fixtures.rs"]
#[allow(dead_code, reason = "each module drives a subset of the fixtures")]
mod fixtures;
#[path = "seams/restore.rs"]
mod restore;
#[path = "seams/shutdown.rs"]
mod shutdown;

use fixtures::{
    COLS, ROWS, connected, dir, marker_shell, painted, rename, session_state, sidecars,
    snapshot_mentions, split_in, switch, workspace,
};
use rig::env::processes_with_arg;
use rig::screen::render;
use rig::{ALT_ENTER, Env, rasterize, shows, wait_until, wait_until_or};

/// Lines the cycle's shell prints on its first run: more than a pane holds, so
/// the early ones scroll into history and a sidecar has something to carry.
const MARK_LINES: usize = 60;

/// The marker still on the live grid when the server dies: the last line the
/// shell printed can never have scrolled off, so it is never history and must
/// never come back. Its absence is what proves the markers that *do* come back
/// were replayed out of a sidecar rather than reprinted by a shell.
fn live_mark() -> String {
    format!("cyc-{MARK_LINES} ")
}

#[tokio::test]
async fn a_full_m1_cycle_survives_shutdown_reboot_rename_reload_and_reports_honestly() {
    // The shell prints its markers only when told to, and the run after the
    // reboot is not told: a marker on the restored screen can then only have
    // come off the disk, never from a shell that simply ran again.
    let shell = marker_shell(
        "cyc",
        &format!(
            "if [ -n \"$AMX_RIG_MARKS\" ]; then i=1; while [ $i -le {MARK_LINES} ]; \
             do echo \"cyc-$i \"; i=$((i+1)); done; fi"
        ),
    );
    let mut env = Env::new("cyc");
    env.set_var("SHELL", &shell.path());
    env.set_var("AMX_RIG_MARKS", "on");
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    // ---------------------------------------------- populate, with a client
    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the seeded shell's markers to scroll past", |seen| {
        shows(seen, &live_mark())
    });

    let mut wire = connected(&env).await;
    let doomed = dir(&env.scratch(), "doomed");
    let kept = dir(&env.scratch(), "kept");
    let (build, build_root) = workspace(&mut wire, "build").await;
    let editor = split_in(&mut wire, build_root, &kept).await;
    rename(&mut wire, editor, "editor").await;
    let runner = split_in(&mut wire, build_root, &doomed).await;
    rename(&mut wire, runner, "runner").await;
    switch(&mut wire, build).await;

    term.wait_until_or(
        "the debounced save to record the whole session",
        || snapshot_mentions(&env, "editor") && snapshot_mentions(&env, "runner"),
        || format!("the snapshot holds {:?}", fixtures::snapshot(&env)),
    );
    term.wait_until_or(
        "the opted-in sidecars to be dumped",
        || !sidecars(&env).is_empty(),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    drop(wire);

    // -------------------------------------------------- the power cut, and a
    // deliberately broken item to be found on the way back up
    env.set_var("AMX_RIG_MARKS", "");
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });
    std::fs::remove_dir_all(&doomed).expect("remove one saved directory");

    // ------------------------------------------------------------ the reboot
    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    // Nothing printed this run — `$AMX_RIG_MARKS` is empty — so a marker on
    // screen at all can only have been replayed off a sidecar.
    term.wait_output("the replayed scrollback to render", |seen| {
        shows(seen, "cyc-")
    });
    assert!(
        !shows(term.output(), &live_mark()),
        "only history is persisted; the live grid at kill time is not:\n{}",
        render(&rasterize(term.output()))
    );

    let mut wire = connected(&env).await;
    let state = session_state(&mut wire).await;
    assert_eq!(
        state["workspaces"].as_array().expect("workspaces").len(),
        2,
        "the attach seeded nothing over the restored workspaces: {state}"
    );
    assert_eq!(
        state["restore"]["degraded"], 1,
        "the vanished directory degrades exactly one pane: {state}"
    );
    assert_eq!(
        state["restore"]["lost"], 0,
        "a missing directory degrades a pane, it does not lose it: {state}"
    );

    // The loss, in all three places 04 §6 asks for it. The status line first,
    // because it is the one a person sees without asking.
    let settled = term.wait_settled();
    assert!(
        render(&settled).contains("⚠1"),
        "the status line carries the restore's one loss; the screen held:\n{}",
        render(&settled)
    );
    let report = env.run(&["session", "report"]).ok().to_owned();
    assert!(
        report.starts_with("0 lost, 1 degraded"),
        "the report leads with its counts:\n{report}"
    );
    assert!(
        report.contains(&doomed.to_string_lossy().into_owned()),
        "the report names the directory the pane lost:\n{report}"
    );
    assert!(
        state["panes"]
            .as_array()
            .expect("panes")
            .iter()
            .any(|pane| pane["label"] == "runner"),
        "the degraded pane is kept, with its label: {state}"
    );

    // ------------------------------------- mutate and reconfigure the restored
    // session, then stop it the civilised way
    rename(&mut wire, editor, "editor-after-reboot").await;
    std::fs::write(env.config_path(), "[persist]\nhistory = false\n").expect("turn scrollback off");
    term.wait_until_or(
        "the sidecars to be wiped the moment history is turned off",
        || sidecars(&env).is_empty(),
        || format!("the history directory still holds {:?}", sidecars(&env)),
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    drop(wire);
    // The rename happened inside the debounce window that the wipe interrupted,
    // so what puts it on disk is the final capture `Core` pushes to `Persist`
    // on its way down — the seam this stop exercises.
    server.shutdown();
    assert!(
        snapshot_mentions(&env, "editor-after-reboot"),
        "the clean stop lost a rename made after the restore; the snapshot holds {:?}",
        fixtures::snapshot(&env)
    );

    // ---------------------------------------------- and the second reboot is
    // clean, because the restore repaired what it degraded
    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    let mut wire = connected(&env).await;
    let state = session_state(&mut wire).await;
    assert_eq!(
        state["restore"]["degraded"], 0,
        "the pane respawned in $HOME last time has a directory that exists: {state}"
    );
    assert!(
        state["panes"]
            .as_array()
            .expect("panes")
            .iter()
            .any(|pane| pane["label"] == "editor-after-reboot"),
        "the label the final capture saved came back: {state}"
    );
    assert_eq!(
        env.run(&["session", "report"]).ok(),
        "no losses to report\n",
        "a restore that lost nothing says so"
    );
    let settled = term.wait_settled();
    assert!(
        !render(&settled).contains('⚠'),
        "a clean restore leaves the status line clean; the screen held:\n{}",
        render(&settled)
    );
    assert!(
        sidecars(&env).is_empty(),
        "history is still off, so nothing was dumped: {:?}",
        sidecars(&env)
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();

    // A last wait with a diagnostic, so a session that leaves shells behind
    // fails here rather than in whatever runs next.
    wait_until_or(
        "the stopped session's shells to be reaped",
        || processes_with_arg(shell.marker()) == 0,
        || {
            format!(
                "{} shells are still running",
                processes_with_arg(shell.marker())
            )
        },
    );
}
