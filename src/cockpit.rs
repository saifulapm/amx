//! `amx` on its own — the front door.
//!
//! Three doors, and what decides between them is where the command was typed
//! rather than anything on its command line. A program reading amx's output
//! wants the table and nothing else. A person already inside tmux wants the
//! view where they are, because they arranged that terminal themselves. A
//! person at a bare terminal wants the room: amx's own tmux, so the one on the
//! machine is never the thing standing between them and their agents.
//!
//! The room is one session on amx's own server — the view in a window, and
//! beside it the wall the agents tile into, which the first agent to arrive
//! makes whether that is before the cockpit or long after it.

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::Path;

use crate::config::Config;
use crate::spawn::{self, PRIVATE};
use crate::store::now;
use crate::tmux::{Server, SessionId, Spawn, WindowId};
use crate::{exit, paths, tui, verbs};

/// The window the view lives in.
pub const VIEW: &str = "amx-view";

/// Which door bare `amx` opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// Nobody is reading a screen: the table, once.
    Table,
    /// A terminal that is already inside tmux: the view goes in it.
    View,
    /// A terminal of its own: the room, and this terminal handed to it.
    Cockpit,
}

/// The door, given whether anybody is looking at a terminal and what tmux that
/// terminal is inside.
pub fn door(terminal: bool, inside: Option<&str>) -> Door {
    if !terminal {
        return Door::Table;
    }
    match inside {
        Some(tmux) if !tmux.is_empty() => Door::View,
        _ => Door::Cockpit,
    }
}

/// Open the front door against the machine.
pub fn from_env(config: &Config) -> Result<i32> {
    let root = paths::state_root()?;
    let inside = std::env::var("TMUX").ok();

    match door(std::io::stdout().is_terminal(), inside.as_deref()) {
        Door::Table => verbs::ls::run(&root, false, now(), &mut std::io::stdout().lock()),
        Door::View => tui::run(&root, config),
        // The view's pane runs amx again, and lands on the door above: inside
        // tmux by then, because that is what the room is made of.
        Door::Cockpit => open(&root, &std::env::current_exe().context("finding amx")?),
    }
}

/// Build the room and become its client.
fn open(root: &Path, view_command: &Path) -> Result<i32> {
    // Outside tmux, so this is amx's own server — carrying amx's own config on
    // every call that could be the one that starts it.
    let server = spawn::server(root)?;
    let session = ensure(&server, view_command, &spawn::wall_lock(root))?;
    attach(&server, &session)
}

/// The room, however much of it is there already.
///
/// A cockpit is opened far more often than it is made: an agent started from
/// outside tmux has the session and the wall up already, and what that leaves
/// out is the one pane a person types in.
///
/// The lock is the wall's own. A cockpit and an agent arriving together must
/// not make two rooms: finding and creating are two steps, and between them
/// the answer to "is there a session?" is still no.
fn ensure(server: &Server, view_command: &Path, lock: &Path) -> Result<SessionId> {
    let _held = spawn::hold(lock)?;

    let Some(session) = server.session_named(PRIVATE)? else {
        return open_session(server, view_command);
    };
    let window = match server.window_named(&session, VIEW)? {
        Some(window) => window,
        None => new_view(server, &session, view_command)?,
    };

    // Whatever was last on the screen, `amx` was typed to be back at the view.
    server.run(&["select-window", "-t", window.as_str()])?;
    Ok(session)
}

/// The room from nothing: a session whose first window is the view.
///
/// The config rides this call rather than a `start-server` of its own, and
/// that is the whole trick — a server with no session in it exits the moment
/// it starts, so the call that makes the first session is the only one whose
/// config a server lives to remember.
fn open_session(server: &Server, view_command: &Path) -> Result<SessionId> {
    let command = view_command.to_string_lossy();
    let (session, _) = server.new_session(&Spawn {
        name: Some(PRIVATE),
        window: Some(VIEW),
        command: &[&command],
        ..Spawn::default()
    })?;
    Ok(session)
}

/// The view for a room that has none — the one an agent made before anybody
/// looked in on it.
fn new_view(server: &Server, session: &SessionId, view_command: &Path) -> Result<WindowId> {
    let command = view_command.to_string_lossy();
    let (window, _) = server.new_window(
        session,
        &Spawn {
            name: Some(VIEW),
            command: &[&command],
            ..Spawn::default()
        },
    )?;
    Ok(window)
}

/// Become the client, so the command a person typed and the room they end up
/// in are one thing.
fn attach(server: &Server, session: &SessionId) -> Result<i32> {
    use std::os::unix::process::CommandExt;

    // `exec` only returns when it failed to replace this process.
    let failed = server.attach_command(session).exec();
    Err(failed).context("handing the terminal to the cockpit")?;
    Ok(exit::FAILURE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_the_door_is_chosen_by_where_the_command_was_typed() {
        // Something is reading the output, and reading is all it does.
        assert_eq!(door(false, None), Door::Table);
        assert_eq!(
            door(false, Some("/tmp/tmux-1000/default,42,0")),
            Door::Table
        );

        // A terminal inside tmux: the person arranged it, and the view goes
        // where they are.
        assert_eq!(door(true, Some("/tmp/tmux-1000/default,42,0")), Door::View);

        // A terminal of its own — including one whose tmux left the variable
        // behind empty.
        assert_eq!(door(true, None), Door::Cockpit);
        assert_eq!(door(true, Some("")), Door::Cockpit);
    }
}
