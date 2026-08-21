//! Forgetting agents that ended a long time ago.
//!
//! Records are how an agent's answer outlives its pane, so they are kept well
//! past the moment somebody stopped watching. What is swept is only what has
//! plainly been read or abandoned: an agent whose command ended, a week ago.
//!
//! A stopped agent is never swept. Somebody stopped it on purpose, and its
//! record is where the branch it left behind is named.
//!
//! This is the one thing a reader writes, and it happens on `ls` because that
//! is the command a person runs often and expects nothing of.

use anyhow::Result;
use std::path::Path;

use crate::store::{Agent, Phase};

/// How long a finished agent's record is kept.
pub const KEEP: u64 = 7 * 24 * 60 * 60;

/// Remove the records that have outlived their use, and answer with what was
/// removed.
pub fn sweep(root: &Path, now: u64) -> Result<Vec<String>> {
    let mut swept = Vec::new();
    for id in crate::store::list(root)? {
        let Ok(agent) = Agent::open(root, &id) else {
            continue;
        };
        let Ok(state) = agent.state() else { continue };

        if !matches!(state.state, Phase::Done | Phase::Failed) {
            continue;
        }
        if now.saturating_sub(state.last_event.max(state.since)) <= KEEP {
            continue;
        }
        // A record that will not go is not worth failing a listing over.
        if agent.remove().is_ok() {
            swept.push(id);
        }
    }
    swept.sort();
    Ok(swept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    const NOW: u64 = 1_800_000_000;

    fn record(root: &Path, id: &str, phase: Phase, last_event: u64) {
        let agent = Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
        )
        .unwrap();
        // Straight onto disk: the writer stamps `last_event` with now, and
        // these records are meant to be old.
        let state = State {
            state: phase,
            last_event,
            since: last_event,
            ..State::default()
        };
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn reader_forgets_an_agent_that_ended_a_week_ago() {
        let root = TempDir::new().unwrap();
        record(root.path(), "old-done-a1b", Phase::Done, NOW - KEEP - 1);
        record(root.path(), "old-failed-c3d", Phase::Failed, NOW - KEEP - 1);

        assert_eq!(
            sweep(root.path(), NOW).unwrap(),
            ["old-done-a1b", "old-failed-c3d"]
        );
        assert!(crate::store::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn reader_keeps_what_somebody_may_still_want() {
        let root = TempDir::new().unwrap();
        // Finished, but recently: its answer is the reason it is kept.
        record(root.path(), "just-done-a1b", Phase::Done, NOW - 60);
        // Stopped on purpose: its branch is named in the record.
        record(root.path(), "stopped-c3d", Phase::Stopped, NOW - KEEP - 1);
        // Still going, however old the last event is.
        record(root.path(), "working-e5f", Phase::Working, NOW - KEEP - 1);
        record(root.path(), "waiting-g7h", Phase::Waiting, NOW - KEEP - 1);

        assert!(sweep(root.path(), NOW).unwrap().is_empty());
        assert_eq!(crate::store::list(root.path()).unwrap().len(), 4);
    }
}
