//! T17 acceptance: `amx` just works (04 §1).
//!
//! Every test here runs the real binary against a real socket, and the ones
//! that attach run it against a real pseudoterminal — the properties under test
//! are process properties, and a test that stubbed the process would be testing
//! its own stub.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::time::Duration;

use amx_server::session::probe::{Probe, probe, server_pid};

mod support;

use support::{ALT_ENTER, ALT_LEAVE, Env, server_processes, wait_until, window};

/// The terminal size every attaching test uses.
const ROWS: u16 = 24;
const COLS: u16 = 80;

/// The reverse-video sequence the full client's status line is drawn with, and
/// the border glyphs it draws around panes — the chrome `--pane` must not have.
const STATUS_SGR: &[u8] = b"\x1b[7m";
const BORDER_GLYPHS: [&str; 6] = ["┌", "┐", "└", "┘", "─", "│"];

// ------------------------------------------------------ probe and daemonize

#[tokio::test]
async fn bare_amx_daemonizes_then_attaches_when_no_server_is_running() {
    let env = Env::new("daemonize");
    assert_eq!(
        probe(&env.socket()).expect("probe"),
        Probe::Absent,
        "the test starts with nothing running"
    );

    let mut client = env.spawn_on_tty(&[], ROWS, COLS);
    client.wait_for(ALT_ENTER);
    assert_eq!(
        probe(&env.socket()).expect("probe"),
        Probe::Running,
        "attaching started a server"
    );

    // The daemon is nobody's foreground child: the client detaches and exits,
    // and the server it started is still there afterwards. That is the whole
    // difference between daemonizing and backgrounding.
    let served_by = server_pid(&env.socket())
        .await
        .expect("peer")
        .expect("a pid");
    assert_ne!(served_by, client.pid(), "the server is not the client");

    client.chord(b'd');
    assert_eq!(client.wait(), Some(0));
    assert_eq!(probe(&env.socket()).expect("probe"), Probe::Running);
    assert_eq!(
        server_pid(&env.socket()).await.expect("peer"),
        Some(served_by),
        "the same server is still answering"
    );
}

#[tokio::test]
async fn bare_amx_attaches_to_an_existing_server_without_daemonizing() {
    let env = Env::new("existing");
    let mut server = env.spawn(&["server"]);
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });
    let before = server_pid(&env.socket())
        .await
        .expect("peer")
        .expect("a pid");
    assert_eq!(
        before,
        server.id(),
        "the server we started is the one running"
    );

    let mut client = env.spawn_on_tty(&[], ROWS, COLS);
    client.wait_for(ALT_ENTER);
    client.chord(b'd');
    assert_eq!(client.wait(), Some(0));

    assert_eq!(
        server_pid(&env.socket()).await.expect("peer"),
        Some(before),
        "a socket that answers is attached to, never started over"
    );
    assert_eq!(
        server_processes(&env.exe(), &env.session),
        1,
        "no second server was spawned"
    );

    let _ = server.kill();
    let _ = server.wait();
}

#[tokio::test]
async fn a_stale_socket_does_not_stop_bare_amx_from_starting_a_server() {
    let env = Env::new("staleboot");
    std::fs::create_dir_all(env.runtime_dir()).expect("create the runtime dir");
    // Exactly what a killed server leaves behind: the file, with nothing
    // listening on it.
    drop(std::os::unix::net::UnixListener::bind(env.socket()).expect("stale socket"));
    assert_eq!(probe(&env.socket()).expect("probe"), Probe::Stale);

    let mut client = env.spawn_on_tty(&[], ROWS, COLS);
    client.wait_for(ALT_ENTER);
    assert_eq!(
        probe(&env.socket()).expect("probe"),
        Probe::Running,
        "the stale file was replaced, not worked around or refused"
    );

    client.chord(b'd');
    assert_eq!(client.wait(), Some(0));
}

#[tokio::test]
async fn two_concurrent_amx_invocations_leave_exactly_one_server() {
    let env = Env::new("race");

    // Started back to back, so both probe an absent socket before either
    // server has bound: both daemonize, and `bind` is the arbiter. Sampling
    // `/proc` between the spawns and the first frame shows two servers alive
    // in five runs out of six, so the losing path is genuinely exercised; the
    // sixth is a client that found the other's socket already answering, which
    // must hold just as well and is why the assertions below are about the
    // outcome rather than about the race having happened.
    let mut first = env.spawn_on_tty(&[], ROWS, COLS);
    let mut second = env.spawn_on_tty(&[], ROWS, COLS);

    first.wait_for(ALT_ENTER);
    second.wait_for(ALT_ENTER);

    // Both clients are attached, and to the same server: one socket, one peer.
    let peer = server_pid(&env.socket())
        .await
        .expect("peer")
        .expect("a pid");
    assert_eq!(
        server_pid(&env.socket()).await.expect("peer"),
        Some(peer),
        "both attaches reached one server"
    );
    wait_until("the server that lost the bind race exits", || {
        server_processes(&env.exe(), &env.session) == 1
    });

    let listed = env.run(&["session", "list"]);
    assert_eq!(
        listed.ok().lines().count(),
        1,
        "one session, one server: {listed:?}"
    );

    first.chord(b'd');
    second.chord(b'd');
    assert_eq!(first.wait(), Some(0));
    assert_eq!(second.wait(), Some(0));
    assert_eq!(
        probe(&env.socket()).expect("probe"),
        Probe::Running,
        "the survivor is still serving after both clients left"
    );
}

// ------------------------------------------------------------- the lifecycle

#[tokio::test]
async fn session_stop_shuts_down_cleanly_and_removes_the_socket() {
    let env = Env::new("stop");
    let mut server = env.spawn(&["server"]);
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });

    let stopped = env.run(&["session", "stop"]);
    assert!(
        stopped.ok().contains("stopped"),
        "stop reports what it stopped: {stopped:?}"
    );

    // Clean means the process ended normally *and* took its socket with it —
    // a socket left behind is the stale file the next start has to probe.
    let code = wait_for_exit(&mut server);
    assert_eq!(code, Some(0), "the server exited normally on SIGTERM");
    assert!(!env.socket().exists(), "the socket was removed");
    assert_eq!(probe(&env.socket()).expect("probe"), Probe::Absent);
    assert!(
        env.run(&["session", "list"]).ok().is_empty(),
        "and it is gone from the list"
    );
}

#[tokio::test]
async fn a_deleted_session_takes_its_directories_with_it() {
    let env = Env::new("delete");
    let mut server = env.spawn(&["server"]);
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });

    let refused = env.run(&["session", "delete"]);
    assert_eq!(refused.code, Some(1), "a running session is not deletable");
    assert!(refused.stderr.contains("stop it first"), "{refused:?}");

    env.run(&["session", "stop"]).ok();
    let _ = wait_for_exit(&mut server);
    env.run(&["session", "delete"]).ok();
    assert!(!env.runtime_dir().exists(), "the runtime directory is gone");
}

// ------------------------------------------------------------ attach/detach

#[tokio::test]
async fn detach_restores_the_terminal_and_leaves_the_pane_running() {
    let env = Env::new("detach");
    let mut client = env.spawn_on_tty(&[], ROWS, COLS);
    client.wait_for(ALT_ENTER);

    let raw = client.termios();
    let session = session_id(&env);

    client.chord(b'd');
    assert_eq!(client.wait(), Some(0), "detach is a clean exit");

    assert!(
        window(client.output(), ALT_LEAVE),
        "the client left the alternate screen it entered"
    );
    let restored = client.termios();
    assert_ne!(
        raw,
        client.initial_termios(),
        "the client had the terminal in raw mode to begin with, or the next \
         assertion proves nothing"
    );
    assert_eq!(
        restored,
        client.initial_termios(),
        "the terminal is the way the client found it"
    );

    // And the session is untouched: same server, same instance, same state.
    assert_eq!(probe(&env.socket()).expect("probe"), Probe::Running);
    assert_eq!(
        session_id(&env),
        session,
        "detaching leaves the server and everything running in it alone"
    );
}

#[tokio::test]
async fn attach_pane_renders_full_screen_with_no_chrome() {
    let env = Env::new("viewport");
    let pane = amx_core::PaneId::new_v4().to_string();

    let mut viewport = env.spawn_on_tty(&["attach", "--pane", &pane], ROWS, COLS);
    viewport.wait_for(ALT_ENTER);
    // The bottom row: the full client reserves it for a status line, and the
    // viewport paints the pane over it.
    viewport.wait_for(format!("\x1b[{ROWS};1H").as_bytes());
    viewport.chord(b'q');
    assert_eq!(viewport.wait(), Some(0), "prefix+q detaches a viewport");

    let painted = viewport.output().to_vec();
    for glyph in BORDER_GLYPHS {
        assert!(
            !window(&painted, glyph.as_bytes()),
            "a one-pane viewport draws no border, and {glyph} is one"
        );
    }
    assert!(
        !window(&painted, STATUS_SGR),
        "nor a status line, which is the only thing drawn in reverse video"
    );

    // The differential: the ordinary client on the same terminal draws exactly
    // the chrome this one does not, so the assertions above are about the mode
    // and not about an empty screen.
    let mut chrome = env.spawn_on_tty(&[], ROWS, COLS);
    chrome.wait_for(STATUS_SGR);
    chrome.chord(b'd');
    assert_eq!(chrome.wait(), Some(0));
}

#[tokio::test]
async fn attaching_a_pipe_is_refused_before_a_server_is_started() {
    let env = Env::new("notty");
    let refused = env.run(&[]);
    assert_eq!(refused.code, Some(1));
    assert!(refused.stderr.contains("terminal"), "{refused:?}");
    assert_eq!(
        probe(&env.socket()).expect("probe"),
        Probe::Absent,
        "a command that cannot succeed does not leave a server behind"
    );
}

#[tokio::test]
async fn attach_pane_wants_a_pane_id() {
    let env = Env::new("badpane");
    let refused = env.run(&["attach", "--pane", "not-a-pane"]);
    assert_eq!(refused.code, Some(1));
    assert!(refused.stderr.contains("pane id"), "{refused:?}");
}

// ------------------------------------------------------------------ helpers

/// The session id the server reports, via `amx ping`.
fn session_id(env: &Env) -> String {
    let reply = env.run(&["ping"]);
    let json: serde_json::Value = serde_json::from_str(reply.ok()).expect("a JSON reply");
    json["session"].as_str().expect("a session id").to_owned()
}

/// Wait for a child to exit, up to [`support::PATIENCE`].
fn wait_for_exit(child: &mut std::process::Child) -> Option<i32> {
    let deadline = std::time::Instant::now() + support::PATIENCE;
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            return status.code();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the server did not exit"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
