//! `amx attach` — hand this terminal to an agent's pane.
//!
//! amx is never in the byte path, so attaching is tmux attaching: the pane is
//! selected on the server the record names, and then this process is replaced
//! by tmux's own client. What happens after that is between the person and the
//! vendor.
//!
//! A pane that is gone is not the end of the verb. What somebody asked for is
//! to look at this agent, and an agent with a session behind it can be looked
//! at again: it is brought back into a pane first, and the terminal is handed
//! over to that one. Only an agent with nothing to continue is refused, and
//! then in the words that say which is missing.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::config::{self, Config};
use crate::store::Agent;
use crate::tmux::{PaneId, Server, SessionId};
use crate::verbs::resume::{self, Comeback};
use crate::{exit, paths, spawn};

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    // The config is read here because attaching may become a resume, and a
    // resume answers to `max_agents`. The environment for the same reason: an
    // agent that comes back runs in the environment the command that brought it
    // back was typed in, which is the rule `new` and `resume` both follow.
    let config = config::current();
    let env = spawn::env_snapshot(std::env::vars());
    let inside = std::env::var("TMUX").ok().filter(|v| !v.is_empty());
    run(&root, config, id, &env, inside.as_deref())
}

/// Attach to `id`, from inside tmux or from outside it.
pub fn run(
    root: &Path,
    config: &Config,
    id: &str,
    env: &BTreeMap<String, String>,
    inside: Option<&str>,
) -> Result<i32> {
    let agent = Agent::open(root, id)?;
    let mut meta = agent.meta()?;

    if !Server::from_socket(meta.socket.clone()).pane_alive(&meta.pane) {
        match resume::again(root, config, id, env)? {
            // The record now names a pane nothing has read yet, so it is read
            // again: where the agent is is what the rest of this verb is about.
            Comeback::Back => meta = agent.meta()?,
            Comeback::No(why) => bail!("{why}"),
        }
    }

    let server = Server::from_socket(meta.socket.clone());
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
        .is_some_and(|here| same_server(&here, server))
}

/// Whether two ways of writing a server address the same one.
///
/// Written the same way, they do, and that is answered without asking tmux
/// anything. Written differently, only tmux can say: `$TMUX` names a socket by
/// path, and an agent started outside tmux was recorded on `-L default`, which
/// is that same socket spelled the way each side learned it. A server nothing
/// is listening on has no path to give, and two silences are not a match.
fn same_server(here: &Server, there: &Server) -> bool {
    if here.socket() == there.socket() {
        return true;
    }
    let path = |server: &Server| {
        server
            .run(&["display-message", "-p", "#{socket_path}"])
            .ok()
            .filter(|path| !path.is_empty())
    };
    matches!((path(here), path(there)), (Some(here), Some(there)) if here == there)
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
    use crate::tmux::Spawn;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A socket name of this test's own.
    fn tag() -> String {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        format!(
            "amx-test-attach-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A server of this test's own, gone when the test is.
    struct TestServer {
        name: String,
        server: Server,
    }

    impl TestServer {
        /// A server with one idle session on it, and its socket's path.
        fn new() -> (TestServer, String) {
            let name = tag();
            // An empty conf, so nothing in the developer's ~/.tmux.conf can
            // change what these tests measure.
            let server = Server::named(&name).with_conf("/dev/null");
            server
                .new_session(&Spawn {
                    command: &["sh", "-c", "while :; do sleep 0.05; done"],
                    ..Spawn::default()
                })
                .expect("a server to ask about");
            let path = server
                .run(&["display-message", "-p", "#{socket_path}"])
                .expect("where its socket is");
            (TestServer { name, server }, path)
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.server.kill();
        }
    }

    #[test]
    fn attach_asks_the_client_that_is_already_here_to_switch() {
        // Nesting a client inside a client is how a terminal ends up with two
        // status lines and no way back. When the agent is on the server this
        // terminal is already attached to, the client that is here switches.
        let here = Server::at("/tmp/tmux-1000/default");
        assert!(switches(&here, Some("/tmp/tmux-1000/default,4242,0")));
    }

    #[test]
    fn attach_switches_when_the_socket_was_written_down_the_other_way() {
        // An agent started outside tmux is recorded on `-L default`, and the
        // `$TMUX` of the terminal it is attached from names that same socket by
        // path. Two spellings of one server, and only tmux can say so.
        let (here, path) = TestServer::new();
        let inside = format!("{path},4242,0");

        assert!(switches(&Server::named(&here.name), Some(&inside)));

        // A second server, live and answering, is still a different one.
        let (elsewhere, _) = TestServer::new();
        assert!(!switches(&Server::named(&elsewhere.name), Some(&inside)));
    }

    #[test]
    fn attach_from_elsewhere_starts_a_client_of_its_own() {
        // Sockets of this test's own throughout: a server the developer is
        // sitting in must not be what decides how this comes out.
        let agents = Server::named(tag());
        // Outside tmux entirely.
        assert!(!switches(&agents, None));
        assert!(!switches(&agents, Some("")));
        // Inside tmux, but a server the agents are not on — and a socket
        // nothing is listening on cannot answer its way into a match.
        let elsewhere = format!("/tmp/{}/socket", tag());
        assert!(!switches(&agents, Some(&format!("{elsewhere},4242,0"))));
        // And a pane on one server, from inside another.
        let theirs = Server::at(format!("/tmp/{}/socket", tag()));
        assert!(!switches(&theirs, Some(&format!("{elsewhere},4242,0"))));
    }
}
