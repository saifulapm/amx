//! T13 acceptance: a resize settles into exactly one server report and one
//! repaint, however many `SIGWINCH`-driven notes arrived first. Fails without
//! the change: make `settle_resize` call `report`/`repaint` once per
//! [`App::note_resize`] instead of once per settle, and this asserts more
//! than one of each.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::App;
use amx_client::term::TermSize;
use amx_proto::ClientInfo;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

#[tokio::test]
async fn resize_triggers_one_server_resize_and_one_full_repaint() {
    let server = support::Server::start("resize").await;
    let pty = support::open_pty();

    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach");
    let repaints_before = app.repaints;

    // A burst: three signals arrive before anything settles, as a dragged
    // terminal resize would deliver them.
    app.note_resize(TermSize { rows: 20, cols: 60 });
    app.note_resize(TermSize {
        rows: 30,
        cols: 100,
    });
    app.note_resize(TermSize {
        rows: 40,
        cols: 120,
    });
    assert!(app.has_pending_resize());

    let mut reports = Vec::new();
    let settled = app.settle_resize(&mut |size| reports.push(size));

    assert!(settled, "a pending resize must settle");
    assert_eq!(
        reports,
        vec![TermSize {
            rows: 40,
            cols: 120
        }],
        "exactly one report, carrying only the last size noted"
    );
    assert_eq!(
        app.repaints,
        repaints_before + 1,
        "exactly one full repaint for the whole burst"
    );
    assert!(!app.has_pending_resize());

    // Settling again with nothing pending must neither report nor repaint.
    let settled_again = app.settle_resize(&mut |size| reports.push(size));
    assert!(!settled_again);
    assert_eq!(reports.len(), 1);
    assert_eq!(app.repaints, repaints_before + 1);

    server.shutdown().await;
}
