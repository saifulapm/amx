//! The front door: what `amx` on its own does, and the room it opens.

mod common;

use common::Harness;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A server of amx's own, on a socket this test alone uses.
///
/// Its own socket rather than the harness's, because the only way to see what
/// a server was *born* with is to be there when it is born: tmux reads a
/// config file when it starts a server and on no later call.
struct Private(String);

impl Private {
    fn new() -> Private {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Private(format!(
            "amx-cockpit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn socket(&self) -> &str {
        &self.0
    }

    /// Ask this server something, or `None` while nothing is listening on it.
    fn ask(&self, args: &[&str]) -> Option<String> {
        ask(&self.0, args)
    }

    /// Every window on it, as `<session> <window>` lines.
    fn windows(&self) -> Vec<String> {
        self.ask(&["list-windows", "-a", "-F", "#{session_name} #{window_name}"])
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.0, "kill-server"])
            .output();
    }
}

/// Ask the server on this socket something, or `None` while nothing is
/// listening on it.
fn ask(socket: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("tmux")
        .args(["-L", socket])
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// A terminal that is not inside tmux, which is where the cockpit is opened
/// from, with amx's own server pinned to `cockpit`.
fn outside_tmux(cockpit: &Private) -> [(&str, &str); 3] {
    [
        ("TMUX", ""),
        ("TMUX_PANE", ""),
        ("AMX_TMUX_SOCKET", cockpit.socket()),
    ]
}

#[test]
fn read_by_a_program_the_front_door_is_the_table() {
    let amx = Harness::new();
    amx.record("fix-login-a1b", "%404");

    let out = amx.amx(&[]);
    assert!(
        out.status.success(),
        "amx: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains("fix-login-a1b"), "{printed}");
    assert_eq!(
        ask(amx.socket(), &["list-sessions"]),
        None,
        "a question answered off the disk starts nothing"
    );
}

#[test]
fn inside_tmux_the_view_opens_where_it_was_asked_from() {
    let amx = Harness::new();
    amx.record("fix-login-a1b", "%404");

    let pane = amx.in_a_terminal(&[], &[]);

    amx.until("the view to reach the terminal", || {
        amx.capture(&pane).contains("fix-login-a1b").then_some(())
    });
    assert!(
        !amx.tmux(&["list-windows", "-a", "-F", "#{window_name}"])
            .lines()
            .any(|name| name == "amx-view"),
        "in place means in the terminal it was asked from, not in a room of its own"
    );
}

#[test]
fn outside_tmux_the_front_door_opens_the_cockpit() {
    let amx = Harness::new();
    let cockpit = Private::new();
    amx.record("fix-login-a1b", "%404");

    let pane = amx.in_a_terminal(&outside_tmux(&cockpit), &[]);

    let windows = amx.until("the cockpit", || {
        let windows = cockpit.windows();
        (!windows.is_empty()).then_some(windows)
    });
    assert_eq!(windows, ["amx amx-view"], "one room, with the view in it");

    // And the terminal the command was typed in is now looking at it.
    amx.until("the view to reach the terminal", || {
        amx.capture(&pane).contains("fix-login-a1b").then_some(())
    });
    assert!(
        !cockpit
            .ask(&["list-clients", "-F", "#{client_tty}"])
            .unwrap_or_default()
            .is_empty(),
        "the command a person typed and the room they end up in are one thing"
    );
}

#[test]
fn the_cockpit_is_the_room_the_agents_are_in() {
    let amx = Harness::new();
    let cockpit = Private::new();
    let mock = amx.mock();

    // The cockpit first, and an agent into it afterwards.
    amx.in_a_terminal(&outside_tmux(&cockpit), &[]);
    amx.until("the cockpit", || {
        (!cockpit.windows().is_empty()).then_some(())
    });

    let out = amx
        .amx_command(&["new", "--no-worktree", "--agent", &mock, "look busy"])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .env("AMX_TMUX_SOCKET", cockpit.socket())
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut windows = amx.until("the agent's wall", || {
        let windows = cockpit.windows();
        (windows.len() == 2).then_some(windows)
    });
    windows.sort();
    assert_eq!(windows, ["amx amx-view", "amx amx-wall"]);
}

#[test]
fn a_cockpit_opened_after_an_agent_joins_the_room_it_made() {
    let amx = Harness::new();
    let cockpit = Private::new();
    let mock = amx.mock();

    let out = amx
        .amx_command(&["new", "--no-worktree", "--agent", &mock, "look busy"])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .env("AMX_TMUX_SOCKET", cockpit.socket())
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(cockpit.windows(), ["amx amx-wall"]);

    amx.in_a_terminal(&outside_tmux(&cockpit), &[]);

    let mut windows = amx.until("the view", || {
        let windows = cockpit.windows();
        (windows.len() == 2).then_some(windows)
    });
    windows.sort();
    assert_eq!(
        windows,
        ["amx amx-view", "amx amx-wall"],
        "one room, not a second one beside it"
    );
}

#[test]
fn opening_the_cockpit_twice_leaves_one_view() {
    let amx = Harness::new();
    let cockpit = Private::new();

    for _ in 0..2 {
        amx.in_a_terminal(&outside_tmux(&cockpit), &[]);
    }

    amx.until("both terminals to arrive", || {
        let clients = cockpit.ask(&["list-clients", "-F", "#{client_tty}"])?;
        (clients.lines().count() == 2).then_some(())
    });
    assert_eq!(cockpit.windows(), ["amx amx-view"]);
}

#[test]
fn the_private_server_is_born_with_amxs_own_settings() {
    // amx's config rides every call that could start its server, because tmux
    // reads one on no other call. Whichever call gets there first, the server
    // it starts is amx's own and not the machine's.
    let amx = Harness::new();
    let history = ["show-options", "-g", "-v", "history-limit"];

    let cockpit = Private::new();
    amx.in_a_terminal(&outside_tmux(&cockpit), &[]);
    assert_eq!(
        amx.until("the cockpit's server", || cockpit.ask(&history)),
        "50000",
        "the cockpit started it"
    );

    let spawned = Private::new();
    let out = amx
        .amx_command(&["new", "--no-worktree", "--agent", &amx.mock(), "look busy"])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .env("AMX_TMUX_SOCKET", spawned.socket())
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        spawned.ask(&history).as_deref(),
        Some("50000"),
        "an agent started it"
    );
}
