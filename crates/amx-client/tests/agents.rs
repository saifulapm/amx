//! X14 acceptance: what the agents board *shows* (D15 surface 2).
//!
//! Asserted against the rasterized frame wherever a rendered board can say it —
//! the rows a user reads, in the order they read them. A view whose internal
//! `Vec` is in the right order and which draws something else is still a bug.
//! What the board *does* — the verbs, the keys and the peek — is
//! `agents_verbs.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
#![allow(dead_code, reason = "the shared harness serves both suites")]

mod support;

#[path = "agents/harness.rs"]
mod harness;

use amx_core::agent::AgentState;
use amx_core::{PaneId, WorkspaceId};
use harness::*;

#[tokio::test]
async fn the_top_row_is_always_whoever_needs_the_user_most() {
    let server = support::Server::start("agents-order").await;
    let pty = support::open_pty();
    let (api, web) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let panes: Vec<PaneId> = (0..5).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(
        &server,
        pty.slave,
        &[(api, &panes[..3]), (web, &panes[3..])],
    )
    .await;

    press(&mut app, &[PREFIX, b'g']);
    // `web` holds the oldest block; `api` holds a newer one plus a working and
    // an idle agent. Inside `api`, blocked sorts above working above idle.
    app.apply_agent_list(reply_of(vec![
        agent(
            (api, "api"),
            panes[0],
            "idler",
            AgentState::Idle,
            Some(NOW - 1_000),
            "$",
        ),
        agent(
            (api, "api"),
            panes[1],
            "tests",
            AgentState::Working,
            Some(NOW - 120_000),
            "running cargo test",
        ),
        agent(
            (api, "api"),
            panes[2],
            "backend",
            AgentState::Blocked,
            Some(NOW - 240_000),
            "Allow Bash(git push)? (y/n)",
        ),
        agent(
            (web, "web"),
            panes[3],
            "frontend",
            AgentState::Blocked,
            Some(NOW - 420_000),
            "Allow Write(a.ts)? (y/n)",
        ),
        agent(
            (web, "web"),
            panes[4],
            "spare",
            AgentState::Idle,
            Some(NOW - 500),
            "$",
        ),
    ]));

    assert_eq!(
        names(&mut app),
        vec![
            "web/frontend",
            "web/spare",
            "api/backend",
            "api/tests",
            "api/idler",
        ],
        "the workspace holding the oldest block comes first, and inside a group \
         it is blocked-oldest-first, then working, then idle",
    );

    let head = list(&mut app).remove(0);
    assert!(
        head.contains("blocked") && head.contains("permission_dialog") && head.contains("7m"),
        "the head names its state, what asserted it and how long it has waited: {head:?}",
    );
    assert!(
        head.contains("Allow Write(a.ts)? (y/n)"),
        "and carries the agent's last screen line as its detail column: {head:?}",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn an_open_board_owns_every_cell_of_the_content_area() {
    let server = support::Server::start("agents-cover").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..2).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    // A short list under a tall screen, which is the case a blank tail hides:
    // the panes are painted before the board, so a row the board does not write
    // shows the pane border it is supposed to be covering.
    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(vec![agent(
        (workspace, "api"),
        panes[0],
        "backend",
        AgentState::Blocked,
        Some(NOW - 1_000),
        "Allow Bash(ls)? (y/n)",
    )]));
    app.repaint();
    assert!(
        written(&app).is_empty(),
        "the board left cells for the panes underneath to show through: {:?}",
        &written(&app)[..written(&app).len().min(8)],
    );

    // And with nothing at all to list, which is what a fresh client sees for
    // its first refresh window.
    app.apply_agent_list(reply_of(Vec::new()));
    app.repaint();
    assert!(
        written(&app).is_empty(),
        "an empty board still owns the screen: {:?}",
        &written(&app)[..written(&app).len().min(8)],
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn more_than_three_idle_agents_collapse_to_one_row_that_enter_expands() {
    let server = support::Server::start("agents-idle").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..5).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    press(&mut app, &[PREFIX, b'g']);
    let mut entries = vec![agent(
        (workspace, "api"),
        panes[0],
        "backend",
        AgentState::Blocked,
        Some(NOW - 60_000),
        "Allow Bash(ls)? (y/n)",
    )];
    for (n, &pane) in panes[1..].iter().enumerate() {
        entries.push(agent(
            (workspace, "api"),
            pane,
            &format!("idle{n}"),
            AgentState::Idle,
            Some(NOW - 1_000),
            "$",
        ));
    }
    app.apply_agent_list(reply_of(entries));

    assert_eq!(
        names(&mut app),
        vec!["api/backend", "4"],
        "four idle agents behind one row, and the blocked one still visible",
    );
    assert!(
        list(&mut app)[1].contains("4 idle"),
        "the collapsed row says how many it stands for",
    );

    // Down onto the collapsed row, then Enter.
    press(&mut app, DOWN);
    press(&mut app, b"\r");
    assert_eq!(
        names(&mut app),
        vec![
            "api/backend",
            "api/idle0",
            "api/idle1",
            "api/idle2",
            "api/idle3",
        ],
        "Enter on the collapsed row expands it",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn ctrl_s_regroups_and_ctrl_b_keeps_only_what_is_waiting() {
    let server = support::Server::start("agents-group").await;
    let pty = support::open_pty();
    let (api, web) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let panes: Vec<PaneId> = (0..4).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(
        &server,
        pty.slave,
        &[(api, &panes[..2]), (web, &panes[2..])],
    )
    .await;

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(vec![
        agent(
            (api, "api"),
            panes[0],
            "blocked",
            AgentState::Blocked,
            Some(NOW - 10_000),
            "?",
        ),
        agent(
            (api, "api"),
            panes[1],
            "busy",
            AgentState::Working,
            Some(NOW - 10_000),
            "…",
        ),
        agent(
            (web, "web"),
            panes[2],
            "blocked",
            AgentState::Blocked,
            Some(NOW - 5_000),
            "?",
        ),
        agent(
            (web, "web"),
            panes[3],
            "busy",
            AgentState::Working,
            Some(NOW - 5_000),
            "…",
        ),
    ]));

    assert_eq!(
        names(&mut app),
        vec!["api/blocked", "api/busy", "web/blocked", "web/busy"],
        "grouped by workspace, the two projects do not interleave",
    );
    assert!(
        board(&mut app)[0].contains("by workspace"),
        "and the header says which grouping is on",
    );

    let calls = press(&mut app, &[CTRL_S]);
    assert!(calls.is_empty(), "grouping is client presentation state");
    assert_eq!(
        names(&mut app),
        vec!["api/blocked", "web/blocked", "api/busy", "web/busy"],
        "grouped by state, the two blocked agents sit together at the top",
    );
    assert!(board(&mut app)[0].contains("by state"));

    let calls = press(&mut app, &[CTRL_B]);
    assert!(calls.is_empty(), "a filter is not a call either");
    assert_eq!(
        names(&mut app),
        vec!["api/blocked", "web/blocked"],
        "ctrl+b is the attention picker: only what is waiting",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn typing_filters_on_the_name_and_has_no_syntax() {
    let server = support::Server::start("agents-filter").await;
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
            AgentState::Working,
            Some(NOW - 1_000),
            "compiling",
        ),
        agent(
            (workspace, "api"),
            panes[1],
            "writer",
            AgentState::Working,
            Some(NOW - 1_000),
            "compiling",
        ),
    ]));

    press(&mut app, b"writ");
    assert_eq!(names(&mut app), vec!["api/writer"], "fuzzy on the name");

    // D15 rejects a filter syntax by name. `s:working` is therefore three
    // characters that match nothing, not a query for the working agents.
    press(&mut app, b"\x7f\x7f\x7f\x7fs:working");
    assert!(
        names(&mut app).is_empty(),
        "`s:working` is text, not a filter expression",
    );

    // And the detail column is not matched: both rows say "compiling", and
    // typing it finds neither.
    press(&mut app, b"\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f");
    assert_eq!(names(&mut app).len(), 2, "the query is empty again");
    press(&mut app, b"compiling");
    assert!(
        names(&mut app).is_empty(),
        "the live detail line is a column, never a haystack",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_filter_the_grouping_and_the_selection_survive_a_close_and_reopen() {
    let server = support::Server::start("agents-keep").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..3).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    let rows = vec![
        agent(
            (workspace, "api"),
            panes[0],
            "alpha",
            AgentState::Working,
            Some(NOW - 1_000),
            "a",
        ),
        agent(
            (workspace, "api"),
            panes[1],
            "beta",
            AgentState::Working,
            Some(NOW - 1_000),
            "b",
        ),
        agent(
            (workspace, "api"),
            panes[2],
            "gamma",
            AgentState::Working,
            Some(NOW - 1_000),
            "c",
        ),
    ];
    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(rows.clone()));
    press(&mut app, &[CTRL_S]);
    press(&mut app, b"a");
    press(&mut app, DOWN);
    let before = names(&mut app);
    assert_eq!(before, vec!["api/alpha", "api/beta", "api/gamma"]);

    let calls = press(&mut app, ESC);
    assert!(!app.agents_open(), "Esc closed it");
    assert!(calls.is_empty(), "and told the server nothing");

    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(rows));
    assert_eq!(names(&mut app), before, "the filter and grouping came back");
    let header = board(&mut app)[0].clone();
    assert!(
        header.contains("by state") && header.contains("· /a"),
        "{header:?}"
    );

    // The selection came back too: `ctrl+x` arms the row it was on.
    press(&mut app, &[CTRL_X]);
    assert!(
        board(&mut app)[0].contains("kill api/beta?"),
        "the selection is remembered by identity: {:?}",
        board(&mut app)[0],
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn a_refresh_moves_the_detail_lines_and_nothing_else() {
    let server = support::Server::start("agents-detail").await;
    let pty = support::open_pty();
    let workspace = WorkspaceId::new_v4();
    let panes: Vec<PaneId> = (0..2).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(&server, pty.slave, &[(workspace, &panes)]).await;

    let rows = |line: &str| {
        vec![
            agent(
                (workspace, "api"),
                panes[0],
                "alpha",
                AgentState::Working,
                Some(NOW - 1_000),
                line,
            ),
            agent(
                (workspace, "api"),
                panes[1],
                "beta",
                AgentState::Working,
                Some(NOW - 1_000),
                "steady",
            ),
        ]
    };
    press(&mut app, &[PREFIX, b'g']);
    app.apply_agent_list(reply_of(rows("compiling amx-core")));
    press(&mut app, b"al");
    press(&mut app, DOWN);
    assert_eq!(names(&mut app), vec!["api/alpha"]);
    assert!(list(&mut app)[0].contains("compiling amx-core"));

    app.apply_agent_list(reply_of(rows("compiling amx-client")));
    assert_eq!(
        names(&mut app),
        vec!["api/alpha"],
        "the query survived the refresh",
    );
    assert!(
        list(&mut app)[0].contains("compiling amx-client"),
        "and the detail line moved with the agent's screen: {:?}",
        list(&mut app)[0],
    );

    drop(app);
    server.shutdown().await;
}
