//! `amx attach` — hand this terminal to an agent's pane.
//!
//! amx is never in the byte path, so attaching is tmux attaching: the pane is
//! selected on the server the record names, and then this process is replaced
//! by tmux's own client. What happens after that is between the person and the
//! vendor.
//!
//! A pane that is gone is not the end of the verb, and neither is a pane that
//! turns out to be another agent's — the same loss with somebody standing in
//! the way, and handing that pane over would show the person somebody else's
//! work under the name they asked for. What somebody asked for is to look at
//! this agent, and an agent with a session behind it can be looked at again:
//! it is brought back into a pane first, and the terminal is handed over to
//! that one. Only an agent with nothing to continue is refused, and then in
//! the words that say which is missing.
//!
//! The agent can also be left unsaid, which is what a tmux key presses: alt-j
//! is one keystroke and has no room to type an id in, so `--next`, `--prev`
//! and `--waiting` ask the wall which agent instead. The wall is the view's
//! own order, read off the arrangement the last view left behind, so stepping
//! through it from a key lands in the order somebody arranged the rows into.
//! Where the stepping starts from is the agent whose session the key was
//! pressed in, which tmux is the only thing that can say.
//!
//! `--last` asks a different question: not where this agent is on the wall,
//! but where whoever pressed the key was before they came here. That is the
//! one thing the wall cannot answer, so it is written down as it happens —
//! every terminal amx hands over, here and in the view, goes on the trail.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::config::{self, Config};
use crate::store::{Agent, now};
use crate::tmux::{self, PaneId, Server, SessionId};
use crate::tui::rows::{self, Arrangement, Group};
use crate::verbs::resume::{self, Comeback};
use crate::{derive, exit, paths, spawn};

/// Where tmux says which pane a process is running in.
const PANE_ENV: &str = "TMUX_PANE";

/// What a wall with nobody on it has to say to a key asking for the next
/// agent: there is no such agent, and no direction changes that.
const EMPTY: &str = "nothing on the wall to attach to";

/// How far back the trail goes. Long enough that a morning's work is on it,
/// short enough that the file stays a file somebody could read.
const TRAIL: usize = 20;

/// Which agent the terminal was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aim {
    /// The one somebody named.
    Id(String),
    /// The one after this on the wall.
    Next,
    /// The one before it.
    Prev,
    /// The first one with something for somebody to do.
    Waiting,
    /// The one this terminal was in before this one.
    Last,
}

/// Run the verb against the machine.
pub fn from_env(aim: &Aim) -> Result<i32> {
    let root = paths::state_root()?;
    // The config is read here because attaching may become a resume, and a
    // resume answers to `max_agents`. The environment for the same reason: an
    // agent that comes back runs in the environment the command that brought it
    // back was typed in, which is the rule `new` and `resume` both follow.
    let config = config::current();
    let env = spawn::env_snapshot(std::env::vars());
    let inside = std::env::var("TMUX").ok().filter(|v| !v.is_empty());

    let id = match aim {
        Aim::Id(id) => id.clone(),
        _ => {
            // Everything on the machine, in the order the view would draw it.
            // A reading each time rather than anything remembered: the wall a
            // key steps through is the wall as it is when the key is pressed.
            let views = derive::views(&root, now())?;
            let order = rows::wall_order(&views, &Arrangement::from_disk(&root));
            if order.is_empty() {
                bail!(EMPTY);
            }
            let pane = std::env::var(PANE_ENV).ok();
            let current = current(&order, inside.as_deref(), pane.as_deref());
            pick(&order, current.as_deref(), &visited(&root), aim)?
        }
    };

    run(&root, config, &id, &env, inside.as_deref())
}

/// The agent whose session this command was typed in, if it is one the wall
/// is holding.
///
/// Outside tmux there is no answer, and no guess worth making: a key bound in
/// tmux is pressed inside it, and a shell prompt somewhere else is standing in
/// no agent at all. Inside it, the session is the one fact that says which
/// agent this is, because every pane amx places sits in a session named after
/// the agent it holds.
fn current(order: &[(Group, String)], inside: Option<&str>, pane: Option<&str>) -> Option<String> {
    let server = Server::from_tmux_env(inside?)?;
    let pane = PaneId::new(pane.filter(|pane| !pane.is_empty())?).ok()?;
    let name = server.pane_field(&pane, "#{session_name}").ok()?;
    let id = agent_in(&name)?;
    // A session amx named after an agent it has since forgotten is a name and
    // nothing else, and stepping on from it would step from nowhere.
    order.iter().any(|(_, on)| *on == id).then_some(id)
}

/// The agents this machine's terminals have been handed to, newest first.
///
/// A convenience and not a record: a trail that will not read is a trail
/// nobody has left yet, because refusing to attach over a file somebody's
/// editor half wrote would be the worse answer.
fn visited(root: &Path) -> Vec<String> {
    paths::visited_file(root)
        .and_then(|path| std::fs::read(&path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Write `id` down as where this terminal has just been.
///
/// Nothing it can fail at is worth failing an attach over: the terminal is
/// about to become tmux, and a trail that could not be written costs one
/// `--last` rather than the thing somebody actually asked for.
pub fn note_visited(root: &Path, id: &str) {
    let Some(path) = paths::visited_file(root) else {
        return;
    };
    // The front, and only once: the trail says where somebody has been in the
    // order they were there, and an agent they keep coming back to is one
    // place on it rather than twenty.
    let mut trail = visited(root);
    trail.retain(|on| on != id);
    trail.insert(0, id.to_string());
    trail.truncate(TRAIL);

    if let Ok(mut bytes) = serde_json::to_vec_pretty(&trail) {
        bytes.push(b'\n');
        let _ = crate::store::write_atomic(&path, &bytes);
    }
}

/// The agent a tmux session's name says it holds, if it says so.
fn agent_in(session_name: &str) -> Option<String> {
    let id = session_name.trim().strip_prefix(tmux::SESSION_PREFIX)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Which agent on the wall the aim lands on, from where the key was pressed.
///
/// Stepping wraps, because a wall is a list somebody is going round rather
/// than a queue they are working off: alt-j at the foot of it comes back to
/// the top. With nowhere to step from — the key pressed at a shell, or in an
/// agent the wall has forgotten — the step starts at the end it is heading
/// away from, so one press lands on the first row for `--next` and the last
/// for `--prev`.
///
/// `visited` is the trail, newest first, which is what `--last` goes back
/// along. The agent it is already in is skipped, so two presses toggle between
/// two agents rather than standing still on one.
fn pick(
    order: &[(Group, String)],
    current: Option<&str>,
    visited: &[String],
    aim: &Aim,
) -> Result<String> {
    let at = current.and_then(|id| order.iter().position(|(_, on)| on == id));
    let wrapped = |row: Option<&(Group, String)>| match row {
        Some((_, id)) => Ok(id.clone()),
        None => bail!(EMPTY),
    };
    match aim {
        Aim::Id(id) => Ok(id.clone()),
        Aim::Next => wrapped(at.and_then(|at| order.get(at + 1)).or(order.first())),
        Aim::Prev => wrapped(
            at.and_then(|at| at.checked_sub(1))
                .and_then(|before| order.get(before))
                .or(order.last()),
        ),
        // Which agent is waiting on somebody is the wall's own question, and
        // the key on the list asks it of the same rows: see
        // [`rows::needing_you`] for what it answers and why.
        Aim::Waiting => match rows::needing_you(order) {
            Some(id) => Ok(id),
            None => bail!("nothing on the wall is waiting on you"),
        },
        // An agent the wall no longer holds is a name on the trail and
        // nothing to go back to, so the trail is read past it.
        Aim::Last => {
            let back = visited
                .iter()
                .find(|id| Some(id.as_str()) != current && order.iter().any(|(_, on)| on == *id));
            match back {
                Some(id) => Ok(id.clone()),
                None => bail!("no agent to go back to"),
            }
        }
    }
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

    if !Server::from_socket(meta.socket.clone()).pane_answers_for(&meta.pane, &meta.id) {
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
    // Written down once the terminal is going to be handed over and before it
    // is: after this line there is no process here to write anything, and
    // before the tmux above answered there was nothing to say somebody got in.
    note_visited(root, id);
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
    // The way back first: ctrl+z inside the session lands whoever pressed it
    // where this was typed, at the shell or in the session the client left.
    server.bind_way_back()?;
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

    /// A wall, as `rows::wall_order` answers with one.
    fn wall(rows: &[(Group, &str)]) -> Vec<(Group, String)> {
        rows.iter()
            .map(|(group, id)| (*group, (*id).to_string()))
            .collect()
    }

    /// The wall a person opens in the morning: something pinned, a question,
    /// work in flight, a turn that ended, and one they put under everything.
    fn a_wall() -> Vec<(Group, String)> {
        wall(&[
            (Group::Pinned, "pin-a1b"),
            (Group::NeedsInput, "ask-b2c"),
            (Group::Working, "busy-c3d"),
            (Group::Completed, "done-d4e"),
            (Group::Asleep, "gone-e5f"),
        ])
    }

    #[test]
    fn attach_next_and_prev_step_through_the_wall_and_wrap() {
        let wall = a_wall();
        let step = |current: Option<&str>, aim| pick(&wall, current, &[], &aim).unwrap();

        assert_eq!(step(Some("ask-b2c"), Aim::Next), "busy-c3d");
        assert_eq!(step(Some("ask-b2c"), Aim::Prev), "pin-a1b");

        // Round the ends: the foot of the wall leads back to the head of it.
        assert_eq!(step(Some("gone-e5f"), Aim::Next), "pin-a1b");
        assert_eq!(step(Some("pin-a1b"), Aim::Prev), "gone-e5f");

        // Typed where no agent is: one press lands on the end it is coming
        // from, so a person who pressed the key at a shell is on the wall.
        assert_eq!(step(None, Aim::Next), "pin-a1b");
        assert_eq!(step(None, Aim::Prev), "gone-e5f");
    }

    #[test]
    fn attach_waiting_takes_the_question_first_and_the_last_ending_last() {
        let waiting = |wall: &[(Group, String)]| pick(wall, None, &[], &Aim::Waiting);

        // A question nobody has answered is what is holding somebody up.
        assert_eq!(waiting(&a_wall()).unwrap(), "ask-b2c");

        // With nothing asking, work standing in front of a reviewer.
        let reviewing = wall(&[
            (Group::Working, "busy-c3d"),
            (Group::Review, "ship-f6g"),
            (Group::Completed, "done-d4e"),
        ]);
        assert_eq!(waiting(&reviewing).unwrap(), "ship-f6g");

        // And with neither, the turn that ended most recently — which the
        // wall has already put at the head of its group.
        let ended = wall(&[
            (Group::Working, "busy-c3d"),
            (Group::Completed, "late-g7h"),
            (Group::Completed, "early-h8i"),
        ]);
        assert_eq!(waiting(&ended).unwrap(), "late-g7h");

        // A wall of work in flight and rows somebody put away is a wall with
        // nothing on it for them, and saying so beats attaching to anything.
        let nothing = wall(&[(Group::Working, "busy-c3d"), (Group::Asleep, "gone-e5f")]);
        let why = waiting(&nothing).unwrap_err().to_string();
        assert!(why.contains("waiting on you"), "{why}");
    }

    #[test]
    fn attach_last_goes_back_to_the_agent_you_came_from() {
        let wall = a_wall();
        let trail = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        let back =
            |current: Option<&str>, visited: &[String]| pick(&wall, current, visited, &Aim::Last);

        // Two agents and one key between them: from the one somebody came to,
        // going back is the one they came from, and from there it is the one
        // they just left. Two presses toggle.
        let there = trail(&["busy-c3d", "ask-b2c"]);
        assert_eq!(back(Some("busy-c3d"), &there).unwrap(), "ask-b2c");
        let and_back = trail(&["ask-b2c", "busy-c3d"]);
        assert_eq!(back(Some("ask-b2c"), &and_back).unwrap(), "busy-c3d");

        // Pressed at a shell, standing in no agent at all: the head of the
        // trail is the agent this terminal was last handed to, and going back
        // goes there.
        assert_eq!(back(None, &there).unwrap(), "busy-c3d");

        // An agent the wall no longer holds is a name and nothing to go back
        // to, so the trail is read past it.
        let gone_since = trail(&["forgotten-z9z", "done-d4e"]);
        assert_eq!(back(None, &gone_since).unwrap(), "done-d4e");

        // And a trail with nobody on it to go back to says that rather than
        // standing still on the agent somebody is already in.
        let only_here = trail(&["busy-c3d", "forgotten-z9z"]);
        let why = back(Some("busy-c3d"), &only_here).unwrap_err().to_string();
        assert!(why.contains("no agent to go back to"), "{why}");
        assert!(back(None, &[]).is_err(), "and nowhere is nowhere");
    }

    #[test]
    fn attach_keeps_the_trail_newest_first_and_one_place_per_agent() {
        let state = tempfile::TempDir::new().unwrap();
        let root = state.path().join("agents");
        assert!(visited(&root).is_empty(), "a trail nobody has left yet");

        note_visited(&root, "ask-b2c");
        note_visited(&root, "busy-c3d");
        assert_eq!(visited(&root), ["busy-c3d", "ask-b2c"]);

        // Back to the one before: an agent somebody keeps returning to is one
        // place on the trail, at the front of it.
        note_visited(&root, "ask-b2c");
        assert_eq!(visited(&root), ["ask-b2c", "busy-c3d"]);

        // A trail and not a log: what falls off the end is the oldest.
        for nth in 0..TRAIL {
            note_visited(&root, &format!("agent-{nth:02}"));
        }
        let trail = visited(&root);
        assert_eq!(trail.len(), TRAIL);
        assert_eq!(trail.first().unwrap(), &format!("agent-{:02}", TRAIL - 1));
        assert!(!trail.iter().any(|id| id == "ask-b2c"));

        // Half a file is a trail nobody has left, because a file amx keeps
        // for its own convenience must not be what refuses an attach.
        let path = paths::visited_file(&root).expect("somewhere to keep it");
        std::fs::write(&path, "[\"ask-b2c").unwrap();
        assert!(visited(&root).is_empty());
    }

    #[test]
    fn attach_steps_from_the_agent_whose_session_the_key_was_pressed_in() {
        // Every pane amx places sits in a session named after the agent it
        // holds, and that name is the only thing that says which agent a key
        // was pressed inside.
        assert_eq!(
            agent_in("amx-fix-login-a1b").as_deref(),
            Some("fix-login-a1b")
        );

        // Somebody's own session, and amx's own prefix with nothing after it.
        assert_eq!(agent_in("work"), None);
        assert_eq!(agent_in(""), None);
        assert_eq!(agent_in("amx-"), None);
    }

    #[test]
    fn attach_steps_from_the_session_this_pane_is_in() {
        let (here, path) = TestServer::new();
        let (_, pane) = Server::named(&here.name)
            .new_session(&Spawn {
                name: Some(&format!("{}fix-login-a1b", tmux::SESSION_PREFIX)),
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .expect("a session named after the agent in it");
        let inside = format!("{path},4242,0");

        let rows = wall(&[(Group::Working, "fix-login-a1b")]);
        assert_eq!(
            current(&rows, Some(&inside), Some(pane.as_str())).as_deref(),
            Some("fix-login-a1b")
        );

        // A session amx named after an agent that has since been forgotten is
        // a name and nothing else: stepping on from it would step from
        // nowhere, and the end of the wall is the better answer.
        assert_eq!(current(&a_wall(), Some(&inside), Some(pane.as_str())), None);

        // Outside tmux, and inside a tmux that named no pane: no session to
        // read, so no agent to step from.
        assert_eq!(current(&rows, None, Some(pane.as_str())), None);
        assert_eq!(current(&rows, Some(&inside), None), None);
        assert_eq!(current(&rows, Some(&inside), Some("")), None);
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
