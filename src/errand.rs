//! Commands a person configured to run when an agent reaches a moment.
//!
//! The keys are `on_waiting`, `on_idle`, `on_done`, `on_failed` and
//! `on_stopped`, each a shell command. This module assembles the command;
//! [`crate::notify::start`] runs it. Only the process that wrote the phase
//! (the hook, or `stop`) starts one, so reading a record never does.
//!
//! The command gets the phase in [`STATE_ENV`] and the event that caused it
//! on stdin, as the same JSON line the event log holds.

use std::path::Path;

use crate::config::Config;
use crate::notify::Errand;
use crate::store::{Agent, Event, Meta, Phase};

/// The phase the agent reached, as `ls --json` spells it.
pub const STATE_ENV: &str = "AMX_STATE";

/// Whether the agent's pane was being watched when the moment arrived.
///
/// `1` or `0` when the hook starts the command. Unset when `stop` does, since
/// it does not ask about a pane it is closing.
pub const WATCHED_ENV: &str = "AMX_WATCHED";

/// The errand for this moment, if a command is configured for it.
///
/// The project's key wins over the person's. Only that one key is read from
/// the project file, since the hook already holds the person's config; see
/// [`crate::config::project_key_in`].
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
        // The agent is on its way somewhere; nothing to report.
        _ => return None,
    };

    // The agent's worktree if it has one, else its start directory. The
    // project config is looked up from the same place.
    let dir = meta.worktree.clone().unwrap_or_else(|| meta.dir.clone());
    // Consent is read under the state root this record lives in.
    let root = agent.dir().parent().unwrap_or(agent.dir());
    let command = crate::config::project_key_in(&dir, key, root).or_else(|| person.clone())?;

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

/// Environment for a command run on behalf of an agent.
///
/// Shared by moment errands and view key bindings so both spell the variables
/// the same way. The phase is not included: a key press has none.
pub fn surroundings(agent: &Agent, meta: &Meta) -> Vec<(String, String)> {
    let mut env = vec![
        (crate::hook::ID_ENV.to_string(), meta.id.clone()),
        (
            crate::spawn::RECORD_DIR_ENV.to_string(),
            spelled(agent.dir()),
        ),
    ];
    // A missing scratch directory drops only that variable.
    if let Ok(scratch) = crate::spawn::scratch(agent.dir()) {
        env.push((crate::spawn::AGENT_DIR_ENV.to_string(), spelled(&scratch)));
    }
    if let Some(tree) = &meta.worktree {
        env.push((crate::worktree::WORKTREE_ENV.to_string(), spelled(tree)));
    }
    // Keep a claude started by the command from reporting under this agent.
    env.push((crate::hook::NESTED_ENV.to_string(), "1".to_string()));
    env
}

/// A path as an environment variable value.
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

    /// A record on disk and its meta.
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

            // No other phase reaches this key.
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
        // The project's key wins; the person's key covers the rest.
        let root = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        std::fs::create_dir_all(work.path().join(".amx")).unwrap();
        std::fs::write(
            work.path().join(".amx/config.toml"),
            "on_done = \"the project's own\"\n",
        )
        .unwrap();
        let (agent, meta) = agent(root.path(), work.path(), None);
        let file = crate::paths::project_config(work.path()).unwrap();
        let mut config = says("on_done", "the person's");

        // An unallowed project file is ignored.
        assert_eq!(
            assembled(&config, &agent, &meta, Phase::Done, &event())
                .unwrap()
                .command,
            "the person's"
        );
        crate::consent::allow_in(root.path(), &file).unwrap();

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

        // It runs in the agent's worktree.
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
