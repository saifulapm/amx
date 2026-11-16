//! Removal and archiving of records for agents that ended long ago.
//!
//! Only `done` and `failed` records older than [`KEEP`] are removed. A stopped
//! agent is kept, since its record names the branch it left behind, and so is
//! any record whose worktree still exists. A stopped or parked record idle for
//! [`ARCHIVE_AFTER`] moves to the archive, which the readers skip and `amx
//! resume` brings it back from. Nothing archived is deleted. The pass runs on
//! `ls`.

use crate::derive::Record;
use crate::store::{Agent, Meta, Phase, State};
use crate::tmux::Server;

/// How long a finished agent's record is kept.
pub const KEEP: u64 = 7 * 24 * 60 * 60;

/// How long a stopped or parked agent's record stays among the live ones.
pub const ARCHIVE_AFTER: u64 = 14 * 24 * 60 * 60;

/// Remove the records past keeping, archive the ones past
/// [`ARCHIVE_AFTER`], and return the rest.
///
/// Candidates are picked from the listing's own reading, so most records are
/// read once. Each candidate is checked again under its writer lock before it
/// is removed or moved, because the reading may predate a resume.
pub fn sweep(records: Vec<Record>, now: u64) -> Vec<Record> {
    let mut kept = Vec::new();
    for record in records {
        // A record that fails to go is kept in the listing.
        let gone = (past_keeping(&record.state, now)
            && !holding(&record.meta)
            && under_lock(&record, now, forgettable, Agent::remove))
            || (past_live(&record.state, now)
                && under_lock(
                    &record,
                    now,
                    |_, state, now| past_live(state, now),
                    Agent::archive,
                ));
        if !gone {
            kept.push(record);
        }
    }
    kept
}

/// Whether a record read again under its lock may still be removed.
fn forgettable(meta: &Meta, state: &State, now: u64) -> bool {
    past_keeping(state, now) && !holding(meta)
}

/// Take one record away with `take` if `still` holds on a fresh reading.
///
/// The writer lock is held until the record is gone. `resume` takes the same
/// lock, so it either finished first and the record reads as running, or it
/// waits and finds no record where it looked. A record whose pane still
/// answers is kept whatever its phase, and so is one whose tmux cannot be
/// asked.
fn under_lock(
    record: &Record,
    now: u64,
    still: impl Fn(&Meta, &State, u64) -> bool,
    take: impl Fn(&Agent) -> anyhow::Result<()>,
) -> bool {
    let Ok(_writer) = record.agent.writer() else {
        return false;
    };
    let (Ok(meta), Ok(state)) = (record.agent.meta(), record.agent.state()) else {
        return false;
    };
    still(&meta, &state, now)
        // A tmux that cannot be asked may hold a live pane, so it keeps the
        // record.
        && matches!(
            Server::from_socket(meta.socket.clone()).answers_for_now(&meta.pane, &meta.id),
            Ok(false)
        )
        && take(&record.agent).is_ok()
}

/// Whether a record's worktree still exists. The record is the only thing
/// that names the tree and its branch, so it is kept until the tree is gone.
fn holding(meta: &Meta) -> bool {
    meta.worktree.as_ref().is_some_and(|tree| tree.exists())
}

/// Whether a record is a finished command older than [`KEEP`].
fn past_keeping(state: &State, now: u64) -> bool {
    matches!(state.state, Phase::Done | Phase::Failed) && idle_for(state, now) > KEEP
}

/// Whether a record is a stopped or parked agent idle for more than
/// [`ARCHIVE_AFTER`]. An idle agent whose pane was never let go is at its
/// prompt, and is not archived.
fn past_live(state: &State, now: u64) -> bool {
    let resting = match state.state {
        Phase::Stopped => true,
        Phase::Idle => state.parked_at != 0,
        _ => false,
    };
    resting && idle_for(state, now) > ARCHIVE_AFTER
}

/// How long since the record last heard anything. A record dated after `now`,
/// as under a clock set back, has been idle for no time at all.
fn idle_for(state: &State, now: u64) -> u64 {
    now.saturating_sub(state.last_event.max(state.since))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{PaneId, Server, Socket, Spawn};
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const NOW: u64 = 1_800_000_000;
    const DAY: u64 = 24 * 60 * 60;

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
    fn a_tmux_that_cannot_be_asked_keeps_its_record() {
        // A socket path longer than a unix socket can name: tmux cannot say
        // whether a pane there still answers, so nothing is taken.
        let root = TempDir::new().unwrap();
        let away = TempDir::new().unwrap();
        let unusable = away.path().join("s".repeat(120));
        let nobody = away.path().join("nobody-listens");
        for (id, socket) in [("unaskable-a1b", &unusable), ("no-server-c3d", &nobody)] {
            record(root.path(), id, Phase::Done, NOW - KEEP - 1);
            let agent = Agent::open(root.path(), id).unwrap();
            agent
                .writer()
                .unwrap()
                .update_meta(|meta| meta.socket = Socket::Path(socket.clone()))
                .unwrap();
            wrote(&agent, Phase::Done, NOW - KEEP - 1);
        }

        assert_eq!(ids(&sweep(read(root.path()), NOW)), ["unaskable-a1b"]);
        assert_eq!(
            left(root.path()),
            ["unaskable-a1b"],
            "and the one with no server listening goes, as before"
        );
    }

    fn parked(root: &Path, id: &str, last_event: u64) {
        record(root, id, Phase::Idle, last_event);
        let agent = Agent::open(root, id).unwrap();
        let state = State {
            state: Phase::Idle,
            last_event,
            since: last_event,
            parked_at: last_event,
            ..State::default()
        };
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();
    }

    fn archive(root: &Path) -> Vec<String> {
        let mut ids = crate::store::list(&root.join(crate::store::ARCHIVE)).unwrap();
        ids.sort();
        ids
    }

    #[test]
    fn a_stopped_or_parked_record_idle_for_two_weeks_moves_to_the_archive() {
        let root = TempDir::new().unwrap();
        record(
            root.path(),
            "old-stopped-a1b",
            Phase::Stopped,
            NOW - 15 * DAY,
        );
        parked(root.path(), "old-parked-c3d", NOW - 15 * DAY);
        record(
            root.path(),
            "young-stopped-e5f",
            Phase::Stopped,
            NOW - 13 * DAY,
        );
        parked(root.path(), "young-parked-g7h", NOW - 13 * DAY);

        assert_eq!(
            ids(&sweep(read(root.path()), NOW)),
            ["young-parked-g7h", "young-stopped-e5f"]
        );
        assert_eq!(left(root.path()), ["young-parked-g7h", "young-stopped-e5f"]);
        assert_eq!(archive(root.path()), ["old-parked-c3d", "old-stopped-a1b"]);
        assert!(
            root.path()
                .join(".archive/old-stopped-a1b/meta.json")
                .is_file(),
            "the record moved whole"
        );
    }

    #[test]
    fn a_record_still_in_use_is_never_archived_however_old() {
        let root = TempDir::new().unwrap();
        record(root.path(), "working-a1b", Phase::Working, NOW - 30 * DAY);
        record(root.path(), "waiting-c3d", Phase::Waiting, NOW - 30 * DAY);
        record(root.path(), "limited-e5f", Phase::Limited, NOW - 30 * DAY);
        // Idle with its pane kept is an agent at its prompt, not a parked one.
        record(root.path(), "idle-g7h", Phase::Idle, NOW - 30 * DAY);
        record(root.path(), "starting-i9j", Phase::Starting, NOW - 30 * DAY);

        assert_eq!(sweep(read(root.path()), NOW).len(), 5);
        assert!(archive(root.path()).is_empty());
    }

    #[test]
    fn a_record_dated_after_now_is_not_archived() {
        // A clock set back leaves a record's last event in the future.
        let root = TempDir::new().unwrap();
        record(root.path(), "stopped-a1b", Phase::Stopped, NOW + 30 * DAY);
        parked(root.path(), "parked-c3d", NOW + 30 * DAY);

        assert_eq!(sweep(read(root.path()), NOW).len(), 2);
        assert!(archive(root.path()).is_empty());
    }

    #[test]
    fn a_gc_pass_moves_records_and_loses_none() {
        let root = TempDir::new().unwrap();
        for n in 0..6 {
            let age = if n % 2 == 0 { 20 * DAY } else { DAY };
            record(
                root.path(),
                &format!("stopped-{n}"),
                Phase::Stopped,
                NOW - age,
            );
            parked(root.path(), &format!("parked-{n}"), NOW - age);
        }
        let count = |root: &Path| left(root).len() + archive(root).len();
        let before = count(root.path());

        sweep(read(root.path()), NOW);
        assert_eq!(archive(root.path()).len(), 6);
        assert_eq!(count(root.path()), before);
        // A second pass finds nothing more to move.
        sweep(read(root.path()), NOW);
        assert_eq!(count(root.path()), before);
        assert_eq!(archive(root.path()).len(), 6);
    }

    #[test]
    fn the_archive_checks_the_disk_again_before_it_moves_a_record() {
        // The listing read the record stopped long ago; it was resumed since.
        let root = TempDir::new().unwrap();
        record(root.path(), "stopped-a1b", Phase::Stopped, NOW - 20 * DAY);
        let records = read(root.path());
        wrote(
            &Agent::open(root.path(), "stopped-a1b").unwrap(),
            Phase::Working,
            NOW,
        );

        assert_eq!(ids(&sweep(records, NOW)), ["stopped-a1b"]);
        assert_eq!(left(root.path()), ["stopped-a1b"]);
        assert!(archive(root.path()).is_empty());
    }

    #[test]
    fn a_stopped_record_whose_pane_still_answers_is_not_archived() {
        // The record says stopped, and the screen disagrees: the pane is
        // there and answers for the agent, so it stays among the live ones.
        let server = Server::named(format!("amx-test-gc-arch-{}", std::process::id()))
            .with_conf("/dev/null");
        let (_, pane) = server
            .new_session(&Spawn {
                name: Some("amx-stopped-a1b"),
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .unwrap();
        let root = TempDir::new().unwrap();
        record(root.path(), "stopped-a1b", Phase::Stopped, NOW - 20 * DAY);
        let agent = Agent::open(root.path(), "stopped-a1b").unwrap();
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| {
                meta.socket = server.socket().clone();
                meta.pane = pane.clone();
            })
            .unwrap();
        wrote(&agent, Phase::Stopped, NOW - 20 * DAY);

        let kept = sweep(read(root.path()), NOW);
        let _ = server.kill();
        assert_eq!(ids(&kept), ["stopped-a1b"]);
        assert!(archive(root.path()).is_empty());
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
