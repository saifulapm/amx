//! Scrollback sidecars: what survives a reboot when the user opts in, what is
//! never written when they do not, and what is removed when they change their
//! mind.
//!
//! Every proof turns on a shell that prints its markers exactly once and stays
//! silent afterwards. After the reboot nothing prints, so a marker on the
//! restored screen can only have been replayed off the disk — and the marker
//! still on the live grid when the server died must *not* be there, because M1
//! persists history and a live grid is not history (D-M1-6).
//!
//! The two sessions here differ in *when* the scrollback arrives, and that
//! turned out to be the whole difference between a sidecar and no sidecar: one
//! produces it while the pane is still being created, the other long after the
//! session has settled, which is the shape every real session spends its life
//! in.

use rig::env::processes_with_arg;
use rig::screen::render;
use rig::{ALT_ENTER, Env, rasterize, shows, wait_until};

use crate::fixtures::{
    COLS, ROWS, crash_durable, focused_pane, history_dir, history_files, marker_shell, painted,
    restore_evidence, sidecar_holds, sidecars, snapshot_mentions,
};

// ---------------------------------------------------------------- sidecars

/// Lines the marker shell prints when it is asked to: more than a pane holds,
/// so the early ones are scrollback and the late ones are still on the grid.
const MARK_LINES: usize = 30;

/// The scrollback marker asserted on: early enough to have scrolled off the
/// live grid before the kill, so only a replayed sidecar can put it back. The
/// trailing space is the marker shell's own, and it keeps `snap-2` from
/// matching `snap-20`.
const EARLY_MARK: &str = "snap-2 ";

/// A marker still on the live grid at kill time — never persisted, because M1
/// sidecars hold history and the grid is not history. Its absence after the
/// reboot is what proves the early marker came out of the sidecar rather than
/// out of a shell that simply ran again.
const LIVE_MARK: &str = "snap-30 ";

#[tokio::test]
async fn sidecars_restore_scrollback_only_when_opted_in() {
    // The shell prints its markers only when `$AMX_RIG_MARKS` says to, and the
    // second run says nothing: after the reboot the script is silent, so a
    // marker on the restored screen can only have come off the disk.
    let body = format!(
        "if [ -n \"$AMX_RIG_MARKS\" ]; then i=1; while [ $i -le {MARK_LINES} ]; \
         do echo \"snap-$i \"; i=$((i+1)); done; fi"
    );

    // Opted in: `[persist] history = true`.
    let shell = marker_shell("hist", &body);
    let mut env = Env::new("hist");
    env.set_var("SHELL", &shell.path());
    env.set_var("AMX_RIG_MARKS", "on");
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    // A client from the start, so the scrollback is produced at the size a
    // user's pane really has: the first attach seeds the workspace whose root
    // pane runs the marker shell, and the markers scroll past on screen.
    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the markers to scroll past", |seen| shows(seen, LIVE_MARK));
    let root = focused_pane(&env).await;
    term.wait_until_or(
        "a sidecar holding the scrolled-off markers is dumped",
        || sidecar_holds(&env, EARLY_MARK),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );
    let dumped = sidecars(&env);
    assert_eq!(dumped.len(), 1, "one pane, one sidecar: {dumped:?}");
    assert!(
        dumped[0].to_string_lossy().contains(&root.to_string()),
        "the sidecar is named for its pane: {dumped:?}"
    );
    assert!(
        !shows(term.output(), EARLY_MARK),
        "the early marker has scrolled off the live grid, which is what makes \
         it scrollback; the screen holds:\n{}",
        render(&rasterize(term.output()))
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));

    env.set_var("AMX_RIG_MARKS", "");
    crash_durable(&env, root, EARLY_MARK);
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });

    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output_or(
        "the replayed scrollback to render",
        |seen| shows(seen, EARLY_MARK),
        || restore_evidence(&env, root),
    );
    assert!(
        !shows(term.output(), LIVE_MARK),
        "only history is persisted; the live grid at kill time is not:\n{}",
        render(&rasterize(term.output()))
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();

    // Opted out: no config file at all, which is the default configuration.
    let shell = marker_shell("nohi", &body);
    let mut env = Env::new("nohi");
    env.set_var("SHELL", &shell.path());
    env.set_var("AMX_RIG_MARKS", "on");

    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the markers to scroll past", |seen| shows(seen, LIVE_MARK));
    let root = focused_pane(&env).await;
    term.wait_until("the save lands", || {
        snapshot_mentions(&env, &root.to_string())
    });
    // The whole directory, not just the committed sidecars: a staging file
    // holds the same payload under a different name, and scrollback nobody
    // opted into is a leak whichever name it is written under.
    assert!(
        history_files(&env).is_empty(),
        "scrollback holds secrets: nothing is written without opting in, but found {:?}",
        history_files(&env)
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));

    env.set_var("AMX_RIG_MARKS", "");
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });

    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    let settled = term.wait_settled();
    assert!(
        !render(&settled).contains("snap-"),
        "nothing was saved, so nothing can be replayed; the screen held:\n{}",
        render(&settled)
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}

// ------------------------------------------------- scrollback that arrives late

/// The session above produces its scrollback while the pane is still being
/// created, so the save the *pane* earns is what carries the dump along with
/// it. A real session is the other order — a user opens a terminal, watches it
/// settle, and only then runs something — and the scrollback that arrives then
/// has to reach disk on its own account, with no structural change to ride.
#[tokio::test]
async fn scrollback_produced_after_the_session_settles_is_still_dumped() {
    // One `read` per burst of markers, so the pane is silent until this test
    // types at it — long after the attach's structural events have been saved
    // and the debounce disarmed.
    let body = format!(
        "while read _line; do i=1; while [ $i -le {MARK_LINES} ]; \
         do echo \"snap-$i \"; i=$((i+1)); done; done"
    );
    let shell = marker_shell("late", &body);
    let mut env = Env::new("late");
    env.set_var("SHELL", &shell.path());
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    let root = focused_pane(&env).await;
    // Waited for by its consequence, like every save in this suite: once the
    // snapshot names the pane, the save the attach earned has been written and
    // the debounce is disarmed. Nothing but the output typed below can arm it
    // again — which is exactly the claim under test.
    term.wait_until("the attach's save lands", || {
        snapshot_mentions(&env, &root.to_string())
    });
    term.wait_settled();
    assert!(
        sidecars(&env).is_empty(),
        "nothing has scrolled yet, so there is nothing to dump: {:?}",
        sidecars(&env)
    );

    term.type_line("");
    term.wait_output("the markers to scroll past", |seen| shows(seen, LIVE_MARK));
    term.wait_until_or(
        "the scrolled-off markers reach a sidecar on their own account",
        || sidecar_holds(&env, EARLY_MARK),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );
    assert!(
        !shows(term.output(), EARLY_MARK),
        "the early marker has scrolled off the live grid, which is what makes \
         it scrollback; the screen holds:\n{}",
        render(&rasterize(term.output()))
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));

    // The power cut, and the proof that the dump is worth something: the
    // restored pane's shell is waiting on `read` and prints nothing, so a
    // marker on the screen can only have been replayed off the disk.
    crash_durable(&env, root, EARLY_MARK);
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });

    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output_or(
        "the replayed scrollback to render",
        |seen| shows(seen, EARLY_MARK),
        || restore_evidence(&env, root),
    );
    assert!(
        !shows(term.output(), LIVE_MARK),
        "only history is persisted; the live grid at kill time is not:\n{}",
        render(&rasterize(term.output()))
    );
    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}

// --------------------------------------------------------------- the wipe

#[tokio::test]
async fn turning_history_off_wipes_the_sidecars() {
    let body =
        format!("i=1; while [ $i -le {MARK_LINES} ]; do echo \"snap-$i \"; i=$((i+1)); done");
    let shell = marker_shell("wipe", &body);
    let mut env = Env::new("wipe");
    env.set_var("SHELL", &shell.path());
    std::fs::write(env.config_path(), "[persist]\nhistory = true\n").expect("write the config");

    let server = env.server();
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("the markers to scroll past", |seen| shows(seen, LIVE_MARK));
    term.wait_until_or(
        "a sidecar holding the scrolled-off markers is dumped",
        || sidecar_holds(&env, EARLY_MARK),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );

    // The toggle, hot, through the file the running server is watching. D-M1-6
    // says off wipes *immediately* — not at the next save — because what is on
    // disk is the user's scrollback.
    std::fs::write(env.config_path(), "[persist]\nhistory = false\n").expect("rewrite the config");
    term.wait_until_or(
        "the sidecars are gone, directory and all",
        || !history_dir(&env).exists(),
        || format!("the history directory holds {:?}", sidecars(&env)),
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}
