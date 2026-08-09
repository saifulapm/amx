//! X14 acceptance: what the agents board *does*.
//!
//! The other half of `agents.rs`: the calls that leave the client, the two
//! prefix keys that reach the surface, the refresh rate R-M4-7 bounds, and seam
//! 5's join with X15's peek. Asserted on the emitted `Call`s and on the real
//! socket, because none of it is visible in a frame.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
#![allow(dead_code, reason = "the shared harness serves both suites")]

mod support;

#[path = "agents/harness.rs"]
mod harness;

use std::path::Path;
use std::time::Duration;

use amx_client::app::App;
use amx_client::config::NarrowCols;
use amx_client::net::{self, Session};
use amx_core::agent::AgentState;
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::Call;
use harness::*;

#[tokio::test]
async fn ctrl_p_prompts_the_selected_agent_and_ctrl_r_renames_it() {
    let server = support::Server::start("agents-verbs").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..2).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(vec![
        agent(
            (workspace, "api"),
            panes[0],
            "backend",
            AgentState::Blocked,
            Some(NOW - 1_000),
            "Allow Bash(ls)? (y/n)",
        ),
        agent(
            (workspace, "api"),
            panes[1],
            "writer",
            AgentState::Idle,
            Some(NOW - 1_000),
            "$",
        ),
    ]));

    let calls = press(&mut app, &[CTRL_P]);
    assert!(calls.is_empty(), "opening the line sends nothing");
    assert!(
        board(&mut app)[0].contains("prompt api/backend>"),
        "the line names the agent it is about to reach: {:?}",
        board(&mut app)[0],
    );
    press(&mut app, b"yes please");
    let calls = press(&mut app, b"\r");
    match calls.as_slice() {
        [Call::AgentPrompt(params)] => {
            assert_eq!(params.target.to_string(), panes[0].to_string());
            assert_eq!(params.text, "yes please", "spaces reach the prompt");
            assert!(
                params.wait.is_none(),
                "prompting from the board must not wait on the agent",
            );
        }
        other => panic!("one agent.prompt and nothing else: {other:?}"),
    }
    assert!(
        app.agents_open(),
        "prompting does not attach and does not close the board",
    );

    // Rename opens prefilled with the name it is replacing.
    press(&mut app, &[CTRL_R]);
    assert!(board(&mut app)[0].contains("rename api/backend> backend"));
    let calls = press(&mut app, b"\x7f\x7f\x7f\x7f\x7f\x7f\x7fapi2\r");
    match calls.as_slice() {
        [Call::PaneRename(params)] => {
            assert_eq!(params.pane, panes[0]);
            assert_eq!(params.label, "api2");
        }
        other => panic!("one pane.rename and nothing else: {other:?}"),
    }

    // And Esc out of an entry keeps the board.
    press(&mut app, &[CTRL_P]);
    let calls = press(&mut app, ESC);
    assert!(calls.is_empty(), "a cancelled prompt sends nothing");
    assert!(app.agents_open(), "and leaves the board up");

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn ctrl_x_kills_only_on_the_second_press() {
    let server = support::Server::start("agents-kill").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..2).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(vec![
        agent(
            (workspace, "api"),
            panes[0],
            "backend",
            AgentState::Blocked,
            Some(NOW - 1_000),
            "?",
        ),
        agent(
            (workspace, "api"),
            panes[1],
            "writer",
            AgentState::Idle,
            Some(NOW - 1_000),
            "$",
        ),
    ]));

    let calls = press(&mut app, &[CTRL_X]);
    assert!(calls.is_empty(), "the first press asks, it does not kill");
    assert!(
        board(&mut app)[0].contains("kill api/backend?"),
        "and says so on the header rather than in a dialog: {:?}",
        board(&mut app)[0],
    );

    let calls = press(&mut app, &[CTRL_X]);
    match calls.as_slice() {
        [Call::PaneClose(params)] => assert_eq!(params.pane, panes[0]),
        other => panic!("the second press closes the pane: {other:?}"),
    }

    // Anything in between disarms: a first press, a keystroke, a second press.
    let calls = press(&mut app, &[CTRL_X]);
    assert!(calls.is_empty());
    press(&mut app, DOWN);
    let calls = press(&mut app, &[CTRL_X]);
    assert!(
        calls.is_empty(),
        "moving the selection between the two presses must not kill anything",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn enter_jumps_to_the_agents_pane_and_closes_the_board() {
    let server = support::Server::start("agents-jump").await;
    let pty = support::open_pty();
    let (api, web) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let panes: Vec<PaneId> = (0..2).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(
        &server,
        pty.slave,
        &[(api, &panes[..1]), (web, &panes[1..])],
    )
    .await;
    app.model().focus_workspace(api);

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(vec![agent(
        (web, "web"),
        panes[1],
        "frontend",
        AgentState::Blocked,
        Some(NOW - 1_000),
        "?",
    )]));

    let calls = press(&mut app, b"\r");
    match calls.as_slice() {
        [Call::WorkspaceSwitch(params)] => assert_eq!(params.workspace, web),
        other => panic!("the jump echoes the workspace it moved to: {other:?}"),
    }
    assert!(!app.agents_open(), "and the board is gone");
    assert_eq!(app.model().focused_workspace_id(), Some(web));
    assert_eq!(
        app.focused_pane(),
        Some(panes[1]),
        "input now addresses the agent that was selected",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_board_refreshes_from_one_agent_list_per_window() {
    let server = support::Server::start("agents-rate").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..3).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    assert_eq!(app.agent_lists(), 0, "a closed board asks for nothing");
    app.refresh_agents()
        .await
        .expect("a closed board is a no-op");
    assert_eq!(app.agent_lists(), 0);

    press(&mut app, &[PREFIX, b'g']);
    // Thirty refreshes inside one window, over the real socket, with three panes
    // in the session: R-M4-7's whole point is that this is one call and not one
    // per pane, nor one per invitation to refresh.
    for _ in 0..30 {
        app.refresh_agents().await.expect("refresh");
    }
    assert_eq!(
        app.agent_lists(),
        1,
        "one agent.list per window, whatever asks and however many panes there are",
    );
    // That the window *elapses* is the other half, and it is a clock question
    // rather than a socket one: `AgentsUi::due`'s own inline test in
    // `app/agents/mod.rs` walks it with instants it constructs, so this suite
    // does not have to nap for a quarter second to watch a counter move.

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn space_hands_the_selected_pane_to_the_peek_and_the_board_keeps_off_it() {
    let server = support::Server::start("agents-peek").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..3).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(
        // Distinct block times, so the row order is the pane order and a
        // selection move is unambiguous about which pane it landed on.
        ["backend", "writer", "tests"]
            .iter()
            .zip(&panes)
            .enumerate()
            .map(|(n, (name, &pane))| {
                agent(
                    (workspace, "api"),
                    pane,
                    name,
                    AgentState::Blocked,
                    Some(NOW - 60_000 + (n as EpochMillis) * 1_000),
                    "Allow Bash(ls)? (y/n)",
                )
            })
            .collect(),
    ));
    assert_eq!(app.peeked(), None, "nothing is peeked unasked");
    assert_eq!(
        app.peek_layout().peek,
        None,
        "and the list has the whole content area",
    );

    // `Space` names the selected pane; X15's `open_peek` is what binds it.
    press(&mut app, b" ");
    app.settle_peek().await.expect("open the peek");
    assert_eq!(app.peeked(), Some(panes[0]), "the row the cursor was on");
    let layout = app.peek_layout();
    let region = layout.peek.expect("the region is open");
    assert!(region.h > 0 && layout.list.h > 0, "half each: {layout:?}");
    assert_eq!(
        layout.list.h + region.h,
        ROWS - 1,
        "the two exactly tile the content area",
    );

    // The board draws its list and stops at the seam: every row from the peek's
    // first is the peek's to paint, and none of them carries an agent's name.
    let drawn = board(&mut app);
    for (n, row) in drawn.iter().enumerate() {
        if n < usize::from(region.y) {
            continue;
        }
        assert!(
            !row.contains("api/"),
            "row {n} is inside the peek region and the board drew in it: {row:?}",
        );
    }

    // The rebind *is* the move: the selection moving under an open peek moves
    // what is peeked with it.
    press(&mut app, DOWN);
    app.settle_peek().await.expect("rebind");
    assert_eq!(app.peeked(), Some(panes[1]));

    // Under D14's narrow policy the peek replaces the list rather than sharing.
    app.set_narrow_cols(NarrowCols(COLS + 1));
    let narrow = app.peek_layout();
    assert_eq!(narrow.list.h, 0, "no list, not even a header: {narrow:?}");
    assert!(narrow.peek.is_some());
    assert!(
        board(&mut app).is_empty(),
        "and the board paints nothing at all",
    );

    // Esc closes the peek first and the board second.
    app.set_narrow_cols(NarrowCols::default());
    press(&mut app, ESC);
    app.settle_peek().await.expect("close the peek");
    assert_eq!(app.peeked(), None, "the peek closed");
    assert!(app.agents_open(), "and the board did not");
    press(&mut app, ESC);
    assert!(!app.agents_open());

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn prefix_a_cycles_the_whole_queue_and_prefix_shift_a_cycles_this_project() {
    let server = support::Server::start("agents-scope").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let pane = PaneId::new_v4();
    let mut app = attached(&server, pty.slave, &[(workspace, &[pane])]).await;

    match press(&mut app, &[PREFIX, b'a']).as_slice() {
        [Call::AgentNext(params)] => assert_eq!(
            params.workspace, None,
            "the unscoped key walks the global queue",
        ),
        other => panic!("one agent.next: {other:?}"),
    }
    match press(&mut app, &[PREFIX, b'A']).as_slice() {
        [Call::AgentNext(params)] => assert_eq!(
            params.workspace,
            Some(workspace),
            "the neighbouring key scopes it to the workspace this client shows",
        ),
        other => panic!("one agent.next: {other:?}"),
    }

    drop(app);
    server.shutdown().await;
}

/// How long the join below waits for the session to send what it asked for.
const DEADLINE: Duration = Duration::from_secs(10);

/// A connection of its own, for the calls this test makes on the session rather
/// than through the client under test.
async fn side_channel(socket: &Path) -> Session {
    let stream = net::connect(socket).await.expect("a second connection");
    let (session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .expect("negotiate the second connection");
    session
}

/// Seam 5's join, end to end: the board lists an agent in a workspace this
/// terminal is not drawing, `Space` peeks it, and that pane's own cells arrive
/// at a client whose viewport never named it.
///
/// The two surfaces are pinned separately — this suite for the board, X15's
/// `peek.rs` for the stream — and this is the one place they meet: the key that
/// names the pane, the seam that turns it into a bind, and the cells that come
/// back. It is here rather than in `peek.rs` because the *key* is the half that
/// was missing, and a test that called `open_peek` directly would prove the half
/// that already had a test.
#[tokio::test]
async fn space_on_a_row_brings_that_panes_own_cells_to_this_client() {
    let server = support::Server::start("agents-join").await;
    let pty = support::open_pty();
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");

    // A workspace this client is not showing, with a live root pane in it.
    let mut side = side_channel(server.socket()).await;
    let made = side
        .call(
            "workspace.create",
            serde_json::json!({ "label": "elsewhere" }),
        )
        .await
        .expect("workspace.create");
    let elsewhere: WorkspaceId =
        serde_json::from_value(made["workspace"].clone()).expect("the workspace id");
    app.resync_state().await.expect("fold the new workspace");
    let watched = app
        .model()
        .workspace(elsewhere)
        .expect("the workspace is mirrored")
        .layout
        .panes()[0];
    assert_ne!(
        app.model().focused_workspace_id(),
        Some(elsewhere),
        "the peeked pane must be in a workspace this terminal is not drawing",
    );
    assert!(
        app.model().pane(watched).is_none(),
        "and one this client holds no cells for",
    );

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(vec![agent(
        (elsewhere, "elsewhere"),
        watched,
        "backend",
        AgentState::Blocked,
        Some(NOW - 1_000),
        "Allow Bash(ls)? (y/n)",
    )]));
    press(&mut app, b" ");
    app.settle_peek().await.expect("open the peek");
    assert_eq!(app.peeked(), Some(watched));

    let arrived = tokio::time::timeout(DEADLINE, async {
        while app.model().pane(watched).is_none() {
            app.step_frame().await.expect("read one server frame");
        }
    })
    .await;
    assert!(
        arrived.is_ok(),
        "the peeked pane never sent its cells to the client that bound it",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_arrows_move_the_selection_and_a_wheel_turn_does_not_close_the_board() {
    let server = support::Server::start("agents-keys").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..3).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(
        ["alpha", "beta", "gamma"]
            .iter()
            .zip(&panes)
            .map(|(name, &pane)| {
                agent(
                    (workspace, "api"),
                    pane,
                    name,
                    AgentState::Working,
                    Some(NOW - 1_000),
                    "…",
                )
            })
            .collect(),
    ));

    press(&mut app, DOWN);
    press(&mut app, DOWN);
    press(&mut app, UP);
    press(&mut app, &[CTRL_X]);
    assert!(
        board(&mut app)[0].contains("kill api/beta?"),
        "two down and one up is the second row: {:?}",
        board(&mut app)[0],
    );

    // X13's F-C, on this surface: a report's bytes are `ESC [ < … M`, which this
    // table would otherwise read as close-the-board followed by junk.
    press(&mut app, b"\x1b[<64;10;5M");
    assert!(app.agents_open(), "a wheel turn is not an Esc");
    assert!(
        board(&mut app)[0].contains("kill api/beta?"),
        "and moves nothing: {:?}",
        board(&mut app)[0],
    );

    drop(app);
    server.shutdown().await;
}
