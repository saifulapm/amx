//! U08: configuration reaching a running server — load, watch, hot apply.
//!
//! The claims defended here are `docs/07-m1-plan.md` D-M1-8's, one test each:
//!
//! - **A machine with no `config.toml` is a machine running the defaults.**
//!   Absent is not broken, so startup neither fails nor writes a file.
//! - **`[terminal] shell` is hot for the *next* pane and only the next pane.**
//!   The pane already running keeps the process it started with — a config
//!   edit that restarted a shell would throw away whatever was in it.
//! - **`[persist] history` is hot both ways, asymmetrically.** On dumps at the
//!   next save; off deletes what is on disk without waiting for one, because
//!   scrollback holds secrets (04 §6) and "at the next save" could be never.
//! - **A broken edit yanks nothing.** A section that does not parse keeps its
//!   running values while its valid neighbours apply; a file that does not
//!   parse keeps everything and says every section was kept.
//! - **Every reload is announced.** `ConfigReloaded` carries the rejection
//!   count, which is what lets these tests wait on a reload instead of polling
//!   for its effects.
//!
//! Nothing here reads the process environment: the config path comes from the
//! `Ctx` each test builds over its own temporary tree, which is what makes two
//! of these tests running side by side independent (04 §2).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use amx_core::config::SECTIONS;
use amx_core::{
    Config, Ctx, Delivery, Event, PaneId, PersistConfig, RowId, RowRange, Subscription,
};
use amx_proto::control::workspace;
use amx_server::actor::{CoreCommand, WorkspaceCall};
use amx_server::config_rt::ConfigRuntime;
use amx_server::persist::sidecar;
use amx_server::platform::watch::{Debounce, watch_path};
use tokio::sync::watch;
use tokio::task::JoinHandle;

// A test crate root's module directory is `tests/`, so the fixtures the
// persistence suite already built need their path spelled out to be reachable
// from here. The `Persist` actor's hot toggle is tested against the same
// stand-in `Core` and the same stand-in panes U05's own suite drives.
#[path = "persist_actor/fixtures.rs"]
#[allow(
    dead_code,
    reason = "this suite drives a subset of the persistence rig"
)]
mod fixtures;
#[path = "persist_actor/seam.rs"]
#[allow(
    dead_code,
    reason = "this suite drives a subset of the persistence rig"
)]
mod seam;
mod support;

use fixtures::{Answers, FakePane, Rig, pane_id};
use support::restore_rig::{Fixture, Running, screen, wait_until};
use support::{PATIENCE, TempDir, ctx_under};

/// The debounce the watcher runs at in this suite.
///
/// The shipped pair is half a second of quiet with a five-second ceiling
/// (`tests/watch.rs` pins those); a reload suite that waited them out would
/// spend its life asleep, so the windows are wound down and the *behaviour*
/// under them is what gets tested.
const FAST: Debounce = Debounce {
    quiet: Duration::from_millis(30),
    cap: Duration::from_millis(500),
};

/// How long a test waits for a save it expects.
///
/// Spent in tokio's *virtual* time, so a debounce that is never armed fails the
/// assertion instead of hanging the test binary.
const WAIT_LIMIT: Duration = Duration::from_secs(60);

/// Let every other task run to a standstill before an assertion.
///
/// The bus, the mailbox and the config channel are three separate branches of
/// the actor's `select!`, so "the event has been published" and "the actor has
/// folded it" are two different facts; this makes the second one true.
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

// --------------------------------------------------------------- the startup

#[tokio::test]
async fn startup_without_a_config_file_uses_defaults() {
    let dir = TempDir::new("dflt");
    let ctx = ctx_under(dir.path());
    assert!(!ctx.config_path.exists(), "the fixture writes no config");

    let config = ConfigRuntime::load(&ctx);

    assert_eq!(
        config.config(),
        Config::default(),
        "an absent file says exactly what an empty one says",
    );
    assert!(!config.config().persist.history, "sidecars are opt-in");
    assert_eq!(config.config().terminal.shell, None, "the shell is $SHELL");
    assert!(
        config.diagnostics().is_empty(),
        "a missing config file is not a failure to report",
    );
    assert!(
        !ctx.config_path.exists(),
        "reading a config file must not create one",
    );
}

// ----------------------------------------------------------------- the shell

#[tokio::test]
async fn editing_shell_applies_to_new_panes_without_restart() {
    let mut fx = Fixture::new("shel");
    let alpha = marker_shell(fx.dir.path(), "alpha");
    let beta = marker_shell(fx.dir.path(), "beta");

    let (config, receiver) = watch::channel(shell_config(&alpha));
    fx.core.set_config(receiver);
    let running = fx.start();

    let first = new_workspace_pane(&running).await;
    let started = running.wiring_of(first).await;
    wait_until("the first pane runs the configured shell", async || {
        screen(&started.frames.latest()).contains("alpha")
    })
    .await;

    // The edit: same server, same session, no restart of anything.
    config.send_modify(|config| config.terminal.shell = Some(path_string(&beta)));

    let second = new_workspace_pane(&running).await;
    let next = running.wiring_of(second).await;
    wait_until(
        "the next pane runs the newly configured shell",
        async || screen(&next.frames.latest()).contains("beta"),
    )
    .await;

    // And the pane that was already running is untouched: its process was
    // started once, by the shell that was configured then.
    let unchanged = screen(&started.frames.latest());
    assert!(
        unchanged.contains("alpha") && !unchanged.contains("beta"),
        "a config reload must never restart a live pane, got: {unchanged}",
    );

    running.into_core().await;
}

#[tokio::test]
async fn an_empty_configured_shell_counts_as_unset() {
    // `shell = ""` is a half-finished edit, not a request to exec the empty
    // string: it falls through to `$SHELL` and then `/bin/sh`, exactly as an
    // absent key does, so the session keeps working while the user types.
    let mut fx = Fixture::new("mtsh");
    let (_config, receiver) = watch::channel(shell_config(Path::new("")));
    fx.core.set_config(receiver);
    let running = fx.start();

    // Both calls fail loudly if the spawn did not happen: a workspace whose
    // root pane could not start answers with an error, and a pane with no live
    // process has no wiring to hand out.
    let pane = new_workspace_pane(&running).await;
    let _live = running.wiring_of(pane).await;

    running.into_core().await;
}

// -------------------------------------------------------------- the sidecars

/// Time is virtual here: the save this test waits for is the debounced one the
/// edit itself schedules, and waiting out half a second of quiet on the wall
/// clock is what the rig's pacing rules exist to forbid.
#[tokio::test(start_paused = true)]
async fn enabling_history_dumps_sidecars_on_the_next_save() {
    let root = TempDir::new("hist");
    let pane = FakePane::serving(&[("cargo test --all", false)]);
    let rig = Rig::under(&root)
        .history(false)
        .answers(Answers::WithPanes(vec![(pane_id(0), pane.handle.clone())]))
        .start();
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::single(RowId::FIRST),
    });
    settle().await;

    rig.flush().await.expect("the save while history is off");
    assert_eq!(pane.asked(), 0, "history is off: nothing is dumped");
    assert!(
        sidecar::load(rig.state_dir(), pane_id(0))
            .expect("no sidecar is not a failure")
            .is_none(),
    );

    // Turning it on is a change like any other: it dirties the session and the
    // ordinary debounce carries it to disk. Nothing is flushed by hand here —
    // an edit that needed a later structural change to take effect would leave
    // the user's scrollback unsaved until they happened to split a pane.
    rig.config
        .send_modify(|config| config.persist.history = true);
    tokio::time::timeout(WAIT_LIMIT, rig.spy.captures(2))
        .await
        .expect("enabling history schedules the save that dumps it");

    let state_dir = rig.state_dir().to_path_buf();
    let report = rig.stop().await;
    assert_eq!(report.dumps, 1, "the save that followed the edit dumped");
    assert_eq!(report.wipes, 0, "nothing was turned off");
    assert_eq!(pane.asked(), 1, "the pane was asked for its history once");
    let dumped = sidecar::load(&state_dir, pane_id(0))
        .expect("the sidecar reads back")
        .expect("the sidecar exists");
    assert_eq!(dumped.rows.len(), 1);
}

#[tokio::test]
async fn disabling_history_wipes_sidecars_immediately() {
    let root = TempDir::new("wipe");
    let pane = FakePane::serving(&[("secret", false)]);
    let rig = Rig::under(&root)
        .history(true)
        .answers(Answers::WithPanes(vec![(pane_id(0), pane.handle.clone())]))
        .start();
    rig.publish(Event::HistoryCommitted {
        pane: pane_id(0),
        range: RowRange::single(RowId::FIRST),
    });
    settle().await;
    rig.flush().await.expect("the save that dumps the sidecar");

    let dumped = sidecar::path(rig.state_dir(), pane_id(0));
    assert!(dumped.is_file(), "the sidecar this test is about exists");

    // No flush, no structural change, no debounce: turning the toggle off is
    // the whole trigger, because scrollback that the user has just asked amx
    // to stop keeping must not sit on the disk until the next save.
    rig.config
        .send_modify(|config| config.persist.history = false);
    wait_until("the sidecars are gone", async || !dumped.exists()).await;
    assert!(
        !sidecar::dir(rig.state_dir()).exists(),
        "the history directory goes with its contents",
    );

    let report = rig.stop().await;
    assert_eq!(report.wipes, 1);
    assert_eq!(report.dumps, 1, "the wipe is not a save");
}

// --------------------------------------------------------------- the reloads

#[tokio::test]
async fn a_broken_edit_keeps_the_running_config_and_reports_rejected_sections() {
    let mut watched = Watched::new(
        "brkn",
        "[persist]\nhistory = true\n\n[terminal]\nshell = \"/bin/zsh\"\n",
    );
    assert!(watched.config.config().persist.history, "the file applied");

    // One section broken: it keeps what it was running, its neighbour applies.
    watched.write("[persist]\nhistory = \"yes please\"\n\n[terminal]\nshell = \"/bin/dash\"\n");
    assert_eq!(watched.reloaded().await, 1);
    let kept = watched.config.config();
    assert!(
        kept.persist.history,
        "the rejected section kept its running value",
    );
    assert_eq!(
        kept.terminal.shell.as_deref(),
        Some("/bin/dash"),
        "a valid section still applies beside a broken one",
    );
    let diagnostics = watched.config.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].section,
        Some("persist"),
        "diagnostics name the section that failed",
    );

    // A file that is not TOML at all: nothing moves, and the count says every
    // section was kept rather than pretending the reload was clean.
    watched.write("[terminal\nshell = ");
    assert_eq!(
        watched.reloaded().await,
        u32::try_from(SECTIONS.len()).unwrap(),
    );
    assert_eq!(
        watched.config.config(),
        kept,
        "a typo mid-edit yanks nothing out from under the session",
    );
    let diagnostics = watched.config.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].section, None,
        "a whole-file failure is not any one section's fault",
    );

    watched.stop().await;
}

#[tokio::test]
async fn reload_publishes_config_reloaded_with_the_rejection_count() {
    let mut watched = Watched::new("evnt", "");

    watched.write("[terminal]\nshell = \"/bin/dash\"\n");
    assert_eq!(
        watched.reloaded().await,
        0,
        "a clean reload rejects nothing"
    );
    assert_eq!(
        watched.config.config().terminal.shell.as_deref(),
        Some("/bin/dash"),
    );

    watched.write("[persist]\nhistory = 7\n");
    assert_eq!(watched.reloaded().await, 1);
    assert_eq!(
        watched.config.config().terminal.shell,
        None,
        "a section absent from the file resets to its defaults",
    );

    // Deleting the file says what emptying it says, and is still announced.
    std::fs::remove_file(&watched.ctx.config_path).expect("remove the config file");
    assert_eq!(watched.reloaded().await, 0);
    assert_eq!(watched.config.config(), Config::default());

    watched.stop().await;
}

// ------------------------------------------------------------------ the rigs

/// A `ConfigRuntime` over a real file, with its watcher task running.
struct Watched {
    ctx: Ctx,
    config: ConfigRuntime,
    events: Subscription,
    task: JoinHandle<()>,
    _dir: TempDir,
}

impl Watched {
    /// A watched config file holding `initial`, already loaded.
    fn new(tag: &str, initial: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        let parent = ctx.config_path.parent().expect("the config path has a dir");
        std::fs::create_dir_all(parent).expect("create the config directory");
        std::fs::write(&ctx.config_path, initial).expect("write the config file");

        let config = ConfigRuntime::load(&ctx);
        let watcher = watch_path(&ctx.config_path, ctx.cancel.clone(), FAST)
            .expect("establish the config watch");
        // Subscribed before the task starts, so no reload can be published
        // into the gap between the two.
        let events = ctx.bus.subscribe();
        let task = tokio::spawn(config.clone().run(Arc::clone(&ctx.bus), watcher));
        Self {
            ctx,
            config,
            events,
            task,
            _dir: dir,
        }
    }

    /// Rewrite the config file, the way an editor saving in place does.
    fn write(&self, text: &str) {
        std::fs::write(&self.ctx.config_path, text).expect("rewrite the config file");
    }

    /// Wait for the next reload and answer with its rejection count.
    ///
    /// Event-driven on purpose: the reload has a debounce in front of it and a
    /// blocking read inside it, so anything that polled for the *effect* would
    /// be racing both. `ConfigReloaded` exists so that nothing has to.
    async fn reloaded(&mut self) -> u32 {
        let wait = async {
            loop {
                match self.events.recv().await {
                    Some(Delivery::Event(envelope)) => {
                        if let Event::ConfigReloaded { rejected_sections } = envelope.event {
                            return rejected_sections;
                        }
                    }
                    Some(Delivery::Gap { .. }) => {}
                    None => panic!("the bus closed before the reload was announced"),
                }
            }
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .expect("the watcher must announce the reload")
    }

    /// Cancel the session and join the watcher task.
    async fn stop(self) {
        self.ctx.cancel.cancel();
        tokio::time::timeout(PATIENCE, self.task)
            .await
            .expect("the watcher task stops when the session is cancelled")
            .expect("the watcher task did not panic");
    }
}

/// A config whose `[terminal] shell` is `shell`.
fn shell_config(shell: &Path) -> Config {
    Config {
        terminal: amx_core::TerminalConfig {
            shell: Some(path_string(shell)),
        },
        persist: PersistConfig::default(),
        update: amx_core::UpdateConfig::default(),
    }
}

fn path_string(path: &Path) -> String {
    path.to_str().expect("the temp path is utf-8").to_owned()
}

/// Write an executable shell that announces itself and then stays alive.
///
/// A pane's identity has to be readable off its own grid — there is no wire
/// verb for "what is this pane running" — so the stand-in shell prints a
/// marker and then blocks in `cat`, which keeps the pty open for the rest of
/// the test the way a real interactive shell would.
fn marker_shell(root: &Path, marker: &str) -> PathBuf {
    let path = root.join(format!("shell-{marker}"));
    std::fs::write(&path, format!("#!/bin/sh\necho {marker}\nexec cat\n"))
        .expect("write the stand-in shell");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the stand-in shell executable");
    path
}

/// Create a workspace over the mailbox and answer with its root pane.
async fn new_workspace_pane(running: &Running) -> PaneId {
    let created: workspace::CreateReply = running
        .call(|reply| {
            CoreCommand::Workspace(WorkspaceCall::Create {
                params: workspace::CreateParams {
                    label: None,
                    focus: true,
                },
                reply,
            })
        })
        .await;
    let state = running.state().await;
    state
        .workspaces
        .iter()
        .find(|ws| ws.workspace == created.workspace)
        .and_then(|ws| ws.focus)
        .expect("a fresh workspace has a focused root pane")
}
