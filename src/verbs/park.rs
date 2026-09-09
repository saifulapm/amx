//! `amx _park` — let an idle agent's pane go, and keep everything else.
//!
//! A vendor sitting at its prompt holds a couple of hundred megabytes to do
//! nothing with, and a wall of them is the machine's memory spent on turns
//! that ended hours ago. So the pane goes and the agent stays: the record, the
//! event log, the transcript and the vendor's session are where they were, and
//! the next enter, attach or resume puts it in a pane again.
//!
//! amx has no daemon, so nothing sits watching for the moment to do this. The
//! tmux server holding the pane is asked to run the verb once, `park_after`
//! seconds after the hook that left the record idle, which makes what is
//! written here the whole of the decision — and a decision taken against a
//! record and a pane that have both had that long to move on. So every reason
//! to leave the pane where it is is asked again from scratch, and any one of
//! them ends the verb having done nothing at all. Doing nothing is the usual
//! outcome and is not a failure: a timer that fired over an agent somebody
//! went back to has no complaint to make about it.

use anyhow::Result;
use std::path::Path;

use crate::store::{Agent, Event, Meta, Phase, State};
use crate::tmux::Server;
use crate::tui::rows::Arrangement;
use crate::verbs::stop;
use crate::{config, exit, paths, store};

/// What amx records when it lets a pane go.
const PARKED: &str = "park";

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    consider(&root, id, store::now())
}

/// The verb, with the state directory and the moment named.
pub fn consider(root: &Path, id: &str, now: u64) -> Result<i32> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    // The project's file over the person's, which is the rule `resume` follows
    // for the cap: what a pane of that project's is worth is that project's to
    // say, and this process is a timer a server fired rather than a command
    // somebody typed anywhere in particular.
    let (config, _) = config::for_dir(&meta.dir);
    let_go(root, &agent, &meta, config.park_after, now)
}

/// [`consider`], with the seconds an idle agent keeps its pane passed in
/// rather than read off the config, so a test says what the person's file
/// would have said. The suite cannot set an environment for one of its
/// threads; see [`crate::paths`].
fn let_go(root: &Path, agent: &Agent, meta: &Meta, park_after: u64, now: u64) -> Result<i32> {
    let state = agent.state()?;
    if !ripe(&state, park_after, now) {
        return Ok(exit::OK);
    }

    // The pane as it is now rather than as the timer was set over it. One that
    // has already gone was killed, or its server died, and both of those are
    // an agent that ended: a stamp on that record would tell every reader
    // after it that amx let this one go and will bring it back.
    let server = Server::from_socket(meta.socket.clone());
    if !server.pane_alive(&meta.pane) {
        return Ok(exit::OK);
    }

    // Somebody is looking at it, or has said they want to be. A pin outlives
    // the view it was made in, so it is read off the file that view left
    // rather than asked of a screen that is no longer open.
    if server.pane_watched(&meta.pane) || Arrangement::from_disk(root).has_pinned(agent.id()) {
        return Ok(exit::OK);
    }

    // stop's own ladder: the vendor is asked to finish what it is writing
    // before it is insisted on. What it was writing is the transcript, and the
    // transcript is what an agent that comes back comes back to.
    stop::end(&server, &meta.pane, &meta.id)?;

    let writer = agent.writer()?;
    writer.append(&Event::new(
        PARKED,
        serde_json::json!({ "idle": now.saturating_sub(state.since) }),
    ))?;
    // `observe` rather than `update_state`: this is something amx did, not
    // something the agent said, and a `last_event` that moved would put an
    // unread mark on a row with nothing new on it.
    writer.observe(|state| state.parked_at = now)?;
    Ok(exit::OK)
}

/// Whether the record says this agent has sat idle long enough for its pane to
/// be worth more than what it is doing with it.
fn ripe(state: &State, park_after: u64, now: u64) -> bool {
    park_after > 0 && state.state == Phase::Idle && now >= state.since.saturating_add(park_after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, now};
    use crate::tmux::{PaneId, Socket, Spawn};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// A server of this test's own, gone when the test is.
    struct TestServer {
        name: String,
        server: Server,
    }

    impl TestServer {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let name = format!(
                "amx-test-park-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            // An empty conf, so nothing in the developer's ~/.tmux.conf can
            // change what these tests measure.
            let server = Server::named(&name).with_conf("/dev/null");
            Self { name, server }
        }
    }

    impl std::ops::Deref for TestServer {
        type Target = Server;
        fn deref(&self) -> &Server {
            &self.server
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.server.kill();
        }
    }

    /// An agent with a record and a pane of its own, which is every refusal's
    /// starting point: what each of them proves is that this pane is still
    /// there afterwards.
    struct Sitting {
        server: TestServer,
        session: crate::tmux::SessionId,
        pane: PaneId,
        agent: Agent,
    }

    impl Sitting {
        /// One agent, in a pane running a shell that does not exit.
        ///
        /// In a session named the way [`crate::spawn::place`] names one, which
        /// is what makes the pane answer for this agent rather than for
        /// nobody: an agent whose pane is somebody else's has lost it, and a
        /// pane nobody can be shown to own is that.
        fn new(root: &Path, id: &str) -> Sitting {
            let server = TestServer::new();
            let (session, pane) = server
                .new_session(&Spawn {
                    name: Some(&format!("{}{id}", crate::tmux::SESSION_PREFIX)),
                    command: &["sh", "-c", "while :; do sleep 0.05; done"],
                    ..Spawn::default()
                })
                .expect("a pane for it");
            let agent = record(root, id, server.socket().clone(), pane.clone());
            Sitting {
                server,
                session,
                pane,
                agent,
            }
        }

        /// Put the record in a phase at a moment of the test's choosing.
        ///
        /// Through `observe`, because `update_state` stamps `since` with the
        /// clock, and what these tests are about is an agent that has been
        /// sitting there since before lunch.
        fn recorded(&self, phase: Phase, since: u64) -> &Sitting {
            self.agent
                .writer()
                .expect("the writer")
                .observe(|state| {
                    state.state = phase;
                    state.since = since;
                    state.last_event = since;
                })
                .expect("a record to read");
            self
        }

        /// Whether its pane is still there.
        fn has_a_pane(&self) -> bool {
            self.server.pane_alive(&self.pane)
        }
    }

    /// Poll until `f` is happy, rather than sleeping and hoping.
    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }

    /// A state directory, with room beside it for the view's own file.
    fn state_root(state: &TempDir) -> PathBuf {
        let root = state.path().join("agents");
        std::fs::create_dir_all(&root).expect("somewhere to keep the records");
        root
    }

    /// A record of an agent, pointed at whichever pane the test has.
    fn record(root: &Path, id: &str, socket: Socket, pane: PaneId) -> Agent {
        Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket,
                pane,
                bg: false,
                session: None,
                transcript: None,
                created: now(),
            },
        )
        .expect("the record")
    }

    /// The verb over the record this test has laid out, with the seconds and
    /// the moment it is about.
    fn considered(root: &Path, agent: &Agent, park_after: u64, now: u64) -> i32 {
        let meta = agent.meta().expect("the record");
        let_go(root, agent, &meta, park_after, now).expect("a decision")
    }

    /// What the verb left on the record: the stamp, and the log it wrote it in.
    fn left(agent: &Agent) -> (u64, Vec<String>) {
        let kinds = agent
            .events()
            .expect("the log")
            .into_iter()
            .map(|event| event.kind)
            .collect();
        (agent.state().expect("the record").parked_at, kinds)
    }

    #[test]
    fn park_takes_the_pane_of_an_agent_that_has_sat_idle_long_enough() {
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        assert_eq!(considered(&root, &it.agent, 3_600, 4_600), exit::OK);
        assert!(!it.has_a_pane(), "the pane went with the vendor");

        // Everything the agent is stays where it was. Only the stamp is new,
        // and it is what tells this pane from one that was killed.
        let after = it.agent.state().unwrap();
        assert_eq!(after.parked_at, 4_600);
        assert_eq!(after.state, Phase::Idle);
        assert_eq!(
            after.last_event, 1_000,
            "letting a pane go is something amx did, not something the agent \
             said, so the row does not turn unread"
        );
        assert_eq!(left(&it.agent).1, [PARKED]);
    }

    #[test]
    fn park_leaves_an_agent_that_is_not_idle_alone() {
        // The record is the answer, and this record says the agent is working:
        // the timer was set an hour ago and somebody has sent it something
        // since.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Working, 1_000);

        assert_eq!(considered(&root, &it.agent, 3_600, 4_600), exit::OK);
        assert!(it.has_a_pane(), "it is in the middle of a turn");
        assert_eq!(left(&it.agent), (0, Vec::new()));
    }

    #[test]
    fn park_takes_no_pane_at_all_where_the_key_is_zero() {
        // Idle since the morning, and the person has said panes are not amx's
        // to take.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        assert_eq!(considered(&root, &it.agent, 0, 90_000), exit::OK);
        assert!(it.has_a_pane(), "the key is off");
        assert_eq!(left(&it.agent), (0, Vec::new()));
    }

    #[test]
    fn park_waits_out_the_whole_of_the_key() {
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        // A second short, which is where a timer set over a turn that ended
        // and then started again lands.
        assert_eq!(considered(&root, &it.agent, 3_600, 4_599), exit::OK);
        assert!(it.has_a_pane(), "the hour is not up");
        assert_eq!(left(&it.agent), (0, Vec::new()));

        assert!(
            ripe(&it.agent.state().unwrap(), 3_600, 4_600),
            "and a second later it is the hour the key asked for"
        );
    }

    #[test]
    fn park_does_not_stamp_a_pane_that_had_already_gone() {
        // Killed, or its server died with it. Both are an agent that ended,
        // and a stamp here would tell every reader after it that amx let this
        // one go and will bring it back.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let agent = record(
            &root,
            "fix-login-a1b",
            Socket::Name(format!("amx-test-park-gone-{}", std::process::id())),
            PaneId::new("%404").unwrap(),
        );
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.since = 1_000;
            })
            .unwrap();

        assert_eq!(considered(&root, &agent, 3_600, 4_600), exit::OK);
        assert_eq!(left(&agent), (0, Vec::new()));
    }

    #[test]
    fn park_leaves_a_pane_somebody_is_looking_at() {
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        // A client on a terminal of its own, which is what `session_attached`
        // counts: a pane on another server, running a client of this one.
        let watcher = TestServer::new();
        watcher
            .new_session(&Spawn {
                command: &[
                    "tmux",
                    "-f",
                    "/dev/null",
                    "-L",
                    it.server.name.as_str(),
                    "attach-session",
                    "-t",
                    it.session.as_str(),
                ],
                ..Spawn::default()
            })
            .unwrap();
        until("somebody to attach to the agent", || {
            it.server.pane_watched(&it.pane)
        });

        assert_eq!(considered(&root, &it.agent, 3_600, 4_600), exit::OK);
        assert!(it.has_a_pane(), "it is on somebody's screen");
        assert_eq!(left(&it.agent), (0, Vec::new()));
    }

    #[test]
    fn park_leaves_an_agent_somebody_pinned_where_they_put_it() {
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        // The view's own file, as a view that has since been closed left it.
        // Pinning an agent is having said you want it in front of you.
        std::fs::write(
            crate::paths::view_file(&root).expect("somewhere to keep it"),
            br#"{"arrangement":{"held":["fix-login-a1b"]}}"#,
        )
        .unwrap();

        assert_eq!(considered(&root, &it.agent, 3_600, 4_600), exit::OK);
        assert!(it.has_a_pane(), "somebody wants it in front of them");
        assert_eq!(left(&it.agent), (0, Vec::new()));
    }
}
