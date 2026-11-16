//! Removal and archiving of records for agents that ended long ago.
//!
//! Only `done` and `failed` records older than [`KEEP`] are removed. A stopped
//! agent is kept, since its record names the branch it left behind, and so is
//! any record whose worktree still exists. A record with no pane (stopped,
//! parked, or left by a reboot) idle for [`ARCHIVE_AFTER`] moves to the
//! archive, which the readers skip and `amx resume` brings it back from.
//! Nothing archived is deleted. The pass runs on `ls`.

use std::path::Path;

use crate::derive::{Record, View};
use crate::store::{Agent, Meta, Phase, State};
use crate::tmux::Server;

/// How long a finished agent's record is kept.
pub const KEEP: u64 = 7 * 24 * 60 * 60;

/// How long a stopped or parked agent's record stays among the live ones.
pub const ARCHIVE_AFTER: u64 = 14 * 24 * 60 * 60;

/// Remove the records past keeping and return the rest.
///
/// Candidates are picked from the listing's own reading, so most records are
/// read once. Each candidate is checked again under its writer lock before it
/// is removed, because the reading may predate a resume.
pub fn sweep(records: Vec<Record>, now: u64) -> Vec<Record> {
    let mut kept = Vec::new();
    for record in records {
        // A record that fails to go is kept in the listing.
        let gone = past_keeping(&record.state, now)
            && !holding(&record.meta)
            && under_lock(&record.agent, now, forgettable, Agent::remove);
        if !gone {
            kept.push(record);
        }
    }
    kept
}

/// Archive the agents a reading found without a pane for more than
/// [`ARCHIVE_AFTER`], and return the rest.
///
/// The reading's phase picks them, since only the pane tells a record left
/// working by a reboot from one at work, and the reading asked tmux once for
/// every record. Each is checked again under its writer lock before it moves.
pub fn archive(root: &Path, views: Vec<View>, now: u64) -> Vec<View> {
    views
        .into_iter()
        .filter(|view| {
            // A record that fails to move is kept in the listing.
            !(shelvable(view, now)
                && Agent::open(root, view.id()).is_ok_and(|agent| {
                    under_lock(
                        &agent,
                        now,
                        |_, state, now| past_live(state, now),
                        Agent::archive,
                    )
                }))
        })
        .collect()
}

/// Whether a reading shows an agent with no pane, stopped or parked, idle for
/// more than [`ARCHIVE_AFTER`].
fn shelvable(view: &View, now: u64) -> bool {
    let resting = match view.phase() {
        Phase::Stopped => true,
        Phase::Idle => view.state.parked_at != 0,
        _ => false,
    };
    resting && past_live(&view.state, now)
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
    agent: &Agent,
    now: u64,
    still: impl Fn(&Meta, &State, u64) -> bool,
    take: impl Fn(&Agent) -> anyhow::Result<()>,
) -> bool {
    let Ok(_writer) = agent.writer() else {
        return false;
    };
    let (Ok(meta), Ok(state)) = (agent.meta(), agent.state()) else {
        return false;
    };
    still(&meta, &state, now)
        // A tmux that cannot be asked may hold a live pane, so it keeps the
        // record.
        && matches!(
            Server::from_socket(meta.socket.clone()).answers_for_now(&meta.pane, &meta.id),
            Ok(false)
        )
        && take(agent).is_ok()
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

/// Whether a record may be an agent with no pane, idle for more than
/// [`ARCHIVE_AFTER`]: stopped, parked, or one whose pane went whatever its
/// record last said, as after a reboot. Only the pane tells, so
/// [`under_lock`] asks it; an agent whose pane answers is in use. A finished
/// command is left to [`KEEP`], and a record with no date at all, whose age
/// is unknown, stays.
fn past_live(state: &State, now: u64) -> bool {
    !matches!(state.state, Phase::Done | Phase::Failed)
        && state.last_event.max(state.since) > 0
        && idle_for(state, now) > ARCHIVE_AFTER
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

    /// What a listing's pass leaves listed: the sweep, the reading, then the
    /// archive.
    fn pass(root: &Path, records: Vec<Record>) -> Vec<String> {
        let views =
            crate::derive::views_of(root, sweep(records, NOW), NOW, crate::derive::Wants::Card);
        let mut ids: Vec<String> = archive(root, views, NOW)
            .iter()
            .map(|view| view.id().to_string())
            .collect();
        ids.sort();
        ids
    }

    fn shelved(root: &Path) -> Vec<String> {
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
            pass(root.path(), read(root.path())),
            ["young-parked-g7h", "young-stopped-e5f"]
        );
        assert_eq!(left(root.path()), ["young-parked-g7h", "young-stopped-e5f"]);
        assert_eq!(shelved(root.path()), ["old-parked-c3d", "old-stopped-a1b"]);
        assert!(
            root.path()
                .join(".archive/old-stopped-a1b/meta.json")
                .is_file(),
            "the record moved whole"
        );
    }

    #[test]
    fn a_record_still_in_use_is_never_archived_however_old() {
        // Each pane still answers for its agent, so each is in use, whatever
        // its record last said.
        let server =
            Server::named(format!("amx-test-gc-use-{}", std::process::id())).with_conf("/dev/null");
        let root = TempDir::new().unwrap();
        let phases = [
            ("working-a1b", Phase::Working),
            ("waiting-c3d", Phase::Waiting),
            ("limited-e5f", Phase::Limited),
            ("idle-g7h", Phase::Idle),
            ("starting-i9j", Phase::Starting),
        ];
        for (id, phase) in phases {
            let name = format!("amx-{id}");
            let (_, pane) = server
                .new_session(&Spawn {
                    name: Some(&name),
                    command: &["sh", "-c", "while :; do sleep 0.05; done"],
                    ..Spawn::default()
                })
                .unwrap();
            record(root.path(), id, phase, NOW - 30 * DAY);
            let agent = Agent::open(root.path(), id).unwrap();
            agent
                .writer()
                .unwrap()
                .update_meta(|meta| {
                    meta.socket = server.socket().clone();
                    meta.pane = pane.clone();
                })
                .unwrap();
            wrote(&agent, phase, NOW - 30 * DAY);
        }

        let kept = pass(root.path(), read(root.path()));
        let _ = server.kill();
        assert_eq!(kept.len(), 5);
        assert!(shelved(root.path()).is_empty());
    }

    #[test]
    fn a_record_whose_pane_went_is_archived_whatever_it_last_said() {
        // A tmux killed or a machine rebooted leaves records that last said
        // working, waiting or idle, with no pane: they read as stopped.
        let root = TempDir::new().unwrap();
        record(root.path(), "working-a1b", Phase::Working, NOW - 20 * DAY);
        record(root.path(), "waiting-c3d", Phase::Waiting, NOW - 20 * DAY);
        record(root.path(), "idle-e5f", Phase::Idle, NOW - 20 * DAY);
        record(root.path(), "unknown-g7h", Phase::Unknown, NOW - 20 * DAY);
        // Still a finished command: the week's sweep is what takes it.
        let tree = root.path().join("repo/.amx/worktrees/done-i9j");
        std::fs::create_dir_all(&tree).unwrap();
        record(root.path(), "done-i9j", Phase::Done, NOW - 20 * DAY);
        let agent = Agent::open(root.path(), "done-i9j").unwrap();
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| meta.worktree = Some(tree.clone()))
            .unwrap();
        wrote(&agent, Phase::Done, NOW - 20 * DAY);

        assert_eq!(pass(root.path(), read(root.path())), ["done-i9j"]);
        assert_eq!(
            shelved(root.path()),
            ["idle-e5f", "unknown-g7h", "waiting-c3d", "working-a1b"]
        );
    }

    #[test]
    fn a_record_dated_after_now_is_not_archived() {
        // A clock set back leaves a record's last event in the future.
        let root = TempDir::new().unwrap();
        record(root.path(), "stopped-a1b", Phase::Stopped, NOW + 30 * DAY);
        parked(root.path(), "parked-c3d", NOW + 30 * DAY);

        assert_eq!(pass(root.path(), read(root.path())).len(), 2);
        assert!(shelved(root.path()).is_empty());
    }

    #[test]
    fn a_record_with_no_date_is_not_archived() {
        // Neither a last event nor a phase start: its age is unknown.
        let root = TempDir::new().unwrap();
        record(root.path(), "stopped-a1b", Phase::Stopped, 0);

        assert_eq!(pass(root.path(), read(root.path())), ["stopped-a1b"]);
        assert!(shelved(root.path()).is_empty());
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
        let count = |root: &Path| left(root).len() + shelved(root).len();
        let before = count(root.path());

        pass(root.path(), read(root.path()));
        assert_eq!(shelved(root.path()).len(), 6);
        assert_eq!(count(root.path()), before);
        // A second pass finds nothing more to move.
        pass(root.path(), read(root.path()));
        assert_eq!(count(root.path()), before);
        assert_eq!(shelved(root.path()).len(), 6);
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

        assert_eq!(pass(root.path(), records), ["stopped-a1b"]);
        assert_eq!(left(root.path()), ["stopped-a1b"]);
        assert!(shelved(root.path()).is_empty());
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

        let kept = pass(root.path(), read(root.path()));
        let _ = server.kill();
        assert_eq!(kept, ["stopped-a1b"]);
        assert!(shelved(root.path()).is_empty());
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
