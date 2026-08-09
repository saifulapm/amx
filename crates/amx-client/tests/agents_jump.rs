//! Where the agents board's `Enter` lands, and what may move it afterwards.
//!
//! The third suite over `agents/harness.rs`, and the one that needs the wire:
//! `agents_verbs.rs` asserts that the jump *emits* a `workspace.switch`, which
//! is what a client's own input can be asked about, and that is exactly the
//! half that was green while the verb did not work. `workspace.switch` carries
//! no pane, so the session answers it by restating whichever pane the workspace
//! it switched to was already remembering — and until the fix this suite covers,
//! that restatement was folded over the pane the user had picked. The M4 exit
//! smoke found it three times from outside the process
//! (`docs/notes/m4-live-smoke.md` §5.5, §6.4); nothing short of a real switch
//! against a real session can show it, so the first test makes one.
//!
//! The second test is the other half of the same rule: a focus the session
//! actually *moved* — `agent.next`, another client's `pane.focus`, a restore —
//! outranks the jump, and so does this terminal moving its own focus. A fix that
//! pinned the jump against everything would break "handle the next one" instead.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
#![allow(dead_code, reason = "the shared harness serves three suites")]

mod support;

#[path = "agents/harness.rs"]
mod harness;

use std::path::Path;
use std::time::Duration;

use amx_client::app::App;
use amx_client::net::{self, Session};
use amx_client::term::TermSize;
use amx_core::agent::AgentState;
use amx_core::{Delivery, Envelope, Event, PaneId, WorkspaceId};
use amx_proto::control::session;
use amx_proto::rpc::Notification;
use harness::*;
use serde_json::json;

/// How long a test waits for the client to fold something before calling it a
/// failure. Generous, and never on the green path.
const DEADLINE: Duration = Duration::from_secs(10);

/// A connection of its own, for the calls a test makes on the session rather
/// than through the client under test.
async fn side_channel(socket: &Path) -> Session {
    let stream = net::connect(socket).await.expect("a second connection");
    let (session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .expect("negotiate the second connection");
    session
}

/// One event as the server's pump encodes it.
fn published(seq: u64, event: Event) -> Notification {
    Notification::new(
        "event",
        Some(
            serde_json::to_value(Delivery::Event(Envelope { seq, event }))
                .expect("encode a delivery"),
        ),
    )
}

/// Step the wire until `done` holds, failing on the deadline.
async fn pump_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) {
    let stepped = tokio::time::timeout(DEADLINE, async {
        while !done(app) {
            app.step_frame().await.expect("read one server frame");
        }
    })
    .await;
    assert!(
        stepped.is_ok(),
        "the client never folded what the session published"
    );
}

/// D15's jump, over a real socket: the board lands on the agent that was
/// selected and not on the pane its workspace happened to remember.
///
/// Every part of the mechanism the smoke recorded is the shipped one — the real
/// `workspace.switch`, the `FocusChanged` the server publishes before replying
/// to it, and the `session.state` the client re-reads after it. Only the rows
/// are injected, for the reason the harness gives (R-M2-8).
#[tokio::test]
async fn enter_lands_on_the_agent_and_not_the_pane_its_workspace_remembers() {
    let server = support::Server::start("agents-land").await;
    let pty = support::open_pty();
    let mut app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");
    app.note_resize(TermSize {
        rows: ROWS,
        cols: COLS,
    });
    app.settle_resize(&mut |_| {});
    let here = app
        .model()
        .focused_workspace_id()
        .expect("the attach seeded a workspace");
    let rooted_here = app
        .model()
        .workspace(here)
        .expect("the seeded workspace is mirrored")
        .layout
        .panes()[0];

    // A second workspace with two live panes, made from another connection —
    // the way `amx agent start` in another terminal makes one. The split is
    // what gives that workspace a remembered focus of its own: the server
    // focuses the pane a split mints (`state/session.rs`).
    let mut side = side_channel(server.socket()).await;
    let made = side
        .call("workspace.create", json!({ "label": "exp" }))
        .await
        .expect("workspace.create");
    let exp: WorkspaceId =
        serde_json::from_value(made["workspace"].clone()).expect("the workspace id");
    app.resync_state().await.expect("fold the new workspace");
    let agent_pane = app
        .model()
        .workspace(exp)
        .expect("the new workspace is mirrored")
        .layout
        .panes()[0];
    let split = side
        .call(
            "pane.split",
            json!({
                "pane": agent_pane,
                "direction": "vertical",
                "command": ["/bin/sh", "-c", "exec cat"],
            }),
        )
        .await
        .expect("pane.split");
    let remembered: PaneId = serde_json::from_value(split["pane"].clone()).expect("the new pane");
    app.resync_state().await.expect("fold the split");

    // The premise, out of the server's own mouth rather than assumed: `exp`
    // remembers the pane the split focused, and the agent is in the other one.
    let state: session::StateReply = serde_json::from_value(
        side.call("session.state", json!({}))
            .await
            .expect("session.state"),
    )
    .expect("decode session.state");
    assert_eq!(
        state
            .workspaces
            .iter()
            .find(|ws| ws.workspace == exp)
            .and_then(|ws| ws.focus),
        Some(remembered),
        "the session remembers the split pane for the workspace being jumped into",
    );
    assert_ne!(remembered, agent_pane, "and it is not the agent's pane");
    assert_eq!(
        app.model().focused_workspace_id(),
        Some(here),
        "this terminal is still showing the workspace it attached to",
    );

    // The board, over the agent in the workspace this terminal is not showing.
    app.handle_bytes(&[PREFIX, b'g'])
        .await
        .expect("open the board");
    app.apply_agent_list(reply_of(vec![agent(
        (exp, "exp"),
        agent_pane,
        "exp-3",
        AgentState::Blocked,
        Some(NOW - 1_000),
        "Allow Bash(ls)? (y/n)",
    )]));

    // `Enter` through the wired loop: the switch really goes out, its reply
    // really comes back, and `mutates_layout` really re-reads state.
    app.handle_bytes(b"\r").await.expect("jump to the agent");
    assert!(!app.agents_open(), "the board is gone");
    assert_eq!(
        app.model().focused_workspace_id(),
        Some(exp),
        "and this terminal is showing the agent's workspace",
    );

    // The snapshot half is folded by now — `mutates_layout` re-read state
    // inside the call above — and the event half is still on the wire. The
    // switch publishes its `FocusChanged` before it replies, so a *later*
    // transition folded here proves the earlier one was folded first: the
    // stream is ordered and gapless (04 §2). A split in the workspace this
    // terminal has left is that transition — it is real session state rather
    // than something a resync could undo, and its own focus move is in a
    // workspace the jump makes no claim on. Waiting on it rather than napping
    // keeps the proof a fact.
    side.call(
        "pane.split",
        json!({
            "pane": rooted_here,
            "direction": "vertical",
            "command": ["/bin/sh", "-c", "exec cat"],
        }),
    )
    .await
    .expect("split the workspace this terminal left");
    pump_until(&mut app, |app| {
        app.model()
            .workspace(here)
            .is_some_and(|ws| ws.layout.panes().len() == 2)
    })
    .await;

    assert_eq!(
        app.focused_pane(),
        Some(agent_pane),
        "input addresses the agent that was selected, not the pane `exp` remembered",
    );

    drop(app);
    server.shutdown().await;
}

/// The other half of the precedence: the jump outranks the *restatement* the
/// switch is answered with, and nothing else.
///
/// Three folds, in the order a user produces them. The second is what makes
/// `prefix+a` still work after a jump — `agent.next` moves the session's focus
/// and says so with the same event the switch does, so a fix that refused the
/// event by workspace would have broken "handle the next one" to fix the board.
#[tokio::test]
async fn a_focus_the_session_moved_outranks_the_jump_but_its_own_echo_does_not() {
    let server = support::Server::start("agents-rank").await;
    let pty = support::open_pty();
    let (here, exp) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let panes: Vec<PaneId> = (0..4).map(|_| PaneId::new_v4()).collect();
    let mut app = attached(
        &server,
        pty.slave,
        &[(here, &panes[..1]), (exp, &panes[1..])],
    )
    .await;
    app.model().focus_workspace(here);
    // `exp` is three panes stacked, in that order: the agent's, the one below
    // it, and one more.
    let (target, echo, moved) = (panes[1], panes[2], panes[3]);

    // What this client knows about `exp` before it jumps: the session focuses
    // `echo` there. This is the pane the switch will be answered with.
    app.apply_notification(&published(
        1,
        Event::FocusChanged {
            workspace: exp,
            pane: Some(echo),
        },
    ));

    let jump = |app: &mut TestApp| {
        press(app, &[PREFIX, b'g']);
        app.apply_agent_list(reply_of(vec![agent(
            (exp, "exp"),
            target,
            "exp-1",
            AgentState::Blocked,
            Some(NOW - 1_000),
            "?",
        )]));
        press(app, b"\r");
    };
    jump(&mut app);
    assert_eq!(app.focused_pane(), Some(target), "the jump landed");

    // 1. The switch's own answer, restating what this client already knew.
    app.apply_notification(&published(
        2,
        Event::FocusChanged {
            workspace: exp,
            pane: Some(echo),
        },
    ));
    assert_eq!(
        app.focused_pane(),
        Some(target),
        "the restatement carries no news and must not move the jump",
    );

    // 2. A focus the session really moved — `agent.next` reaching the head of
    //    the queue, or another client's `pane.focus`. News, and followed.
    app.apply_notification(&published(
        3,
        Event::FocusChanged {
            workspace: exp,
            pane: Some(moved),
        },
    ));
    assert_eq!(
        app.focused_pane(),
        Some(moved),
        "a focus the session moved outranks the jump",
    );

    // 3. A jump made while this terminal is *already* showing that workspace is
    //    the same claim — §5.5 saw the defect both ways round — and its echo is
    //    now whatever fold 2 left behind.
    jump(&mut app);
    assert_eq!(app.focused_pane(), Some(target), "the second jump landed");
    app.apply_notification(&published(
        4,
        Event::FocusChanged {
            workspace: exp,
            pane: Some(moved),
        },
    ));
    assert_eq!(
        app.focused_pane(),
        Some(target),
        "a jump inside the workspace already being shown holds too",
    );

    // 4. And a jump this terminal has itself moved on from claims nothing:
    //    `prefix+w j` onto the pane below the agent's, after which the same
    //    echo is folded like any other report.
    press(&mut app, &[PREFIX, b'w', b'j']);
    assert_eq!(
        app.focused_pane(),
        Some(echo),
        "the pane below the agent's in the mirrored layout",
    );
    app.apply_notification(&published(
        5,
        Event::FocusChanged {
            workspace: exp,
            pane: Some(moved),
        },
    ));
    assert_eq!(
        app.focused_pane(),
        Some(moved),
        "the jump expired when this terminal chose again",
    );

    drop(app);
    server.shutdown().await;
}
