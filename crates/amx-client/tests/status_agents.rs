//! X11 acceptance: what the status line says about the agents in the session —
//! D15's per-workspace breakdown of the attention queue, the queue head with
//! how long it has waited, what asserted a blocked pane's state, and D14's
//! compact form below the narrow threshold.
//!
//! Beside `status.rs` rather than inside it: that suite is about the labels,
//! counts and indicators U07 and V14 pinned, this one is about the agent
//! surfaces M4 added, and one file holding both crossed the module budget the
//! day it was written.
//!
//! The assertions are made against rasterized frame bytes rather than against
//! the cached `String`, because the thing a user sees is the row that lands on
//! their terminal: a status line that is built correctly and then truncated,
//! misplaced or drawn over is still a bug. [`status_cells`] reads the same row
//! with its attributes, for the one thing the characters cannot say: which
//! workspace this client is showing is marked by being drawn *out* of the
//! reverse-video bar rather than by a glyph.
//!
//! The age is folded from real deliveries rather than stamped into the model,
//! because `attention_enqueued` is the *only* path by which a live client
//! learns a wall-clock stamp for a block it watched happen: a test that wrote
//! the stamp itself would prove nothing about the surface.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use amx_client::app::App;
use amx_client::config::NarrowCols;
use amx_client::model::WorkspaceModel;
use amx_client::term::TermSize;
use amx_core::agent::{AgentSnapshot, AgentState, EpochMillis, StatusCause};
use amx_core::{Delivery, Direction, Envelope, Event, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::rpc::Notification;

/// The pty every test here attaches to.
const ROWS: u16 = 24;
const COLS: u16 = 80;

/// A wall clock the server could plausibly have stamped a block at.
const SINCE: EpochMillis = 1_754_650_000_000;

/// The attention glyph the line marks a waiting agent with.
const ATTENTION: char = '\u{2691}';

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-status-agents-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// The text of the status row of the last painted frame.
fn status_row(app: &TestApp) -> String {
    row_of(app, ROWS, COLS)
}

/// [`status_row`] for a client of some other size.
fn row_of(app: &TestApp, rows: u16, cols: u16) -> String {
    let cells = support::rasterize(app.frame());
    (0..cols)
        .map(|col| cells.get(&(rows - 1, col)).copied().unwrap_or(' '))
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// The status row with the one attribute the line uses: whether each cell was
/// drawn in the reverse-video bar or out of it.
///
/// A parser of this suite's own rather than an extension of
/// [`support::rasterize`], which reads characters and drops attributes: what is
/// asserted here is a *property of the paint*, and a helper that inferred it
/// from the text would be asserting the test's own idea of where the active
/// workspace is.
fn status_cells(app: &TestApp) -> Vec<(char, bool)> {
    let text = std::str::from_utf8(app.frame()).expect("frame bytes are valid utf-8");
    let mut row = 0_u16;
    let mut cells = Vec::new();
    let mut reverse = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if row == ROWS - 1 {
                cells.push((c, reverse));
            }
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
        }
        let mut params = String::new();
        let mut terminator = '\0';
        for c2 in chars.by_ref() {
            if c2.is_ascii_digit() || c2 == ';' || c2 == '?' {
                params.push(c2);
            } else {
                terminator = c2;
                break;
            }
        }
        match terminator {
            'H' => {
                let head: u16 = params
                    .split(';')
                    .next()
                    .unwrap_or("1")
                    .parse()
                    .unwrap_or(1_u16);
                row = head.saturating_sub(1);
            }
            // `\x1b[0m` resets, `\x1b[7m` turns the bar on; the writer emits
            // the reset first, so reading them in order is reading the state.
            'm' if params == "0" => reverse = false,
            'm' if params == "7" => reverse = true,
            _ => {}
        }
    }
    cells
}

/// Whether each cell of the run covering `needle` was drawn *out* of the bar.
fn emphasis_of(app: &TestApp, needle: &str) -> Vec<bool> {
    let cells = status_cells(app);
    let row: String = cells.iter().map(|(ch, _)| *ch).collect();
    let at = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not on the status row: {row:?}"));
    let start = row[..at].chars().count();
    cells
        .iter()
        .skip(start)
        .take(needle.chars().count())
        .map(|&(_, reverse)| !reverse)
        .collect()
}

/// One blocked agent, stamped the way the hub stamps one.
fn blocked_at(since: EpochMillis, reason: &str) -> AgentSnapshot {
    AgentSnapshot {
        kind: None,
        state: AgentState::Blocked,
        cause: StatusCause::Screen,
        transition_seq: 1,
        attention: None,
        session_ref: None,
        reason: Some(reason.to_owned()),
        since: Some(since),
    }
}

/// Put `pane` on the attention queue with a status the server could have sent.
fn block(app: &mut TestApp, pane: PaneId, since: EpochMillis) {
    app.model()
        .set_pane_agent(pane, Some(blocked_at(since, "permission_dialog")));
    assert!(
        app.model().enqueue_attention(pane),
        "the pane was not queued"
    );
}

/// One delivery, shaped exactly as the server's pump encodes it.
fn published(seq: u64, event: Event) -> Notification {
    Notification::new(
        "event",
        Some(serde_json::to_value(Delivery::Event(Envelope { seq, event })).expect("encode")),
    )
}

/// The `agent_status` a pane's move into `blocked` publishes first.
fn moved(seq: u64, pane: PaneId) -> Notification {
    published(
        seq,
        Event::AgentStatus {
            pane,
            from: Some(AgentState::Working),
            to: AgentState::Blocked,
            cause: StatusCause::Screen,
        },
    )
}

/// The `attention_enqueued` that follows it, carrying the identity block the
/// hub folds off its own bus (D-M4-6) — including the wall-clock stamp.
fn enqueued(seq: u64, pane: PaneId, since: EpochMillis) -> Notification {
    published(
        seq,
        Event::AttentionEnqueued {
            pane,
            workspace: None,
            name: None,
            reason: Some("permission_dialog".to_owned()),
            since: Some(since),
        },
    )
}

/// Attach, and mirror one labelled workspace per entry of `spec` with as many
/// panes as it asks for. The first workspace is the one focused.
async fn app_with_workspaces(
    server: &support::Server,
    pty_slave: std::fs::File,
    spec: &[(&str, usize)],
) -> (TestApp, Vec<Vec<PaneId>>) {
    let mut app = App::attach(server.socket(), pty_slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");
    let mut boards = Vec::new();
    let mut first = None;
    for (label, panes) in spec {
        let mut ids = vec![PaneId::new_v4()];
        let mut layout = BspLayout::with_root(ids[0]);
        for _ in 1..*panes {
            let next = PaneId::new_v4();
            layout
                .split(ids[0], Direction::Right, next, 0.5)
                .expect("split the mirrored layout");
            ids.push(next);
        }
        let workspace = WorkspaceId::new_v4();
        app.adopt_workspace(
            workspace,
            WorkspaceModel {
                label: Some((*label).to_owned()),
                layout,
            },
        );
        first.get_or_insert(workspace);
        boards.push(ids);
    }
    // `adopt_workspace` focuses what it adopts, so the last one would be the
    // active one; every test here reads the first as the one it is looking at.
    if let Some(workspace) = first {
        app.model().focus_workspace(workspace);
    }
    (app, boards)
}

/// Parsed off the rendered row rather than read out of the model, because the
/// property is about what a *user* could see disagreeing.
fn tallies(row: &str) -> (u32, u32) {
    let mut segments = 0;
    let mut rest = row;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let segment = &rest[open + 1..open + close];
        if let Some(mark) = segment.find(ATTENTION) {
            segments += segment[mark + ATTENTION.len_utf8()..]
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
        }
        rest = &rest[open + close + 1..];
    }
    let global = rest
        .find(ATTENTION)
        .and_then(|at| {
            rest[at + ATTENTION.len_utf8()..]
                .split_whitespace()
                .next()?
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(0);
    (segments, global)
}

#[tokio::test]
async fn status_line_breaks_the_attention_queue_down_by_workspace() {
    let server = support::Server::start("brkd").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, boards) =
        app_with_workspaces(&server, pty.slave, &[("api", 2), ("web", 1), ("infra", 1)]).await;

    app.repaint();
    let quiet = status_row(&app);
    for name in ["[api]", "[web]", "[infra]"] {
        assert!(
            quiet.contains(name),
            "a workspace with nothing waiting is named without a count: {quiet:?}",
        );
    }
    assert!(
        !quiet.contains(ATTENTION),
        "an empty queue is worth no indicator anywhere on the line: {quiet:?}",
    );

    block(&mut app, boards[0][0], SINCE);
    block(&mut app, boards[0][1], SINCE + 1_000);
    block(&mut app, boards[1][0], SINCE + 2_000);
    app.repaint();
    let busy = status_row(&app);
    assert!(
        busy.contains("[api ⚑2]") && busy.contains("[web ⚑1]") && busy.contains("[infra]"),
        "each workspace carries its own share of the queue: {busy:?}",
    );
    assert_eq!(
        tallies(&busy),
        (3, 3),
        "the segments and the global count are the same queue: {busy:?}",
    );

    // Both halves move on one dequeue, because both are read off the queue: a
    // per-workspace tally kept beside it could drift here, and this is where it
    // would show.
    assert!(app.model().dequeue_attention(boards[0][1]));
    app.repaint();
    let after = status_row(&app);
    assert!(
        after.contains("[api ⚑1]") && after.contains("[web ⚑1]"),
        "the segment follows the queue down: {after:?}",
    );
    assert_eq!(tallies(&after), (2, 2), "and so does the global count");

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_active_workspace_is_the_segment_drawn_out_of_the_bar() {
    let server = support::Server::start("actv").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, _boards) =
        app_with_workspaces(&server, pty.slave, &[("api", 1), ("web", 1)]).await;
    let second = app
        .model()
        .workspace_ids()
        .collect::<Vec<_>>()
        .into_iter()
        .find(|&id| app.model().workspace_label(id).as_deref() == Some("web"))
        .expect("the second workspace is mirrored");

    app.repaint();
    assert!(
        emphasis_of(&app, "[api]").iter().all(|&out| out),
        "the workspace this client is showing is drawn out of the reverse bar",
    );
    assert!(
        emphasis_of(&app, "[web]").iter().all(|&out| !out),
        "and every other workspace is drawn in it",
    );

    // Distinguishable means it *moves*: a mark that sat on the first workspace
    // whatever the focus would pass the assertions above and say nothing.
    app.model().focus_workspace(second);
    app.repaint();
    assert!(emphasis_of(&app, "[web]").iter().all(|&out| out));
    assert!(emphasis_of(&app, "[api]").iter().all(|&out| !out));

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_queue_head_is_named_and_aged_against_the_servers_clock() {
    let server = support::Server::start("head").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, boards) =
        app_with_workspaces(&server, pty.slave, &[("api", 1), ("web", 1)]).await;
    let head = boards[0][0];
    let later = boards[1][0];
    app.model().set_pane_label(head, Some("backend".to_owned()));

    // The hub's own order, folded from real deliveries: the status transition
    // first, then the enqueue that carries the wall-clock stamp. Nothing else
    // gives a live client that stamp.
    app.apply_notification(&moved(1, head));
    app.apply_notification(&enqueued(2, head, SINCE));
    app.repaint();
    let queued = status_row(&app);
    assert!(
        queued.contains("api/backend"),
        "the head of the queue is where next-attention would go: {queued:?}",
    );

    // The age is `now − since` inside the server's own clock. A renderer using
    // *this* machine's clock would read the difference between today and the
    // stamp — days, not minutes — so this assertion is also what proves no
    // local wall clock is involved (D-M4-4).
    app.note_server_clock(SINCE + 240_000);
    app.repaint();
    let aged = status_row(&app);
    assert!(
        aged.contains("api/backend 4m"),
        "the head carries how long it has waited: {aged:?}",
    );

    // And it advances on what the session pushes, with no second call: a block
    // elsewhere at a later instant moves this client's estimate of the server's
    // clock, and every age with it.
    app.apply_notification(&moved(3, later));
    app.apply_notification(&enqueued(4, later, SINCE + 600_000));
    app.repaint();
    let advanced = status_row(&app);
    assert!(
        advanced.contains("api/backend 10m"),
        "the head's age advances between refreshes: {advanced:?}",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn a_blocked_focused_pane_names_what_asserted_it_and_drops_it_on_the_next_move() {
    let server = support::Server::start("rsn").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, boards) = app_with_workspaces(&server, pty.slave, &[("work", 1)]).await;
    let pane = boards[0][0];
    assert_eq!(app.focused_pane(), Some(pane));

    app.model()
        .set_pane_agent(pane, Some(blocked_at(SINCE, "permission_dialog")));
    app.repaint();
    let waiting = status_row(&app);
    assert!(
        waiting.contains("blocked (permission_dialog)"),
        "the detector's own name for what asserted the block: {waiting:?}",
    );

    // A move the client hears as `agent_status` carries neither a reason nor a
    // stamp, so the previous state's must not survive it: an idle pane
    // captioned `permission_dialog` would be one edge's name read onto another.
    app.model()
        .apply_agent_status(pane, AgentState::Idle, StatusCause::Screen, 9);
    app.repaint();
    let idle = status_row(&app);
    assert!(
        idle.contains("idle") && !idle.contains("permission_dialog"),
        "the reason describes one edge and goes with it: {idle:?}",
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn the_line_takes_the_compact_form_below_the_narrow_threshold_and_back() {
    let server = support::Server::start("nrw").await;
    let pty = support::open_pty_sized(ROWS, COLS);
    let (mut app, boards) =
        app_with_workspaces(&server, pty.slave, &[("api", 1), ("web", 1)]).await;
    app.model()
        .set_pane_label(boards[0][0], Some("backend".to_owned()));
    block(&mut app, boards[0][0], SINCE);

    app.repaint();
    assert!(
        status_row(&app).contains("[web]"),
        "a wide client shows every workspace",
    );

    // D14: below the threshold the breakdown is the first thing to go — it
    // needs width a phone does not have — and what is left is the workspace
    // this client is in, the global count and the queue head.
    let narrow = TermSize { rows: 20, cols: 45 };
    app.note_resize(narrow);
    assert!(app.settle_resize(&mut |_| {}));
    let compact = row_of(&app, narrow.rows, narrow.cols);
    assert!(
        compact.contains("api") && !compact.contains('['),
        "the compact form drops the breakdown: {compact:?}",
    );
    assert!(
        compact.contains("⚑1") && compact.contains("api/backend"),
        "and keeps the count and the head: {compact:?}",
    );

    // Crossing back is a rendering policy and nothing else: the same state
    // renders the full line again.
    app.note_resize(TermSize {
        rows: ROWS,
        cols: COLS,
    });
    assert!(app.settle_resize(&mut |_| {}));
    assert!(
        status_row(&app).contains("[web]"),
        "and back again: {:?}",
        status_row(&app),
    );

    // The threshold is configuration, not a constant: a client told that
    // anything under 100 columns is narrow renders the compact form at 80.
    app.set_narrow_cols(NarrowCols(COLS + 20));
    app.repaint();
    assert!(
        !status_row(&app).contains('['),
        "the configured threshold is what decides: {:?}",
        status_row(&app),
    );

    drop(app);
    server.shutdown().await;
}
