//! What every test of the agents view is built from.
//!
//! Two suites share it by `#[path]` — `agents.rs` for what the board *shows*
//! and `agents_verbs.rs` for what it *does* — rather than one file holding
//! both, which crossed the hard module budget the day it was written. The rows
//! arrive through [`App::apply_agent_list`] rather than from a real
//! `agent.list` for the reason R-M2-8 has forced since M2: 25 tracked agents
//! need 25 real agent processes, which no runner has. What the *call* does is
//! asserted over the real socket instead, by counting it.
//!
//! `PREFIX` and the control bytes are re-exported from here so both suites spell
//! a key one way.

use amx_client::app::App;
use amx_client::input::InputEvent;
pub use amx_client::input::PREFIX;
use amx_client::model::WorkspaceModel;
use amx_client::term::TermSize;
pub use amx_core::agent::EpochMillis;
use amx_core::agent::{AgentState, AgentWorkspace};
use amx_core::{Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::Call;
use amx_proto::control::agent::{AgentEntry, ListReply};

/// The pty every test here attaches to.
pub const ROWS: u16 = 24;
pub const COLS: u16 = 100;

/// The server's `now` in every reply below.
pub const NOW: EpochMillis = 1_754_650_000_000;

/// `ctrl+b`, `ctrl+p`, `ctrl+r`, `ctrl+s`, `ctrl+x`.
pub const CTRL_B: u8 = 0x02;
pub const CTRL_P: u8 = 0x10;
pub const CTRL_R: u8 = 0x12;
pub const CTRL_S: u8 = 0x13;
pub const CTRL_X: u8 = 0x18;
/// `Esc`, and the two arrows this surface reads.
pub const ESC: &[u8] = b"\x1b";
pub const DOWN: &[u8] = b"\x1b[B";
pub const UP: &[u8] = b"\x1b[A";

pub type TestApp = App<std::fs::File, Vec<u8>>;

pub fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-agents-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// One agent row, as `agent.list` reports one.
pub fn agent(
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
pub fn reply_of(agents: Vec<AgentEntry>) -> ListReply {
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
pub async fn attached(
    server: &crate::support::Server,
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
pub fn press(app: &mut TestApp, bytes: &[u8]) -> Vec<Call> {
    let mut calls = Vec::new();
    app.handle_input(bytes, &mut |event| {
        if let InputEvent::Call(call) = event {
            calls.push(call);
        }
    });
    calls
}

/// The rows of the painted board, trimmed, blank tail dropped.
pub fn board(app: &mut TestApp) -> Vec<String> {
    app.repaint();
    let cells = crate::support::rasterize(app.frame());
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
pub fn written(app: &TestApp) -> Vec<(u16, u16)> {
    let cells = crate::support::rasterize(app.frame());
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
pub fn list(app: &mut TestApp) -> Vec<String> {
    let mut rows = board(app);
    if !rows.is_empty() {
        rows.remove(0);
    }
    rows
}

/// The leading `workspace/name` of each list row.
pub fn names(app: &mut TestApp) -> Vec<String> {
    list(app)
        .iter()
        .map(|row| row.split_whitespace().next().unwrap_or("").to_owned())
        .collect()
}
