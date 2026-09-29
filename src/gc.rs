//! Removal of records for agents that ended long ago.
//!
//! Only `done` and `failed` records older than [`KEEP`] are swept. A stopped
//! agent is kept, since its record names the branch it left behind, and so is
//! any record whose worktree still exists. The sweep runs on `ls`.

use crate::derive::Record;
use crate::store::{Meta, Phase, State};
use crate::tmux::Server;

/// How long a finished agent's record is kept.
pub const KEEP: u64 = 7 * 24 * 60 * 60;

/// Remove the records past keeping and return the rest.
///
/// Candidates are picked from the listing's own reading, so most records are
/// read once. Each candidate is checked again under its writer lock before it
/// is removed, because the reading may predate a resume.
pub fn sweep(records: Vec<Record>, now: u64) -> Vec<Record> {
    let mut kept = Vec::new();
    for record in records {
        // A record that fails to go is kept in the listing.
        if !past_keeping(&record.state, now) || holding(&record.meta) || !forgot(&record, now) {
            kept.push(record);
        }
    }
    kept
}

/// Remove one record if it is still past keeping.
///
/// The writer lock is held until the record is gone. `resume` takes the same
/// lock, so it either finished first and the record reads as running, or it
/// waits and finds no record. A record whose pane still answers is kept
/// whatever its phase.
fn forgot(record: &Record, now: u64) -> bool {
    let Ok(_writer) = record.agent.writer() else {
        return false;
    };
    let (Ok(meta), Ok(state)) = (record.agent.meta(), record.agent.state()) else {
        return false;
    };
    past_keeping(&state, now)
        && !holding(&meta)
        && !Server::from_socket(meta.socket.clone()).pane_answers_for(&meta.pane, &meta.id)
        && record.agent.remove().is_ok()
}

/// Whether a record's worktree still exists. The record is the only thing
/// that names the tree and its branch, so it is kept until the tree is gone.
fn holding(meta: &Meta) -> bool {
    meta.worktree.as_ref().is_some_and(|tree| tree.exists())
}

/// Whether a record is a finished command older than [`KEEP`].
fn past_keeping(state: &State, now: u64) -> bool {
    matches!(state.state, Phase::Done | Phase::Failed)
        && now.saturating_sub(state.last_event.max(state.since)) > KEEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Agent;
    use crate::tmux::{PaneId, Server, Socket, Spawn};
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const NOW: u64 = 1_800_000_000;

    fn record(root: &Path, id: &str, phase: Phase, last_event: u64) {
        let agent = Agent::create(
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

    /// Write the state directly: the writer would stamp `last_event` with now,
    /// and these records must be old.
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

    /// The records a listing holds when it sweeps.
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

        // The kept records are returned, and the rest are gone from disk.
        assert_eq!(ids(&sweep(read(root.path()), NOW)), ["just-done-e5f"]);
        assert_eq!(left(root.path()), ["just-done-e5f"]);
    }

    #[test]
    fn a_kept_tree_keeps_its_record() {
        // A week-old finished record whose tree still exists is kept.
        let root = TempDir::new().unwrap();
        let tree = root.path().join("repo/.amx/worktrees/old-done-a1b");
        std::fs::create_dir_all(&tree).unwrap();
        record(root.path(), "old-done-a1b", Phase::Done, NOW - KEEP - 1);
        let agent = Agent::open(root.path(), "old-done-a1b").unwrap();
        let writer = agent.writer().unwrap();
        writer
            .update_meta(|meta| meta.worktree = Some(tree.clone()))
            .unwrap();
        drop(writer);
        wrote(&agent, Phase::Done, NOW - KEEP - 1);

        assert_eq!(ids(&sweep(read(root.path()), NOW)), ["old-done-a1b"]);

        // Once the tree is gone, the record may go.
        std::fs::remove_dir_all(&tree).unwrap();
        assert!(sweep(read(root.path()), NOW).is_empty());
    }

    #[test]
    fn reader_keeps_what_somebody_may_still_want() {
        let root = TempDir::new().unwrap();
        // Finished recently.
        record(root.path(), "just-done-a1b", Phase::Done, NOW - 60);
        // Stopped: the record names its branch.
        record(root.path(), "stopped-c3d", Phase::Stopped, NOW - KEEP - 1);
        // Still running, however old the last event.
        record(root.path(), "working-e5f", Phase::Working, NOW - KEEP - 1);
        record(root.path(), "waiting-g7h", Phase::Waiting, NOW - KEEP - 1);

        assert_eq!(sweep(read(root.path()), NOW).len(), 4);
        assert_eq!(left(root.path()).len(), 4);
    }

    #[test]
    fn reader_picks_from_the_records_it_was_handed_and_asks_again_before_it_takes() {
        // Each record's reading disagrees with the disk. The reading picks
        // candidates, so a record that aged since is left for the next
        // listing; the disk decides removal, so a record resumed since is
        // kept.
        let root = TempDir::new().unwrap();
        record(root.path(), "old-done-a1b", Phase::Done, NOW - KEEP - 1);
        record(root.path(), "just-done-c3d", Phase::Done, NOW - 60);
        let records = read(root.path());

        let agent = |id| Agent::open(root.path(), id).unwrap();
        wrote(&agent("old-done-a1b"), Phase::Working, NOW);
        wrote(&agent("just-done-c3d"), Phase::Done, NOW - KEEP - 1);

        assert_eq!(ids(&sweep(records, NOW)), ["just-done-c3d", "old-done-a1b"]);
        assert_eq!(left(root.path()), ["just-done-c3d", "old-done-a1b"]);
    }

    #[test]
    fn a_pane_that_still_answers_keeps_its_record() {
        // A week-old finished record whose pane still exists, on a private
        // tmux server.
        let server =
            Server::named(format!("amx-test-gc-{}", std::process::id())).with_conf("/dev/null");
        let (_, pane) = server
            .new_session(&Spawn {
                name: Some("amx-old-done-a1b"),
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .unwrap();
        let root = TempDir::new().unwrap();
        record(root.path(), "old-done-a1b", Phase::Done, NOW - KEEP - 1);
        let agent = Agent::open(root.path(), "old-done-a1b").unwrap();
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| {
                meta.socket = server.socket().clone();
                meta.pane = pane.clone();
            })
            .unwrap();
        wrote(&agent, Phase::Done, NOW - KEEP - 1);

        let kept = sweep(read(root.path()), NOW);
        let _ = server.kill();
        assert_eq!(ids(&kept), ["old-done-a1b"]);
        assert_eq!(left(root.path()), ["old-done-a1b"]);
    }
}
