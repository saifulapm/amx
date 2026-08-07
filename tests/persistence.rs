//! M1's exit criterion, in the rig: reboot the machine, run `amx`, everything
//! is back where it was — and anything that isn't is listed in `amx session
//! report` (`docs/05-roadmap.md` M1).
//!
//! Everything here runs against the real binary over the real socket, and the
//! "reboot" is a `SIGKILL` of the server: no shutdown path runs, no drain
//! flushes, no final capture is pushed. Whatever is on disk afterwards is
//! whatever the debounced save last committed, which is precisely the claim
//! durability makes. The rig has never killed a *server* before — `tests/
//! adversarial.rs` kills clients exclusively — so every kill here goes through
//! [`rig::ServerChild::kill_dash_9`], which reaps the child it killed, and
//! every test then waits for the session's orphaned shells to disappear before
//! it starts the next server, because a marker left in the process table is
//! the next assertion's false positive.
//!
//! The exit criterion is two sentences, and this file holds one test for each:
//! everything is back ([`reboot_restores_workspaces_panes_labels_cwds_and_shorts`])
//! and what isn't is listed
//! ([`restore_loss_is_reported_in_status_line_and_session_report`]). The write
//! discipline underneath them — the kill storm and the debounce — lives in
//! [`atomicity`], and the opt-in scrollback sidecars in [`scrollback`].
//!
//! No wait in this suite is a nap. Saves are waited for by their consequence —
//! the snapshot on disk naming the change — and the one bounded observation
//! window (the kill storm's watch for a staging file) is a poll whose expiry
//! only moves the kill to a different moment, never reddens the test.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

// A test crate root's module directory is the package root, so every module of
// this suite needs its path spelled out to live beside it rather than
// alongside every other suite in `tests/`.
#[path = "persistence/atomicity.rs"]
mod atomicity;
#[path = "persistence/fixtures.rs"]
mod fixtures;
#[path = "persistence/scrollback.rs"]
mod scrollback;

use fixtures::{
    COLS, ROWS, connected, dir, durable_view, marker_shell, painted, physical, rename,
    session_state, snapshot, snapshot_mentions, split_in, switch, workspace,
};
use rig::env::processes_with_arg;
use rig::screen::render;
use rig::{ALT_ENTER, Env, rasterize, wait_until, wait_until_or};
use serde_json::json;

// ------------------------------------------------------------- the reboot

#[tokio::test]
async fn reboot_restores_workspaces_panes_labels_cwds_and_shorts() {
    // The shell every pane in this session runs, restored panes included: it
    // records the directory it was started in and then holds. A restored pane
    // runs `$SHELL`, never the argv it had (D-M1-5 stores no argv), so this
    // script is what comes back — and what it writes is the respawned
    // process's own account of its working directory, which no server-side
    // report can fake.
    let shell = marker_shell("boot", "pwd -P >> \"$AMX_RIG_CWDS\"");
    let mut env = Env::new("boot");
    env.set_var("SHELL", &shell.path());
    let before_log = env.scratch().join("cwds-before");
    env.set_var("AMX_RIG_CWDS", &before_log.to_string_lossy());

    let server = env.server();
    let mut wire = connected(&env).await;

    // Two labelled workspaces, a split in each with a directory of its own,
    // and a renamed pane — every field the snapshot claims to carry.
    let editing = dir(&env.scratch(), "editing");
    let building = dir(&env.scratch(), "building");
    let (build, build_root) = workspace(&mut wire, "build").await;
    let editor = split_in(&mut wire, build_root, &editing).await;
    rename(&mut wire, editor, "editor").await;
    let (_agents, agents_root) = workspace(&mut wire, "agents").await;
    let runner = split_in(&mut wire, agents_root, &building).await;
    rename(&mut wire, runner, "runner").await;
    // Focus lands somewhere deterministic, since it is part of what comes back.
    switch(&mut wire, build).await;

    // The save is waited for by its consequence, not by a flush verb: the
    // debounced write is the only thing that can put these strings on disk.
    wait_until_or(
        "the debounced save records the whole session",
        || {
            snapshot_mentions(&env, "editor")
                && snapshot_mentions(&env, "runner")
                && snapshot_mentions(&env, &editing.to_string_lossy())
                && snapshot_mentions(&env, &building.to_string_lossy())
                && snapshot(&env).is_some_and(|snap| snap["panes"].as_array().unwrap().len() == 4)
        },
        || format!("the snapshot holds {:?}", snapshot(&env)),
    );
    let before = durable_view(&session_state(&mut wire).await);
    wait_until("all four shells start", || {
        processes_with_arg(shell.marker()) == 4
    });

    // The power cut. Nothing is asked of the server first.
    let after_log = env.scratch().join("cwds-after");
    env.set_var("AMX_RIG_CWDS", &after_log.to_string_lossy());
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });

    // Run `amx` again, on the same state directory.
    let server = env.server();
    let mut wire = connected(&env).await;
    let state = session_state(&mut wire).await;
    assert_eq!(
        durable_view(&state),
        before,
        "the reboot did not put the session back as it was"
    );
    assert_eq!(
        state["restore"],
        json!({ "restored": 6, "lost": 0, "degraded": 0 }),
        "two workspaces and four panes came back, with nothing lost: {state}"
    );
    assert_eq!(
        env.run(&["session", "report"]).ok(),
        "no losses to report\n",
        "a clean reboot reports no losses"
    );

    // And the shells really are in the directories the snapshot named — asked
    // of the processes, not of the server.
    wait_until_or(
        "the respawned shells record their directories",
        || std::fs::read_to_string(&after_log).is_ok_and(|logged| logged.lines().count() == 4),
        || format!("the log holds {:?}", std::fs::read_to_string(&after_log)),
    );
    let logged = std::fs::read_to_string(&after_log).expect("read the restored cwds");
    for wanted in [physical(&editing), physical(&building)] {
        assert!(
            logged.lines().any(|line| line == wanted),
            "no restored shell came back in {wanted}; they came back in:\n{logged}"
        );
    }
    server.shutdown();
}

// ------------------------------------------------------------ loss, reported

#[tokio::test]
async fn restore_loss_is_reported_in_status_line_and_session_report() {
    let shell = marker_shell("loss", ":");
    let mut env = Env::new("loss");
    env.set_var("SHELL", &shell.path());

    let server = env.server();
    let mut wire = connected(&env).await;
    let doomed = dir(&env.scratch(), "doomed");
    let (_, root) = workspace(&mut wire, "work").await;
    let pane = split_in(&mut wire, root, &doomed).await;
    rename(&mut wire, pane, "doomed").await;
    wait_until(
        "the save records the pane about to lose its directory",
        || snapshot_mentions(&env, &doomed.to_string_lossy()),
    );

    // The deliberately broken item: the directory is removed while no server
    // is running, so the restore meets a cwd that is simply gone.
    server.kill_dash_9();
    wait_until("the killed server's shells are reaped", || {
        processes_with_arg(shell.marker()) == 0
    });
    std::fs::remove_dir_all(&doomed).expect("remove the saved directory");

    let server = env.server();
    let mut wire = connected(&env).await;
    let state = session_state(&mut wire).await;
    assert_eq!(
        state["restore"]["degraded"], 1,
        "the vanished directory must count as one degraded pane: {state}"
    );
    assert_eq!(
        state["restore"]["lost"], 0,
        "a missing directory degrades a pane, it does not lose it: {state}"
    );
    assert_eq!(
        state["panes"].as_array().unwrap().len(),
        2,
        "the degraded pane is kept, respawned in the home directory: {state}"
    );

    // Place two: the human report names the pane, its label and the path.
    let report = env.run(&["session", "report"]).ok().to_owned();
    assert!(
        report.starts_with("0 lost, 1 degraded"),
        "the report leads with its counts:\n{report}"
    );
    assert!(
        report.contains("degraded  pane"),
        "the report names the severity and the entity:\n{report}"
    );
    assert!(
        report.contains(&doomed.to_string_lossy().into_owned()),
        "the report names the directory the pane lost:\n{report}"
    );

    // Place three: an attached client's status line, rasterized off the pty.
    let mut term = env.attach_on_tty(&[], ROWS, COLS);
    term.wait_for(ALT_ENTER);
    term.wait_output("a fully painted frame", |seen| painted(&rasterize(seen)));
    let settled = term.wait_settled();
    assert!(
        render(&settled).contains("⚠1"),
        "the status line shows the restore's one loss; the screen held:\n{}",
        render(&settled)
    );

    term.chord(b'd');
    assert_eq!(term.wait(), Some(0));
    server.shutdown();
}
