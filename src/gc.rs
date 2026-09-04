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

use crate::derive::Record;
use crate::store::{Phase, State};

/// How long a finished agent's record is kept.
pub const KEEP: u64 = 7 * 24 * 60 * 60;

/// Remove the records that have outlived their use, and answer with the ones
/// that are left.
///
/// Over a reading somebody has already taken. Whether a record is worth keeping
/// is the phase on it and when it was last heard from, which the listing has
/// read the document for anyway — so the sweep opens nothing and parses
/// nothing, and one `ls` is one reading of each record rather than two.
pub fn sweep(records: Vec<Record>, now: u64) -> Vec<Record> {
    let mut kept = Vec::new();
    for record in records {
        // A record that will not go is not worth failing a listing over, and
        // it is still on the disk to be listed.
        if !past_keeping(&record.state, now) || record.agent.remove().is_err() {
            kept.push(record);
        }
    }
    kept
}

/// Whether a record has outlived its use: a command that ended, long enough ago
/// that whatever it answered has been read or abandoned.
fn past_keeping(state: &State, now: u64) -> bool {
    matches!(state.state, Phase::Done | Phase::Failed)
        && now.saturating_sub(state.last_event.max(state.since)) > KEEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Agent, Meta};
    use crate::tmux::{PaneId, Socket};
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const NOW: u64 = 1_800_000_000;

    fn record(root: &Path, id: &str, phase: Phase, last_event: u64) {
        let agent = Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
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
        wrote(&agent, phase, last_event);
    }

    /// Straight onto disk: the writer stamps `last_event` with now, and these
    /// records are meant to be old.
    fn wrote(agent: &Agent, phase: Phase, last_event: u64) {
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

    /// The records a listing has in hand by the time it sweeps.
    fn read(root: &Path) -> Vec<Record> {
        crate::derive::records(root).expect("the records")
    }

    fn ids(records: &[Record]) -> Vec<String> {
        let mut ids: Vec<String> = records
            .iter()
            .map(|record| record.meta.id.clone())
            .collect();
        ids.sort();
        ids
    }

    fn left(root: &Path) -> Vec<String> {
        let mut ids = crate::store::list(root).unwrap();
        ids.sort();
        ids
    }

    #[test]
    fn reader_forgets_an_agent_that_ended_a_week_ago() {
        let root = TempDir::new().unwrap();
        record(root.path(), "old-done-a1b", Phase::Done, NOW - KEEP - 1);
        record(root.path(), "old-failed-c3d", Phase::Failed, NOW - KEEP - 1);
        record(root.path(), "just-done-e5f", Phase::Done, NOW - 60);

        // What comes back is what the listing goes on to answer with, and the
        // records are gone from the disk with it.
        assert_eq!(ids(&sweep(read(root.path()), NOW)), ["just-done-e5f"]);
        assert_eq!(left(root.path()), ["just-done-e5f"]);
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

        assert_eq!(sweep(read(root.path()), NOW).len(), 4);
        assert_eq!(left(root.path()).len(), 4);
    }

    #[test]
    fn reader_sweeps_the_records_it_was_handed_and_reads_none_of_its_own() {
        // The state document each record was read from says the opposite of
        // what is on the disk now. A sweep that opened the file again would
        // answer the other way round on both of them.
        let root = TempDir::new().unwrap();
        record(root.path(), "old-done-a1b", Phase::Done, NOW - KEEP - 1);
        record(root.path(), "just-done-c3d", Phase::Done, NOW - 60);
        let records = read(root.path());

        let agent = |id| Agent::open(root.path(), id).unwrap();
        wrote(&agent("old-done-a1b"), Phase::Working, NOW);
        wrote(&agent("just-done-c3d"), Phase::Done, NOW - KEEP - 1);

        assert_eq!(ids(&sweep(records, NOW)), ["just-done-c3d"]);
        assert_eq!(left(root.path()), ["just-done-c3d"]);
    }
}
