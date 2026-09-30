//! `amx _park`: end an idle agent's pane to free its memory, keeping the agent.
//!
//! The record, event log, transcript and vendor session stay, and the next
//! enter, attach or resume gives the agent a pane again. With no daemon, the
//! tmux server runs this verb once, `park_after` seconds after the hook that
//! left the record idle. Every condition is checked again at that point, and
//! any reason to keep the pane makes the verb exit 0 having done nothing.

use anyhow::Result;
use std::path::Path;

use crate::store::{Agent, Event, Meta, Phase, State};
use crate::tmux::Server;
use crate::tui::rows::Arrangement;
use crate::verbs::stop;
use crate::{config, exit, paths, store};

/// The event logged when amx lets a pane go.
pub(crate) const PARKED: &str = "park";

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    consider(&root, id, store::now())
}

/// The verb, with the state directory and the moment named.
pub fn consider(root: &Path, id: &str, now: u64) -> Result<i32> {
    // A record cleared since the timer was set is nothing to do. An invalid
    // id still fails, and so does a record that is there but unreadable.
    if !paths::agent_dir_in(root, id)?.is_dir() {
        return Ok(exit::OK);
    }
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    // The agent's project config, as `resume` reads it: this runs from a tmux
    // timer, not from any particular directory.
    let (config, _) = config::for_dir(&meta.dir);
    let_go(root, &agent, &meta, config.park_after, now)
}

/// [`consider`] with `park_after` passed in, so tests need not set a config.
fn let_go(root: &Path, agent: &Agent, meta: &Meta, park_after: u64, now: u64) -> Result<i32> {
    // Held to the end, so a send or hook lands either before the reading (and
    // park sees it) or after the stamp (and `send` refuses).
    let writer = agent.writer()?;
    let state = writer.state()?;
    if !ripe(&state, park_after, now) {
        return Ok(exit::OK);
    }

    // A pane already gone means the agent ended, and a stamp would misreport it
    // as parked. A pane number reused by another agent is not ours to end. A
    // tmux that cannot be asked is an error.
    let server = Server::from_socket(meta.socket.clone());
    if !server.answers_for_now(&meta.pane, &meta.id)? {
        return Ok(exit::OK);
    }

    // Someone is watching the pane, or pinned the agent in the view (read off
    // the file the view saves, since it may be closed).
    if server.pane_watched(&meta.pane) || Arrangement::from_disk(root).has_pinned(agent.id()) {
        return Ok(exit::OK);
    }

    // Stamp before the kill, so a reader never sees a missing pane without the
    // stamp and calls the agent stopped. A failed kill removes the stamp.
    // `observe`, so `last_event` does not move and the row shows nothing new.
    writer.observe(|state| state.parked_at = now)?;

    // stop's SIGTERM-then-SIGKILL ladder, so the vendor can flush the
    // transcript the agent resumes from.
    if let Err(e) = stop::end(&server, &meta.pane, &meta.id) {
        writer.observe(|state| state.parked_at = 0)?;
        return Err(e);
    }

    writer.append(&Event::new(
        PARKED,
        serde_json::json!({ "idle": now.saturating_sub(state.since) }),
    ))?;
    Ok(exit::OK)
}

/// Whether the record has been idle for at least `park_after` seconds. Zero
/// disables parking.
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

    /// A private tmux server, killed on drop.
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
            // An empty conf, so the developer's ~/.tmux.conf has no effect.
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

    /// An agent with a record and a live pane of its own.
    struct Sitting {
        server: TestServer,
        session: crate::tmux::SessionId,
        pane: PaneId,
        agent: Agent,
    }

    impl Sitting {
        /// One agent in a pane running a shell that does not exit.
        ///
        /// The session is named as [`crate::spawn::place`] names it, which is
        /// what makes the pane answer for this agent.
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

        /// Put the record in `phase` since `since`.
        ///
        /// Through `observe`, because `update_state` would stamp `since` with
        /// the current time.
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

        fn has_a_pane(&self) -> bool {
            self.server.pane_alive(&self.pane)
        }
    }

    /// Poll until `f` returns true, or panic after ten seconds.
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

    /// A state directory, with room beside it for the view's arrangement file.
    fn state_root(state: &TempDir) -> PathBuf {
        let root = state.path().join("agents");
        std::fs::create_dir_all(&root).expect("somewhere to keep the records");
        root
    }

    /// A record of an agent pointed at `pane`.
    fn record(root: &Path, id: &str, socket: Socket, pane: PaneId) -> Agent {
        Agent::create(
            root,
            &Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                model: None,
                effort: None,
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

    /// Run the verb with this `park_after` at `now`.
    fn considered(root: &Path, agent: &Agent, park_after: u64, now: u64) -> i32 {
        let meta = agent.meta().expect("the record");
        let_go(root, agent, &meta, park_after, now).expect("a decision")
    }

    /// The record's `parked_at` stamp and the kinds in its event log.
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
    fn park_finds_nothing_to_do_where_the_record_has_been_cleared() {
        // Cleared since the timer was set: exit 0, nothing printed.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        assert_eq!(
            consider(&root, "cleared-a1b", 4_600).expect("a cleared record is not a failure"),
            exit::OK
        );
    }

    #[test]
    fn park_still_says_so_where_the_record_cannot_be_read() {
        // A directory without a readable record is an error, unlike a cleared
        // one.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        std::fs::create_dir_all(root.join("broken-a1b")).unwrap();
        assert!(consider(&root, "broken-a1b", 4_600).is_err());
    }

    #[test]
    fn park_refuses_a_tmux_that_cannot_be_asked_and_stamps_nothing() {
        // Idle long enough, but tmux cannot say whether the pane exists.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let agent = record(
            &root,
            "fix-login-a1b",
            crate::tmux::unaskable(),
            PaneId::new("%3").unwrap(),
        );
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.since = 1_000;
            })
            .unwrap();
        let meta = agent.meta().unwrap();

        let why = let_go(&root, &agent, &meta, 3_600, 4_600).unwrap_err();

        assert!(
            format!("{why:#}").starts_with("tmux could not be asked: "),
            "{why:#}"
        );
        assert_eq!(left(&agent), (0, Vec::new()));
    }

    #[test]
    fn park_takes_the_pane_of_an_agent_that_has_sat_idle_long_enough() {
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        assert_eq!(considered(&root, &it.agent, 3_600, 4_600), exit::OK);
        assert!(!it.has_a_pane(), "the pane went with the vendor");

        // Only the stamp and the event are new; the stamp is what tells a
        // parked agent from a killed one.
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
        // Set an hour ago, but the agent has been sent work since.
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
        // `park_after = 0` turns parking off.
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

        // One second short, as when the timer was set over an earlier idle and
        // the agent went idle again since.
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
        // The pane was killed or its server died, so the agent ended and must
        // not be stamped as parked.
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
    fn park_leaves_a_pane_that_answers_for_another_agent_alone() {
        // The pane number was reused by another agent after this record's
        // server died. It is not this record's to end.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let today = Sitting::new(&root, "today-b2c");
        let yesterday = record(
            &root,
            "yesterday-a1b",
            today.server.socket().clone(),
            today.pane.clone(),
        );
        yesterday
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.since = 1_000;
            })
            .unwrap();

        assert_eq!(considered(&root, &yesterday, 3_600, 4_600), exit::OK);
        assert!(today.has_a_pane(), "it is the other agent's pane");
        assert_eq!(left(&yesterday), (0, Vec::new()));
    }

    #[test]
    fn park_leaves_a_pane_somebody_is_looking_at() {
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        // An attached client, counted by `session_attached`: a pane on another
        // server running a client of this one.
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

        // The arrangement file a closed view left, with the agent pinned.
        std::fs::write(
            crate::paths::view_file(&root).expect("somewhere to keep it"),
            br#"{"arrangement":{"held":["fix-login-a1b"]}}"#,
        )
        .unwrap();

        assert_eq!(considered(&root, &it.agent, 3_600, 4_600), exit::OK);
        assert!(it.has_a_pane(), "somebody wants it in front of them");
        assert_eq!(left(&it.agent), (0, Vec::new()));
    }

    #[test]
    fn park_decides_on_the_record_as_it_is_once_it_holds_the_writer() {
        // A send racing the timer. Park reads the record under the writer, so a
        // send that wins the lock is a turn park sees and leaves alone.
        let state = TempDir::new().unwrap();
        let root = state_root(&state);
        let it = Sitting::new(&root, "fix-login-a1b");
        it.recorded(Phase::Idle, 1_000);

        let writer = it.agent.writer().expect("the writer");
        let park = std::thread::spawn({
            let root = root.clone();
            let agent = Agent::open(&root, "fix-login-a1b").unwrap();
            move || considered(&root, &agent, 3_600, 4_600)
        });
        // Long enough for a park that locked first to have read.
        std::thread::sleep(Duration::from_millis(300));
        writer
            .observe(|state| {
                state.state = Phase::Working;
                state.since = 4_590;
            })
            .unwrap();
        drop(writer);

        assert_eq!(park.join().expect("the verb"), exit::OK);
        assert!(
            it.has_a_pane(),
            "it went back to work before park could act"
        );
        assert_eq!(left(&it.agent), (0, Vec::new()));
    }
}
