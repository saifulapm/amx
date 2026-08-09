//! U07 and V14 acceptance: what the status line says — labels, the attention
//! count 04 §5 requires ("the status line renders `⚑3` from the same state"),
//! the focused pane's agent status, and the restore-loss indicator 04 §6
//! requires ("shown in the status line ... never log-only"). What the same line
//! says about the *agents* in the session — D15's per-workspace breakdown, the
//! queue head and its age, D14's compact form — is `status_agents.rs`.
//!
//! The assertions are made against rasterized frame bytes rather than against
//! the cached `String`, because the thing a user sees is the row that lands on
//! their terminal: a status line that is built correctly and then truncated,
//! misplaced or drawn over is still a bug.
//!
//! The summary is folded into the model here rather than arriving over the
//! wire: `session.state` carries it (U01 froze the field, `wired.rs` folds it)
//! but only a server that has actually restored a session fills it in, and
//! that is U06's `Core::restore`. The wire-to-status path end to end is U09's
//! rig test; this pins the half that turns a summary into pixels.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::App;
use amx_client::model::WorkspaceModel;
use amx_core::agent::{AgentSnapshot, AgentState, StatusCause};
use amx_core::{Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::session::RestoreSummary;

/// The pty every test here attaches to.
const ROWS: u16 = 24;
const COLS: u16 = 80;

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-status-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// Attach to a real server, mirror one labelled workspace with one pane, and
/// hand back the app with that pane focused.
async fn app_with_one_pane(
    tag: &'static str,
    server: &support::Server,
    pty_slave: std::fs::File,
) -> (TestApp, PaneId) {
    let mut app = App::attach(server.socket(), pty_slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");
    let pane = PaneId::new_v4();
    app.adopt_workspace(
        WorkspaceId::new_v4(),
        WorkspaceModel {
            label: Some(tag.to_owned()),
            layout: BspLayout::with_root(pane),
        },
    );
    // Records the focus the status line and the cursor both read.
    assert_eq!(app.focused_pane(), Some(pane));
    (app, pane)
}

/// The text of the status row of the last painted frame.
fn status_row(app: &TestApp) -> String {
    let cells = support::rasterize(app.frame());
    (0..COLS)
        .map(|col| cells.get(&(ROWS - 1, col)).copied().unwrap_or(' '))
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[tokio::test]
async fn status_line_shows_loss_count_after_a_lossy_restore() {
    let server = support::Server::start("loss").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, _pane) = app_with_one_pane("work", &server, pty.slave).await;

    app.model().set_restore(Some(RestoreSummary {
        restored: 4,
        lost: 1,
        degraded: 1,
    }));
    app.repaint();

    // One indicator, counting every entry the report holds: a pruned pane and
    // a pane that came back without its directory are both things the user
    // has to be told about, and `amx session report` is where they are told
    // apart.
    assert!(
        status_row(&app).contains("⚠2"),
        "the status line must show the loss count: {:?}",
        status_row(&app),
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn status_line_is_clean_after_a_clean_restore() {
    let server = support::Server::start("clean").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, _pane) = app_with_one_pane("work", &server, pty.slave).await;

    app.model().set_restore(Some(RestoreSummary {
        restored: 4,
        lost: 0,
        degraded: 0,
    }));
    app.repaint();
    let restored = status_row(&app);
    assert!(
        !restored.contains('⚠'),
        "a restore that lost nothing is not worth a warning: {restored:?}",
    );
    assert!(restored.contains("work"), "the label still renders");

    // And a server that restored nothing at all reports nothing at all, which
    // must read the same way.
    app.model().set_restore(None);
    app.repaint();
    assert!(!status_row(&app).contains('⚠'));

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn status_line_names_the_focused_pane_once_it_is_labelled() {
    let server = support::Server::start("label").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, pane) = app_with_one_pane("work", &server, pty.slave).await;

    app.repaint();
    let unlabelled = status_row(&app);
    assert!(unlabelled.contains("work"));
    assert!(
        !unlabelled.contains("editor"),
        "nothing is named editor yet: {unlabelled:?}",
    );

    app.model().set_pane_label(pane, Some("editor".to_owned()));
    app.repaint();
    let labelled = status_row(&app);
    assert!(
        labelled.contains("work") && labelled.contains("editor"),
        "both labels belong on the line: {labelled:?}",
    );

    // Clearing it takes it back off, so a stale name can never outlive the
    // state it mirrors.
    app.model().set_pane_label(pane, None);
    app.repaint();
    assert!(!status_row(&app).contains("editor"));

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn status_line_renders_attention_count_and_updates_on_dequeue() {
    let server = support::Server::start("attn").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, pane) = app_with_one_pane("work", &server, pty.slave).await;
    let second = PaneId::new_v4();

    app.repaint();
    assert!(
        !status_row(&app).contains('⚑'),
        "an empty queue is worth no indicator: {:?}",
        status_row(&app),
    );

    assert!(app.model().enqueue_attention(pane));
    app.repaint();
    assert!(
        status_row(&app).contains("⚑1"),
        "one blocked agent, one on the count: {:?}",
        status_row(&app),
    );

    // The regression this test exists for: the line is cached against its
    // inputs, so a count added to the rendered text and not to the equality
    // guard renders once and then freezes. Every step below is a repaint whose
    // only changed input is the queue.
    assert!(app.model().enqueue_attention(second));
    app.repaint();
    assert!(
        status_row(&app).contains("⚑2"),
        "the count follows the queue up: {:?}",
        status_row(&app),
    );

    assert!(app.model().dequeue_attention(pane));
    app.repaint();
    assert!(
        status_row(&app).contains("⚑1"),
        "and back down again: {:?}",
        status_row(&app),
    );

    assert!(app.model().dequeue_attention(second));
    app.repaint();
    assert!(
        !status_row(&app).contains('⚑'),
        "an emptied queue takes the indicator off: {:?}",
        status_row(&app),
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn status_line_names_the_focused_panes_agent_state() {
    let server = support::Server::start("agst").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, pane) = app_with_one_pane("work", &server, pty.slave).await;

    app.repaint();
    assert!(
        !status_row(&app).contains("working"),
        "nothing is tracked in that pane yet: {:?}",
        status_row(&app),
    );

    app.model().set_pane_agent(
        pane,
        Some(AgentSnapshot {
            kind: None,
            state: AgentState::Working,
            cause: StatusCause::Hook,
            transition_seq: 12,
            attention: None,
            session_ref: None,
            reason: None,
            since: None,
        }),
    );
    app.repaint();
    assert!(
        status_row(&app).contains("working"),
        "the focused pane's status belongs beside its name: {:?}",
        status_row(&app),
    );

    // Cached against the state too, not only against the count.
    app.model()
        .apply_agent_status(pane, AgentState::Blocked, StatusCause::Hook, 13);
    app.repaint();
    let blocked = status_row(&app);
    assert!(
        blocked.contains("blocked") && !blocked.contains("working"),
        "the line follows the transition: {blocked:?}",
    );

    drop(app);
    server.shutdown().await;
}
