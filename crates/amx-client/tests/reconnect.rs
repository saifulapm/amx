//! W09 acceptance: an attached client rides a server swap with its screen
//! intact (`docs/09-m3-plan.md` D-M3-7).
//!
//! Every disconnect here is the real one. `GatewayControl::retire` is W06's
//! §3 step 11 — stop accepting, close every connection, unlink the socket —
//! and `restore` is the abort path taking it back; between them the session's
//! panes are untouched and its `Core` is the same actor, which is exactly what
//! a handoff hands to a successor. Nothing simulates an EOF by closing a
//! socket a test opened itself, because what is under test is what the client
//! does about *this* server going away and coming back.
//!
//! The two branches are driven separately because only one of them can be:
//! retire-and-restore keeps the `SessionId`, and the only way to put a
//! different server on the same path is to bind a different `Core` there
//! (`support::Server::usurp`).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::time::Duration;

use amx_client::app::{App, Reattached, ReconnectPolicy};
use amx_client::net::Session;
use amx_core::PaneId;
use amx_proto::ClientInfo;
use serde_json::json;

/// How long a test waits for the client to fold something before calling it a
/// failure. Generous, and never on the green path: the client wakes on a
/// frame, so a passing run spends microseconds here.
const DEADLINE: Duration = Duration::from_secs(10);

/// How long "the server sent nothing else" is watched for.
///
/// A silence cannot be waited for, only bounded — the same shape W08's
/// `next_frame_within` gave the server side of this claim.
const SILENCE: Duration = Duration::from_millis(300);

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-reconnect-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// Attach to a real session over its real socket.
///
/// The pty master comes back with the app: the slave moved into the terminal
/// guard is unusable the moment nothing holds the master open, so the caller
/// has to keep it alive for the length of the test.
async fn attached(tag: &'static str) -> (support::Server, TestApp, std::fs::File) {
    let server = support::Server::start(tag).await;
    let pty = support::open_pty();
    let app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");
    (server, app, pty.master)
}

/// Step the wire until `done` holds, failing on the deadline.
async fn pump_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) {
    let stepped = tokio::time::timeout(DEADLINE, async {
        while !done(app) {
            app.step_frame().await.expect("read one server frame");
        }
    })
    .await;
    assert!(stepped.is_ok(), "the client never folded what it was owed");
}

/// Keep reading for a bounded window, so a claim about what the server did
/// *not* send is made against frames that had time to arrive.
async fn pump_quietly(app: &mut TestApp) {
    let _ = tokio::time::timeout(SILENCE, async {
        loop {
            if app.step_frame().await.is_err() {
                return;
            }
        }
    })
    .await;
}

/// A one-shot verb connection to the same socket — a second client, which is
/// what changes the session while the attached one is not looking.
async fn verb(server: &support::Server) -> Session {
    let stream = amx_client::net::connect(server.socket())
        .await
        .expect("connect a verb connection");
    Session::attach(stream, client_info(), false, None)
        .await
        .expect("negotiate the verb connection")
        .0
}

/// Split `pane`, running a command that paints `mark` and then holds the pty
/// open without producing anything else.
///
/// `/bin/sh` explicitly and never the user's `$SHELL`: what this pane is for
/// is putting known bytes on a grid, and a login shell's prompt, its startup
/// files and its own line editor are three ways for that to stop being known.
async fn split_marked(session: &mut Session, pane: PaneId, mark: &str) -> PaneId {
    let before = pane_ids(session).await;
    let script = format!("printf '{mark}'; exec cat");
    session
        .call(
            "pane.split",
            json!({
                "pane": pane,
                "direction": "vertical",
                "command": ["/bin/sh", "-c", script],
            }),
        )
        .await
        .expect("split a pane");
    let after = pane_ids(session).await;
    *after
        .iter()
        .find(|id| !before.contains(id))
        .expect("the split minted a pane")
}

/// Every pane the session holds.
async fn pane_ids(session: &mut Session) -> Vec<PaneId> {
    let state = session
        .call("session.state", json!({}))
        .await
        .expect("read session.state");
    state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .map(|pane| serde_json::from_value(pane["pane"].clone()).expect("a pane id"))
        .collect()
}

/// Whether `pane`'s cached grid spells `mark` somewhere on it.
fn shows(app: &mut TestApp, pane: PaneId, mark: &str) -> bool {
    let Some(grid) = app.model().pane(pane) else {
        return false;
    };
    (0..grid.rows()).any(|row| {
        let line: String = (0..grid.cols())
            .filter_map(|col| grid.cell(row, col).map(|cell| cell.ch))
            .collect();
        line.contains(mark)
    })
}

/// How many keyframes `pane`'s grid has absorbed.
fn keyframes(app: &mut TestApp, pane: PaneId) -> u64 {
    app.model()
        .pane(pane)
        .map_or(0, amx_client::model::PaneGrid::keyframes)
}

#[tokio::test]
async fn a_dropped_client_reattaches_with_resume_and_repaints_only_stale_panes() {
    let (server, mut app, _master) = attached("rcn1").await;
    let root = app.focused_pane().expect("the seeded workspace's pane");

    // Two panes with known content, so what survives the swap is something
    // that demonstrably held cells rather than a blank grid agreeing with a
    // blank grid (the T19 lesson).
    let mut driver = verb(&server).await;
    let kept = split_marked(&mut driver, root, "amx-w09-kept").await;
    pump_until(&mut app, |app| {
        shows(app, kept, "amx-w09-kept") && app.model().pane(root).is_some_and(|g| g.complete())
    })
    .await;

    // The claim the reattach will make: the root pane is one this client can
    // no longer vouch for — the same state a frame torn off the socket
    // mid-delta leaves behind — and `kept` is one it can.
    let generation = app.model().pane(kept).expect("a cached grid").generation();
    app.model().pane_mut(root, 0, 0).doubt();
    let claim = app.resume_claim();
    assert!(
        claim.generations.contains(&(kept, generation)),
        "a complete grid is presented: {:?}",
        claim.generations
    );
    assert!(
        !claim.generations.iter().any(|&(pane, _)| pane == root),
        "a doubted grid is omitted: {:?}",
        claim.generations
    );
    assert!(claim.last_seq > 0, "the cursor moved while the client read");
    let cursor = claim.last_seq;
    let repaints_before = keyframes(&mut app, kept);

    // §3 step 11, for real: the socket stops answering and the panes behind it
    // do not move.
    server.control.retire().await.expect("retire the socket");
    let lost = tokio::time::timeout(DEADLINE, async {
        loop {
            if let Err(err) = app.step_frame().await {
                return err;
            }
        }
    })
    .await
    .expect("the client noticed the socket end");
    assert!(
        matches!(lost, amx_client::app::AppError::Net(ref net) if net.is_transport()),
        "a retired gateway ends the connection, it does not corrupt it: {lost}"
    );

    server
        .control
        .restore()
        .await
        .expect("take the socket back");
    let how = app.reconnect().await.expect("reattach to the same session");
    assert_eq!(how, Reattached::Resumed, "the SessionId did not change");
    assert_eq!(app.reconnects(), 1);
    assert!(
        app.cursor() >= cursor,
        "the client resumed from where it had read to"
    );

    // The doubted pane repaints; the vouched one is owed nothing and gets
    // nothing, and its cells are still the ones the server drew.
    pump_until(&mut app, |app| keyframes(app, root) >= 2).await;
    pump_quietly(&mut app).await;
    assert_eq!(
        keyframes(&mut app, kept),
        repaints_before,
        "a pane whose generation the client presented was owed no keyframe"
    );
    assert!(
        shows(&mut app, kept, "amx-w09-kept"),
        "and its cells survived the swap"
    );

    drop(driver);
    server.shutdown().await;
}

#[tokio::test]
async fn a_new_session_id_drops_caches_and_full_resyncs() {
    let (mut server, mut app, _master) = attached("rcn2").await;
    let root = app.focused_pane().expect("the seeded workspace's pane");
    let mut driver = verb(&server).await;
    let marked = split_marked(&mut driver, root, "amx-w09-gone").await;
    pump_until(&mut app, |app| shows(app, marked, "amx-w09-gone")).await;
    drop(driver);

    // A different `Core` binds the path: same socket, different session, and
    // nothing this client holds describes it.
    server.usurp().await;
    let how = app
        .reconnect()
        .await
        .expect("reattach to whatever answers the path");
    assert_eq!(how, Reattached::Fresh, "the SessionId changed");

    assert!(
        app.model().pane(marked).is_none(),
        "a grid from another server's id space is dropped, not presented"
    );
    assert!(
        !app.resume_claim()
            .generations
            .iter()
            .any(|&(pane, _)| pane == marked || pane == root),
        "and nothing from it is claimed on the next hello"
    );

    // A fresh attach seeds a workspace and folds it, exactly as a first attach
    // does: the successor's session is on screen, not the dead one's.
    let now = app.focused_pane().expect("the new session's seeded pane");
    assert_ne!(now, root, "a different server minted a different pane");
    pump_until(&mut app, |app| {
        app.model().pane(now).is_some_and(|grid| grid.complete())
    })
    .await;

    server.shutdown().await;
}

#[tokio::test]
async fn a_reattach_that_never_finds_a_server_gives_up_at_its_deadline() {
    let (server, mut app, _master) = attached("rcn3").await;
    app.set_reconnect_policy(ReconnectPolicy {
        first: Duration::from_millis(5),
        longest: Duration::from_millis(20),
        deadline: Duration::from_millis(200),
    });

    // Retired and never restored: the path is gone and nothing is coming.
    server.control.retire().await.expect("retire the socket");
    let started = std::time::Instant::now();
    let err = app
        .reconnect()
        .await
        .expect_err("nothing answers the socket");
    let took = started.elapsed();

    assert!(
        matches!(err, amx_client::app::AppError::Unreachable { .. }),
        "the failure names the session going away: {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("did not come back"),
        "and says so in a sentence a user can act on: {message}"
    );
    assert!(
        took < Duration::from_secs(2),
        "the deadline is a deadline: {took:?}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn a_bridged_client_has_nowhere_to_redial_and_says_so() {
    // The ssh bridge hands `App::assemble` a socketpair whose far end is a
    // subprocess (`crates/amx/src/remote`), so there is no path to redial and
    // M2's behavior — the loop ends — is the honest one. Pinned here because
    // the reconnect must not turn that into a busy loop against a socket that
    // was never a socket.
    let server = support::Server::start("rcn4").await;
    let pty = support::open_pty();
    let stream = amx_client::net::connect(server.socket())
        .await
        .expect("connect");
    let (session, _welcome) = Session::attach(stream, client_info(), true, None)
        .await
        .expect("negotiate");
    let term = amx_client::term::TerminalGuard::enter(pty.slave, Vec::new()).expect("raw mode");
    let mut app: TestApp = App::assemble(session, term).expect("assemble");

    assert!(!app.can_reconnect());
    let err = app.reconnect().await.expect_err("nowhere to redial");
    assert!(
        err.to_string().contains("no socket to redial"),
        "the reason is the missing path, not a timeout: {err}"
    );

    server.shutdown().await;
}
