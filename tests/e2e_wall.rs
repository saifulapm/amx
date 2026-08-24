//! Where an agent goes: a tmux session of its own, on the server the person
//! already has, and nothing of amx's standing beside it.

mod common;

use common::Harness;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

/// A socket nothing has touched yet, standing in for the person's default
/// server.
///
/// Its own socket rather than the harness's, because the only way to see what
/// a server was *born* with is to be there when it is born: tmux reads a
/// config file when it starts a server and on no later call.
struct Theirs(String);

impl Theirs {
    fn new() -> Theirs {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Theirs(format!(
            "amx-theirs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn socket(&self) -> &str {
        &self.0
    }

    fn ask(&self, args: &[&str]) -> Option<String> {
        ask(&self.0, args)
    }
}

impl Drop for Theirs {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.0, "kill-server"])
            .output();
    }
}

/// Ask the server on this socket something, or `None` while nothing is
/// listening on it.
///
/// No `-f`: what these tests ask about is the config a server was born with,
/// and a flag here would be a second answer to the same question.
fn ask(socket: &str, args: &[&str]) -> Option<String> {
    ask_in(None, socket, args)
}

/// The same, with tmux's socket directory pointed somewhere of the test's own.
fn ask_in(tmpdir: Option<&Path>, socket: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new("tmux");
    command.args(["-L", socket]).args(args);
    if let Some(tmpdir) = tmpdir {
        command.env("TMUX_TMPDIR", tmpdir);
    }
    let out = command.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The server a bare `tmux` reaches, in a socket directory of this test's own.
///
/// tmux keeps that socket under `$TMUX_TMPDIR`, so a directory nothing else
/// shares is how a test can ask what amx does when no socket was named without
/// going anywhere near the machine's real server.
struct Bare(TempDir);

impl Bare {
    fn new() -> Bare {
        Bare(TempDir::new().expect("a socket directory"))
    }

    fn tmpdir(&self) -> &Path {
        self.0.path()
    }

    fn ask(&self, args: &[&str]) -> Option<String> {
        ask_in(Some(self.tmpdir()), "default", args)
    }

    /// Every socket sitting in the directory, whatever it is called. tmux puts
    /// them in one directory of its own per user, and this is the only place
    /// where a private server of amx's would show up.
    fn sockets(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.tmpdir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .flat_map(|dir: PathBuf| std::fs::read_dir(dir).into_iter().flatten().flatten())
            .map(|socket| socket.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Bare {
    fn drop(&mut self) {
        // Every socket, not only the default one: a test that finds a server
        // it did not expect has to take that one with it too, or a failure
        // leaves an agent running with nothing left to reach it by.
        for socket in self.sockets() {
            let _ = ask_in(Some(self.tmpdir()), &socket, &["kill-server"]);
        }
    }
}

/// Ask the harness's server about one of its panes, windows or sessions.
fn field(amx: &Harness, target: &str, format: &str) -> String {
    amx.tmux(&["display-message", "-p", "-t", target, format])
}

/// Start an agent that keeps running, with `env` on top of the harness's own.
fn start(amx: &Harness, env: &[(&str, &str)], task: &str) -> Output {
    let mut command = amx.amx_command(&["new", "--no-worktree", "--agent", &amx.mock(), task]);
    command.env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"));
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("running amx new")
}

fn id_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The environment of a terminal that is inside tmux, and the pane it is in.
fn inside_tmux(amx: &Harness) -> (Vec<(String, String)>, String) {
    let env = amx.inside_tmux();
    let pane = env
        .iter()
        .find(|(name, _)| name == "TMUX_PANE")
        .map(|(_, pane)| pane.clone())
        .expect("the pane the terminal is in");
    (env, pane)
}

#[test]
fn an_agent_lives_in_a_session_named_for_it() {
    let amx = Harness::new();
    let id = id_of(&start(&amx, &[], "look busy"));

    let pane = amx.pane_of(&id);
    assert_eq!(
        field(&amx, &pane, "#{session_name}"),
        format!("amx-{id}"),
        "one session per agent, and the id is what it is called"
    );

    let session = field(&amx, &pane, "#{session_id}");
    assert_eq!(
        amx.tmux(&["list-panes", "-s", "-t", &session, "-F", "#{pane_id}"])
            .lines()
            .count(),
        1,
        "and nothing else is in it"
    );
    assert_eq!(
        amx.tmux(&["show-options", "-t", &session, "-v", "destroy-unattached"]),
        "off",
        "so looking in on it and leaving again does not end it"
    );
}

#[test]
fn an_agent_started_from_inside_tmux_leaves_the_window_where_it_was() {
    // The whole of why an agent gets a session rather than a window. tmux
    // switches to a window it has just made unless it is told not to, so
    // `amx new` used to take the screen out from under whoever typed it.
    let amx = Harness::new();
    let (env, pane) = inside_tmux(&amx);
    let session = field(&amx, &pane, "#{session_id}");

    // A second window, so the one being looked at is a choice and not the
    // only thing there is to look at.
    amx.tmux(&[
        "new-window",
        "-d",
        "-t",
        &session,
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
    let windows = amx.tmux(&["list-windows", "-t", &session, "-F", "#{window_id}"]);
    let watching = field(&amx, &session, "#{window_id}");

    let mut command =
        amx.amx_command(&["new", "--no-worktree", "--agent", &amx.mock(), "look busy"]);
    command.env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"));
    command.envs(env);
    let id = id_of(&command.output().expect("running amx new"));

    assert_eq!(
        field(&amx, &session, "#{window_id}"),
        watching,
        "the window a person was looking at is the window they are still looking at"
    );
    assert_eq!(
        amx.tmux(&["list-windows", "-t", &session, "-F", "#{window_id}"]),
        windows,
        "and nothing was added to the session they were in"
    );
    assert_eq!(
        field(&amx, &amx.pane_of(&id), "#{session_name}"),
        format!("amx-{id}"),
        "the agent is on the same server, in a session of its own"
    );
}

#[test]
fn agents_never_share_a_window_and_never_wait_on_each_other() {
    let amx = Harness::new();
    let (env, _) = inside_tmux(&amx);

    let mut ids = Vec::new();
    for task in ["the first", "the second"] {
        let mut command = amx.amx_command(&["new", "--no-worktree", "--agent", &amx.mock(), task]);
        command.env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"));
        command.envs(env.clone());
        ids.push(id_of(&command.output().expect("running amx new")));
    }

    let sessions: Vec<String> = ids
        .iter()
        .map(|id| field(&amx, &amx.pane_of(id), "#{session_name}"))
        .collect();
    assert_eq!(
        sessions,
        ids.iter().map(|id| format!("amx-{id}")).collect::<Vec<_>>(),
        "a session each, not a wall they are tiled into together"
    );
}

#[test]
fn amx_puts_no_window_of_its_own_between_a_person_and_their_agents() {
    // The wall, and the pane that stood on it saying what to type while it was
    // empty, are both gone. What amx makes on a person's server is one session
    // per agent and nothing besides.
    let amx = Harness::new();
    let (env, pane) = inside_tmux(&amx);
    let theirs = field(&amx, &pane, "#{session_name}");

    let mut command =
        amx.amx_command(&["new", "--no-worktree", "--agent", &amx.mock(), "look busy"]);
    command.env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"));
    command.envs(env);
    let id = id_of(&command.output().expect("running amx new"));

    let mut windows: Vec<String> = amx
        .tmux(&["list-windows", "-a", "-F", "#{session_name} #{window_name}"])
        .lines()
        .map(str::to_string)
        .collect();
    windows.sort();
    assert_eq!(
        windows.len(),
        2,
        "the person's window and the agent's, and nothing amx put up: {windows:?}"
    );
    assert!(
        windows
            .iter()
            .all(|line| line.starts_with(&theirs) || line.starts_with(&format!("amx-{id} "))),
        "{windows:?}"
    );
    assert!(
        !windows.iter().any(|line| line.contains("amx-wall")),
        "{windows:?}"
    );
}

#[test]
fn the_server_an_agent_lands_on_reads_the_config_the_person_wrote() {
    // amx carries no tmux config of its own any more. The server an agent
    // lands on is the person's, and whichever call starts it, it is born
    // reading their file and nobody else's.
    let amx = Harness::new();
    let theirs = Theirs::new();
    std::fs::write(amx.home().join(".tmux.conf"), "set -g history-limit 4242\n")
        .expect("the person's own tmux config");

    let out = start(&amx, &[("AMX_TMUX_SOCKET", theirs.socket())], "look busy");
    id_of(&out);

    assert_eq!(
        theirs
            .ask(&["show-options", "-g", "-v", "history-limit"])
            .as_deref(),
        Some("4242"),
        "the server amx started read ~/.tmux.conf"
    );
    let state = amx.state_root();
    assert!(
        !state
            .parent()
            .expect("the state root has a parent")
            .join("amx.tmux.conf")
            .exists(),
        "and amx wrote no config of its own for it to read instead"
    );
}

#[test]
fn outside_tmux_an_agent_lands_on_the_server_a_bare_tmux_reaches() {
    // The default server, and no other: a person who types `tmux ls` after
    // starting an agent has to find it there. amx used to keep its agents on a
    // private `-L amx` server, where nothing they already had could see them.
    let amx = Harness::new();
    let bare = Bare::new();

    let mut command =
        amx.amx_command(&["new", "--no-worktree", "--agent", &amx.mock(), "look busy"]);
    command
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("works-without-end"))
        .env("TMUX_TMPDIR", bare.tmpdir())
        // The harness pins every other test to a socket of its own. This one is
        // about what amx does when nobody has named a socket at all.
        .env_remove("AMX_TMUX_SOCKET");
    let id = id_of(&command.output().expect("running amx new"));

    assert_eq!(
        bare.ask(&["list-sessions", "-F", "#{session_name}"]),
        Some(format!("amx-{id}")),
        "the agent is a session on the server a bare tmux reaches"
    );
    assert_eq!(
        bare.sockets(),
        ["default"],
        "and amx started no server of its own beside it"
    );
    assert_eq!(
        ask(amx.socket(), &["list-sessions"]),
        None,
        "nothing was left on the socket this harness names either"
    );
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
