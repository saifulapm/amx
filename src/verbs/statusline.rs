//! `amx statusline`: two counts for a tmux status line, how many agents are
//! running and how many need a person. Prints nothing when both are zero.
//!
//! - The output is embedded in someone else's status line, so it is plain
//!   bytes: no colour, no escapes, no `#` that tmux would read as a format.
//! - The counts come from the same reading as `ls` and the view, so the three
//!   never disagree.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::derive::{self, View};
use crate::store::{Phase, now};
use crate::{exit, paths};

/// Agents that are running.
const PULSE: char = '✽';
/// Agents that need a person.
const WARNING: char = '⚠';

/// Run the verb against the machine.
pub fn from_env() -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
///
/// Unlike `ls`, this does not sweep finished records: it runs on a timer, and
/// housekeeping on a timer would make amx a background job.
pub fn run(root: &Path, now: u64, out: &mut impl Write) -> Result<i32> {
    // An ended record reads as its own phase, which has no glyph, so only the
    // live ones need a reading.
    let live = derive::records(root)?
        .into_iter()
        .filter(|record| !record.state.state.is_terminal())
        .collect();
    let phases: Vec<Phase> = derive::views_of(root, live, now)
        .iter()
        .map(View::phase)
        .collect();

    let line = summary(&phases);
    // No bytes at all when there is nothing to count: an empty line would
    // still leave a gap in the status line.
    if !line.is_empty() {
        writeln!(out, "{line}")?;
    }
    Ok(exit::OK)
}

/// Which glyph an agent counts under, if any.
///
/// `unknown` counts as needing a person: a pane amx cannot read is worth a
/// look. Finished phases count under neither. Every phase is listed so a new
/// one fails to compile until it is placed.
fn glyph(phase: Phase) -> Option<char> {
    match phase {
        Phase::Starting | Phase::Working => Some(PULSE),
        Phase::Waiting | Phase::Unknown => Some(WARNING),
        Phase::Idle | Phase::Done | Phase::Failed | Phase::Stopped => None,
    }
}

/// The running count, then the needs-a-person count, each left out when zero.
fn summary(phases: &[Phase]) -> String {
    let counted = |sign: char| {
        phases
            .iter()
            .filter(|phase| glyph(**phase) == Some(sign))
            .count()
    };

    [PULSE, WARNING]
        .into_iter()
        .map(|sign| (sign, counted(sign)))
        .filter(|(_, count)| *count > 0)
        .map(|(sign, count)| format!("{sign}{count}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Agent, Meta};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn meta(id: &str) -> Meta {
        Meta {
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
        }
    }

    fn printed(root: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        assert_eq!(run(root, 1_000, &mut out).unwrap(), exit::OK);
        out
    }

    #[test]
    fn the_glyph_law_puts_each_phase_under_one_sign_or_neither() {
        for (phase, want) in [
            (Phase::Starting, "✽1"),
            (Phase::Working, "✽1"),
            (Phase::Waiting, "⚠1"),
            (Phase::Unknown, "⚠1"),
            (Phase::Idle, ""),
            (Phase::Done, ""),
            (Phase::Failed, ""),
            (Phase::Stopped, ""),
        ] {
            assert_eq!(summary(&[phase]), want, "{phase}");
        }
    }

    #[test]
    fn the_counts_add_up_with_the_pulse_first() {
        assert_eq!(
            summary(&[
                Phase::Working,
                Phase::Waiting,
                Phase::Starting,
                Phase::Unknown,
                Phase::Done
            ]),
            "✽2 ⚠2"
        );
    }

    #[test]
    fn a_sign_nobody_is_under_is_left_off() {
        assert_eq!(summary(&[Phase::Working, Phase::Starting]), "✽2");
        assert_eq!(summary(&[Phase::Waiting, Phase::Unknown]), "⚠2");
    }

    #[test]
    fn nothing_to_say_is_said_with_nothing() {
        assert_eq!(summary(&[]), "");
        // A wall of finished agents prints the same as no agents.
        assert_eq!(
            summary(&[Phase::Done, Phase::Failed, Phase::Stopped, Phase::Idle]),
            ""
        );
    }

    #[test]
    fn the_line_is_nothing_a_status_line_would_read_as_its_own() {
        // Embedded in a tmux `status-right`: an escape would paint the line,
        // and `#` starts a tmux format.
        let line = summary(&[Phase::Working, Phase::Waiting]);
        assert!(!line.contains('\u{1b}'), "an escape in {line:?}");
        assert!(!line.contains('#'), "a tmux format in {line:?}");
        assert!(!line.contains('\n'), "a second line in {line:?}");
    }

    #[test]
    fn a_machine_with_no_agents_prints_no_bytes_at_all() {
        let root = TempDir::new().unwrap();
        let out = printed(root.path());
        assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    }

    #[test]
    fn a_wall_of_finished_agents_prints_no_bytes_either() {
        // Records exist, and none of them has anything to report.
        let root = TempDir::new().unwrap();
        for (id, phase) in [
            ("fix-login-a1b", Phase::Done),
            ("port-importer-c3d", Phase::Stopped),
        ] {
            let agent = Agent::create(root.path(), &meta(id)).unwrap();
            agent
                .writer()
                .unwrap()
                .update_state(|state| state.state = phase)
                .unwrap();
        }

        let out = printed(root.path());
        assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    }

    #[test]
    fn a_live_agent_among_finished_ones_is_counted() {
        let root = TempDir::new().unwrap();
        for (id, phase) in [
            ("fix-login-a1b", Phase::Done),
            ("port-importer-c3d", Phase::Waiting),
        ] {
            let mut meta = meta(id);
            meta.socket = Socket::Name(format!("amx-no-such-server-{}", std::process::id()));
            let agent = Agent::create(root.path(), &meta).unwrap();
            agent
                .writer()
                .unwrap()
                .update_state(|state| {
                    state.state = phase;
                    // Parked, so with no pane the reading is the record's
                    // phase.
                    state.parked_at = 1;
                })
                .unwrap();
        }

        assert_eq!(String::from_utf8(printed(root.path())).unwrap(), "⚠1\n");
    }
}
