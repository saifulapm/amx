//! U07 acceptance: what the status line says — labels, and the restore-loss
//! indicator 04 §6 requires ("shown in the status line ... never log-only").
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
use amx_core::{Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::session::RestoreSummary;

/// The pty every test here attaches to.
const ROWS: u16 = 24;
const COLS: u16 = 80;

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
) -> (App<std::fs::File, Vec<u8>>, PaneId) {
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
fn status_row(app: &App<std::fs::File, Vec<u8>>) -> String {
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
