//! The server half of the mouse path: what a pane's application asks its own
//! terminal for, and how that answer reaches `session.state` (D9, D14).
//!
//! Against **real programs on real ptys**, never a hand-set flag. That is the
//! whole point of the suite: the mechanism was designed, typed and unit-tested
//! three milestones ago and was dead in a running amx, because
//! `Terminal::mouse_tracking` had no caller and no wire field carried the
//! answer (`docs/11-m4-plan.md` D-M4-1). A test that set the mode itself would
//! have passed then too.
//!
//! Every pane here is a `/bin/sh` script that emits the escape sequences a
//! real full-screen application emits, and every assertion is read back
//! through `session.state` — the same reply an attached client folds.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use amx_core::{Bus, Ctx, PaneId, Scheduled, SessionName};
use amx_proto::control::session::{MouseEvents, MouseFormat, MouseMode};
use amx_proto::control::{pane, workspace};
use amx_server::actor::core::Core;
use amx_server::actor::{
    CoreCommand, CoreHandle, PaneCall, PaneCommand, PaneWiring, SessionCall, StreamCall,
    WorkspaceCall,
};
use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// How long a test waits for something that must happen.
const PATIENCE: Duration = Duration::from_secs(15);

/// How long a poll loop waits between looks at its condition.
const TICK: Duration = Duration::from_millis(10);

/// The SGR pair every terminfo entry names as the enable string, in the order
/// it names them (`docs/notes/m4-mouse-path.md` §2.1).
const ENABLE_SGR: &str = r"\033[?1006h\033[?1000h";

/// A `Ctx` with fabricated, never-touched-on-disk paths.
fn fresh_ctx(tag: &str) -> Ctx {
    let root = PathBuf::from("/amx-test-mouse").join(tag);
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        runtime_dir: root.join("runtime"),
        socket: root.join("runtime/sock"),
        state_dir: root.join("state"),
        config_path: root.join("config/amx/config.toml"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

/// A `Core` on the real actor loop, with real panes under it.
struct Harness {
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    task: JoinHandle<Core>,
}

impl Harness {
    fn start(tag: &str) -> Self {
        let ctx = fresh_ctx(tag);
        let (tx, rx) = mpsc::channel(64);
        let core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));
        let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));
        Self { ctx, tx, task }
    }

    /// A workspace, and a pane split off its root running `script` under
    /// `/bin/sh`.
    ///
    /// The split rather than the root, because a root pane runs the user's
    /// shell and only `pane.split` takes a command.
    async fn pane_running(&self, script: &str) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Workspace(WorkspaceCall::Create {
                params: workspace::CreateParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        let created = answer
            .await
            .expect("core answered")
            .expect("create succeeds");
        let root = self
            .state()
            .await
            .workspaces
            .iter()
            .find(|ws| ws.workspace == created.workspace)
            .expect("the workspace just created")
            .focus
            .expect("a fresh workspace focuses its root pane");
        self.split_running(root, script).await
    }

    async fn state(&self) -> amx_proto::control::session::StateReply {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Session(SessionCall::State {
                params: amx_proto::control::session::StateParams::default(),
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer.await.expect("core answered").expect("state answers")
    }

    /// What `session.state` says `pane` asked for.
    async fn mouse_of(&self, pane: PaneId) -> Option<MouseMode> {
        self.state()
            .await
            .panes
            .iter()
            .find(|row| row.pane == pane)
            .expect("the pane is in the state tree")
            .mouse
    }

    /// Poll `session.state` until `pane`'s mouse mode is `want`.
    async fn wait_for_mouse(&self, pane: PaneId, want: Option<MouseMode>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let seen = self.mouse_of(pane).await;
            if seen == want {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "pane never reported {want:?}; last answer was {seen:?}",
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Put `bytes` in front of the pane's child, which is how these scripts
    /// are told to move to their next step.
    async fn write(&self, pane: PaneId, bytes: &'static [u8]) {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
            .await
            .expect("core mailbox is open");
        let wiring: PaneWiring = answer
            .await
            .expect("core answered")
            .expect("the pane is backed by a live actor");
        wiring
            .handle
            .send(PaneCommand::Write(Bytes::from_static(bytes)))
            .await
            .expect("the pane is running");
    }

    async fn split_running(&self, from: PaneId, script: &str) -> PaneId {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(CoreCommand::Pane(PaneCall::Split {
                params: pane::SplitParams {
                    pane: from,
                    direction: pane::SplitDirection::Vertical,
                    command: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
                    cwd: Some(PathBuf::from("/")),
                },
                reply,
            }))
            .await
            .expect("core mailbox is open");
        answer
            .await
            .expect("core answered")
            .expect("split succeeds")
            .pane
    }

    async fn stop(self) {
        self.ctx.cancel.cancel();
        drop(self.tx);
        let _ = tokio::time::timeout(PATIENCE, self.task)
            .await
            .expect("the core loop ended");
    }
}

/// A script that prints `escapes`, then waits for a line on stdin before
/// printing `then` and parking.
///
/// The wait is what makes the second half of a two-step test deterministic:
/// nothing here races a sleep, so a loaded machine changes how long the test
/// takes and never what it observes.
fn two_step(escapes: &str, then: &str) -> String {
    format!("printf '{escapes}'; read x; printf '{then}'; sleep 300")
}

#[tokio::test]
async fn a_pane_that_asks_for_sgr_reporting_says_so_in_session_state() {
    let h = Harness::start("sgr");
    let pane = h
        .pane_running(&format!("printf '{ENABLE_SGR}'; sleep 300"))
        .await;

    h.wait_for_mouse(
        pane,
        Some(MouseMode {
            events: MouseEvents::Normal,
            format: MouseFormat::Sgr,
        }),
    )
    .await;

    h.stop().await;
}

#[tokio::test]
async fn a_pane_running_a_plain_shell_asks_for_nothing() {
    let h = Harness::start("none");
    let pane = h.pane_running("sleep 300").await;
    // A pane that never asked has no entry at all, which is what the absent
    // field means on the wire and what a client turns into "do not relay".
    let quiet = h.split_running(pane, "sleep 300").await;
    h.wait_for_mouse(quiet, None).await;
    assert_eq!(h.mouse_of(pane).await, None);

    h.stop().await;
}

/// X01 F-2's case: `?1000` without `?1006` is the X10 encoding, and a client
/// reading this is meant to *decline* to relay SGR bytes to it.
#[tokio::test]
async fn a_pane_that_asks_without_a_format_reports_the_x10_encoding() {
    let h = Harness::start("x10");
    let pane = h.pane_running(r"printf '\033[?1000h'; sleep 300").await;

    h.wait_for_mouse(
        pane,
        Some(MouseMode {
            events: MouseEvents::Normal,
            format: MouseFormat::X10,
        }),
    )
    .await;

    h.stop().await;
}

/// The half that matters as much as the enable: an application that gives the
/// mouse back must stop being relayed to, or a pane that has quit vim keeps
/// receiving reports its shell will print as garbage.
#[tokio::test]
async fn giving_the_mouse_back_clears_the_pane_state() {
    let h = Harness::start("release");
    let pane = h
        .pane_running(&two_step(ENABLE_SGR, r"\033[?1000l\033[?1006l"))
        .await;

    h.wait_for_mouse(
        pane,
        Some(MouseMode {
            events: MouseEvents::Normal,
            format: MouseFormat::Sgr,
        }),
    )
    .await;
    h.write(pane, b"\n").await;
    h.wait_for_mouse(pane, None).await;

    h.stop().await;
}

/// A pane that upgrades what it asks for is followed, not latched: `?1003`
/// after `?1000` is more motion, and a reader is entitled to know.
#[tokio::test]
async fn a_pane_that_changes_its_mind_is_followed() {
    let h = Harness::start("upgrade");
    let pane = h.pane_running(&two_step(ENABLE_SGR, r"\033[?1003h")).await;

    h.wait_for_mouse(
        pane,
        Some(MouseMode {
            events: MouseEvents::Normal,
            format: MouseFormat::Sgr,
        }),
    )
    .await;
    h.write(pane, b"\n").await;
    h.wait_for_mouse(
        pane,
        Some(MouseMode {
            events: MouseEvents::Any,
            format: MouseFormat::Sgr,
        }),
    )
    .await;

    h.stop().await;
}
