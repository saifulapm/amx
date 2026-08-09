//! X14 acceptance: the agents view (D15 surface 2).
//!
//! What is asserted is the *rendered board* wherever a rendered board can say
//! it — the rows a user reads, in the order they read them — and the emitted
//! calls wherever the claim is about what left the client. A view whose internal
//! `Vec` is in the right order and which draws something else is still a bug,
//! and a `ctrl+x` that fires on the first press is invisible on screen.
//!
//! The rows arrive through [`App::apply_agent_list`] rather than from a real
//! `agent.list` for the reason R-M2-8 has forced since M2: 25 tracked agents
//! need 25 real agent processes, which no runner has. What the *call* does is
//! asserted separately, over the real socket, by counting it
//! ([`App::agent_lists`]) — which is the half R-M4-7 is actually about.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::path::Path;
use std::time::Duration;

use amx_client::app::App;
use amx_client::config::NarrowCols;
use amx_client::input::{InputEvent, PREFIX};
use amx_client::model::WorkspaceModel;
use amx_client::net::{self, Session};
use amx_client::term::TermSize;
use amx_core::agent::{AgentState, AgentWorkspace, EpochMillis};
use amx_core::{Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::Call;
use amx_proto::control::agent::{AgentEntry, ListReply};

/// The pty every test here attaches to.
const ROWS: u16 = 24;
const COLS: u16 = 100;

/// The server's `now` in every reply below.
const NOW: EpochMillis = 1_754_650_000_000;

/// `ctrl+b`, `ctrl+p`, `ctrl+r`, `ctrl+s`, `ctrl+x`.
const CTRL_B: u8 = 0x02;
const CTRL_P: u8 = 0x10;
const CTRL_R: u8 = 0x12;
const CTRL_S: u8 = 0x13;
const CTRL_X: u8 = 0x18;
/// `Esc`, and the two arrows this surface reads.
const ESC: &[u8] = b"\x1b";
const DOWN: &[u8] = b"\x1b[B";
const UP: &[u8] = b"\x1b[A";

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-agents-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// One agent row, as `agent.list` reports one.
fn agent(
    workspace: (WorkspaceId, &str),
    pane: PaneId,
    name: &str,
    status: AgentState,
    since: Option<EpochMillis>,
    last_line: &str,
) -> AgentEntry {
    AgentEntry {
        workspace: AgentWorkspace {
            id: workspace.0,
            name: Some(workspace.1.to_owned()),
        },
        pane,
        name: Some(name.to_owned()),
        kind: None,
        status,
        reason: (status == AgentState::Blocked).then(|| "permission_dialog".to_owned()),
        since,
        last_line: last_line.to_owned(),
    }
}

/// A reply carrying `agents`, with the blocked ones queued oldest first.
fn reply_of(agents: Vec<AgentEntry>) -> ListReply {
    let mut queue: Vec<&AgentEntry> = agents
        .iter()
        .filter(|entry| entry.status == AgentState::Blocked)
        .collect();
    queue.sort_by_key(|entry| entry.since.unwrap_or(EpochMillis::MAX));
    let attention = queue.iter().map(|entry| entry.pane).collect();
    ListReply {
        seq: 1,
        now: NOW,
        attention,
        agents,
    }
}

/// Attach a client and mirror one workspace per entry of `spec`, each holding
/// the panes named for it — so the board's `Enter` has somewhere to jump to.
async fn attached(
    server: &support::Server,
    pty_slave: std::fs::File,
    spec: &[(WorkspaceId, &[PaneId])],
) -> TestApp {
    let mut app = App::attach(server.socket(), pty_slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");
    app.note_resize(TermSize {
        rows: ROWS,
        cols: COLS,
    });
    app.settle_resize(&mut |_| {});
    for &(workspace, panes) in spec {
        let mut layout = BspLayout::with_root(panes[0]);
        for &pane in &panes[1..] {
            layout
                .split(
                    *layout.panes().last().expect("a pane to split"),
                    amx_core::Direction::Down,
                    pane,
                    0.5,
                )
                .expect("split");
        }
        app.adopt_workspace(
            workspace,
            WorkspaceModel {
                label: Some("ws".to_owned()),
                layout,
            },
        );
    }
    app
}

/// Feed the board a read of stdin, collecting whatever left the client.
fn press(app: &mut TestApp, bytes: &[u8]) -> Vec<Call> {
    let mut calls = Vec::new();
    app.handle_input(bytes, &mut |event| {
        if let InputEvent::Call(call) = event {
            calls.push(call);
        }
    });
    calls
}

/// The rows of the painted board, trimmed, blank tail dropped.
fn board(app: &mut TestApp) -> Vec<String> {
    app.repaint();
    let cells = support::rasterize(app.frame());
    let mut rows: Vec<String> = (0..ROWS - 1)
        .map(|row| {
            (0..COLS)
                .map(|col| cells.get(&(row, col)).copied().unwrap_or(' '))
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

/// Which cells of the content area the last frame actually wrote.
///
/// Distinct from [`board`], which reads an unwritten cell as a space: on a real
/// terminal an unwritten cell is not a space but whatever was under it, so a
/// board with fewer rows than the screen would show the panes it is covering
/// through its own blank tail. Only the written set can tell the two apart.
fn written(app: &TestApp) -> Vec<(u16, u16)> {
    let cells = support::rasterize(app.frame());
    let mut missing = Vec::new();
    for row in 0..ROWS - 1 {
        for col in 0..COLS {
            if !cells.contains_key(&(row, col)) {
                missing.push((row, col));
            }
        }
    }
    missing
}

/// The board's rows with the header dropped: what the list itself says.
fn list(app: &mut TestApp) -> Vec<String> {
    let mut rows = board(app);
    if !rows.is_empty() {
        rows.remove(0);
    }
    rows
}

/// The leading `workspace/name` of each list row.
fn names(app: &mut TestApp) -> Vec<String> {
    list(app)
        .iter()
        .map(|row| row.split_whitespace().next().unwrap_or("").to_owned())
        .collect()
}

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
