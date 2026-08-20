//! `amx attach` — hand this terminal to an agent's pane.
//!
//! amx is never in the byte path, so attaching is tmux attaching: the pane is
//! selected on the server the record names, and then this process is replaced
//! by tmux's own client. What happens after that is between the person and the
//! vendor.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

use crate::store::Agent;
use crate::tmux::{PaneId, Server, SessionId};
use crate::{exit, paths};

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let inside = std::env::var("TMUX").ok().filter(|v| !v.is_empty());
    run(&root, id, inside.as_deref())
}

/// Attach to `id`, from inside tmux or from outside it.
pub fn run(root: &Path, id: &str, inside: Option<&str>) -> Result<i32> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let server = Server::from_socket(meta.socket.clone());

    if !server.pane_alive(&meta.pane) {
        bail!("{id} has no pane any more");
    }
    let session = SessionId::new(server.pane_field(&meta.pane, "#{session_id}")?)
        .with_context(|| format!("finding the session {id} is in"))?;

    let mut command = client(&server, &session, &meta.pane, inside)?;
    exec(&mut command)
}

/// The tmux client that takes over this terminal.
///
/// Inside tmux there is already a client attached to this terminal, and
/// starting a second one is what "sessions should be nested with care" is
/// about; the client that is here switches instead.
fn client(
    server: &Server,
    session: &SessionId,
    pane: &PaneId,
    inside: Option<&str>,
) -> Result<Command> {
    // Point the session at the agent before handing the terminal over, so
    // whoever arrives is looking at the pane they asked for.
    server.run(&["select-window", "-t", pane.as_str()])?;
    server.run(&["select-pane", "-t", pane.as_str()])?;

    let mut command = server.command();
    if switches(server, inside) {
        command.arg("switch-client").arg("-t").arg(session.as_str());
    } else {
        command
            .arg("attach-session")
            .arg("-t")
            .arg(session.as_str());
    }
    Ok(command)
}

/// Whether the client already on this terminal can be asked to switch, rather
/// than a second one being started inside it.
fn switches(server: &Server, inside: Option<&str>) -> bool {
    inside
        .and_then(Server::from_tmux_env)
        .is_some_and(|here| here.socket() == server.socket())
}

/// Become tmux.
fn exec(command: &mut Command) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    // `exec` only returns when it failed to replace this process.
    let failed = command.exec();
    Err(failed).context("handing the terminal to tmux")?;
    Ok(exit::FAILURE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_asks_the_client_that_is_already_here_to_switch() {
        // Nesting a client inside a client is how a terminal ends up with two
        // status lines and no way back. When the agent is on the server this
        // terminal is already attached to, the client that is here switches.
        let here = Server::at("/tmp/tmux-1000/default");
        assert!(switches(&here, Some("/tmp/tmux-1000/default,4242,0")));
    }

    #[test]
    fn attach_from_elsewhere_starts_a_client_of_its_own() {
        let amx = Server::named("amx");
        // Outside tmux entirely.
        assert!(!switches(&amx, None));
        assert!(!switches(&amx, Some("")));
        // Inside tmux, but a different server: amx's own agents are not on it.
        assert!(!switches(&amx, Some("/tmp/tmux-1000/default,4242,0")));
        // And a pane on the person's server, from inside a different one.
        let theirs = Server::at("/tmp/tmux-1000/default");
        assert!(!switches(&theirs, Some("/tmp/tmux-1000/work,4242,0")));
    }
}
