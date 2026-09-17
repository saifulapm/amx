//! The command somebody asked to be run when an agent reaches a moment.
//!
//! Five keys, one per moment worth acting on — `on_waiting`, `on_idle`,
//! `on_done`, `on_failed`, `on_stopped` — and each holds a shell command. What
//! a person does with one is their own business: post to a chat, ring a bell,
//! start the next agent. amx knows only when to run it and what to tell it.
//!
//! This file assembles one; [`crate::notify::start`] runs it. Which amx runs
//! it is which amx wrote the phase — the hook that recorded the event, or the
//! `stop` that ended the agent — so a reading of a record never starts
//! anything, however far behind the record it finds itself.
//!
//! The moment is named twice over, because a command gets both roads: the
//! phase is in the environment as a word, and the event that moved the agent
//! arrives on stdin as the same JSON line the event log got.

use std::path::Path;

use crate::config::Config;
use crate::notify::Errand;
use crate::store::{Agent, Event, Meta, Phase};

/// The phase the agent reached, in the word `ls --json` prints.
pub const STATE_ENV: &str = "AMX_STATE";

/// Whether anybody was looking at the agent's pane when the moment arrived.
///
/// `1` or `0` from the hook, which has already asked tmux that question to
/// decide whether the notice was worth posting. Absent from `stop`, which asks
/// nothing about a pane it is closing: a command that cares can tell the two
/// apart, and one that does not reads either as the falsehood it is.
pub const WATCHED_ENV: &str = "AMX_WATCHED";

/// The errand this moment is worth, where somebody has written one.
///
/// The project's own file over the person's, the way every other key layers,
/// and asked of the project first: what to do when the work in a repository
/// reaches a moment is that repository's to say, and a person who has set the
/// key for everything still wanted it set for everything else.
///
/// One key is read off the project rather than the whole file laid up, because
/// the caller is a hook holding the person's config already — see
/// [`crate::config::project_key`].
pub fn assembled(
    config: &Config,
    agent: &Agent,
    meta: &Meta,
    phase: Phase,
    event: &Event,
) -> Option<Errand> {
    let (key, person) = match phase {
        Phase::Waiting => ("on_waiting", &config.on_waiting),
        Phase::Idle => ("on_idle", &config.on_idle),
        Phase::Done => ("on_done", &config.on_done),
        Phase::Failed => ("on_failed", &config.on_failed),
        Phase::Stopped => ("on_stopped", &config.on_stopped),
        // Starting, working and unknown are the agent on its way somewhere.
        // Nothing has arrived to tell anybody about.
        _ => return None,
    };

    // Where the agent works: its own tree where amx cut one, and else the
    // directory it was started in. The same directory the project's file is
    // looked up from, because the project an errand belongs to is the project
    // the work is in.
    let dir = meta.worktree.clone().unwrap_or_else(|| meta.dir.clone());
    let command = crate::config::project_key(&dir, key).or_else(|| person.clone())?;

    let mut env = surroundings(agent, meta);
    env.push((STATE_ENV.to_string(), phase.as_str().to_string()));

    let mut stdin = serde_json::to_vec(event).ok()?;
    stdin.push(b'\n');

    Some(Errand {
        command,
        dir,
        env,
        stdin,
    })
}

/// What a command run for an agent is told about the agent it was run for.
///
/// The one place those pairs are named, because two roads reach them: a moment
/// key assembles an errand above, and a key somebody bound in the view runs a
/// command in the same tree. Both are somebody's own command started off one
/// agent, and a person who learnt the words on one road should not find the
/// other spelling them differently.
///
/// The moment itself is not here. An errand is run because the agent reached
/// one; a key is pressed whenever somebody presses it, and there is no phase
/// the press is about.
pub fn surroundings(agent: &Agent, meta: &Meta) -> Vec<(String, String)> {
    let mut env = vec![
        (crate::hook::ID_ENV.to_string(), meta.id.clone()),
        (
            crate::spawn::RECORD_DIR_ENV.to_string(),
            spelled(agent.dir()),
        ),
    ];
    // A scratch directory that could not be made is one thing the command
    // cannot have; everything else it was going to be told still stands.
    if let Ok(scratch) = crate::spawn::scratch(agent.dir()) {
        env.push((crate::spawn::AGENT_DIR_ENV.to_string(), spelled(&scratch)));
    }
    if let Some(tree) = &meta.worktree {
        env.push((crate::worktree::WORKTREE_ENV.to_string(), spelled(tree)));
    }
    // The command is something an agent's moment started, and a claude run
    // from one would otherwise report its whole session under this agent's id.
    env.push((crate::hook::NESTED_ENV.to_string(), "1".to_string()));
    env
}

/// A path as a variable holds it.
fn spelled(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PaneId, Socket};
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A record on disk, and the meta that names it.
    fn agent(root: &Path, dir: &Path, worktree: Option<PathBuf>) -> (Agent, Meta) {
        let meta = Meta {
            role: None,
            parent: None,
            depth: 0,
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
            dir: dir.to_path_buf(),
            worktree,
            branch: None,
            base: None,
            socket: Socket::Name("amx".to_string()),
            pane: PaneId::new("%7").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        };
        (Agent::create(root, &meta).unwrap(), meta)
    }

    /// A config with one moment key set.
    fn says(key: &str, command: &str) -> Config {
        let mut config = Config::default();
        match key {
            "on_waiting" => config.on_waiting = Some(command.to_string()),
            "on_idle" => config.on_idle = Some(command.to_string()),
            "on_done" => config.on_done = Some(command.to_string()),
            "on_failed" => config.on_failed = Some(command.to_string()),
            "on_stopped" => config.on_stopped = Some(command.to_string()),
            _ => panic!("no key {key}"),
        }
        config
    }

    fn event() -> Event {
        Event::new("Notification", json!({ "message": "needs permission" }))
    }

    #[test]
    fn errand_the_key_is_the_moment_the_agent_reached() {
        let root = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (agent, meta) = agent(root.path(), work.path(), None);

        for (phase, key) in [
            (Phase::Waiting, "on_waiting"),
            (Phase::Idle, "on_idle"),
            (Phase::Done, "on_done"),
            (Phase::Failed, "on_failed"),
            (Phase::Stopped, "on_stopped"),
        ] {
            let config = says(key, key);
            let errand = assembled(&config, &agent, &meta, phase, &event())
                .unwrap_or_else(|| panic!("{key} is what {phase} runs"));
            assert_eq!(errand.command, key);

            // And the key is that moment's alone: no other phase reaches it.
            for other in [
                Phase::Waiting,
                Phase::Idle,
                Phase::Done,
                Phase::Failed,
                Phase::Stopped,
                Phase::Starting,
                Phase::Working,
                Phase::Unknown,
            ] {
                if other == phase {
                    continue;
                }
                assert_eq!(
                    assembled(&config, &agent, &meta, other, &event()),
                    None,
                    "{other} is not what {key} is about"
                );
            }
        }
    }

    #[test]
    fn errand_a_moment_nobody_wrote_a_command_for_runs_nothing() {
        let root = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (agent, meta) = agent(root.path(), work.path(), None);

        for phase in [
            Phase::Waiting,
            Phase::Idle,
            Phase::Done,
            Phase::Failed,
            Phase::Stopped,
        ] {
            assert_eq!(
                assembled(&Config::default(), &agent, &meta, phase, &event()),
                None,
                "{phase}"
            );
        }
    }

    #[test]
    fn errand_the_project_answers_the_moment_over_the_person() {
        // What to run when the work in a repository reaches a moment is that
        // repository's to say. The person's key stands for every project that
        // has not said otherwise.
        let root = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        std::fs::create_dir_all(work.path().join(".amx")).unwrap();
        std::fs::write(
            work.path().join(".amx/config.toml"),
            "on_done = \"the project's own\"\n",
        )
        .unwrap();
        let (agent, meta) = agent(root.path(), work.path(), None);

        let mut config = says("on_done", "the person's");
        config.on_failed = Some("the person's, for a failure".to_string());

        assert_eq!(
            assembled(&config, &agent, &meta, Phase::Done, &event())
                .unwrap()
                .command,
            "the project's own"
        );
        assert_eq!(
            assembled(&config, &agent, &meta, Phase::Failed, &event())
                .unwrap()
                .command,
            "the person's, for a failure",
            "a key the project left alone is still the person's"
        );
    }

    #[test]
    fn errand_carries_the_record_the_tree_and_the_event() {
        let root = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let (agent, meta) = agent(root.path(), work.path(), Some(tree.path().to_path_buf()));

        let event = event();
        let errand = assembled(
            &says("on_idle", "say-so"),
            &agent,
            &meta,
            Phase::Idle,
            &event,
        )
        .unwrap();

        // It runs where the agent works, which is the tree amx cut for it.
        assert_eq!(errand.dir, tree.path());

        let env: std::collections::HashMap<&str, &str> = errand
            .env
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        assert_eq!(env[crate::hook::ID_ENV], "fix-login-a1b");
        assert_eq!(env[STATE_ENV], "idle");
        assert_eq!(env[crate::spawn::RECORD_DIR_ENV], spelled(agent.dir()));
        assert_eq!(
            env[crate::spawn::AGENT_DIR_ENV],
            spelled(&crate::spawn::scratch(agent.dir()).unwrap()),
            "the scratch directory, which is beside the record and not it"
        );
        assert_eq!(env[crate::worktree::WORKTREE_ENV], spelled(tree.path()));
        assert_eq!(env[crate::hook::NESTED_ENV], "1");
        assert_eq!(
            env.get(WATCHED_ENV),
            None,
            "who is looking is the starter's to add"
        );

        // One JSON line, the same line the event log got.
        let line = String::from_utf8(errand.stdin).unwrap();
        assert!(line.ends_with('\n'), "{line}");
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        assert_eq!(
            serde_json::from_str::<Event>(&line).unwrap(),
            event,
            "at, kind and payload, whole"
        );
    }

    #[test]
    fn errand_without_a_tree_runs_where_the_agent_was_started() {
        let root = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let (agent, meta) = agent(root.path(), work.path(), None);

        let errand = assembled(
            &says("on_waiting", "say-so"),
            &agent,
            &meta,
            Phase::Waiting,
            &event(),
        )
        .unwrap();
        assert_eq!(errand.dir, work.path());
        assert!(
            !errand
                .env
                .iter()
                .any(|(name, _)| name == crate::worktree::WORKTREE_ENV),
            "there is no tree to name"
        );
    }
}
