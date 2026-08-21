//! `amx statusline` — the two numbers a status line has room for.
//!
//! Everything else amx prints is read by somebody who came looking for it.
//! This is read by somebody who did not: it sits in a corner of a terminal
//! they were already using, and the only question it answers is whether to go
//! and look. So it answers in two numbers — how many agents are moving, and
//! how many have stopped for a person — and it says nothing at all when there
//! is nothing to say, rather than parking a pair of zeroes in the corner of
//! somebody's screen for ever.
//!
//! It lands inside a line that is not amx's, so it is plain bytes: no colour,
//! no escapes, and nothing tmux would take for a format of its own.
//!
//! The counts come from the same reading `ls` and the view are given, not a
//! cheaper one of their own. A corner of the screen saying two agents are
//! working while `ls` says both stopped is worse than no corner at all: the
//! person now has to work out which of the two to believe.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::derive::{self, View};
use crate::store::{Phase, now};
use crate::{exit, paths, rules};

/// The whole of amx's vocabulary here: a pulse for the agents getting on with
/// it, and the warning sign for the ones that cannot get any further alone.
const PULSE: char = '✽';
const WARNING: char = '⚠';

/// Run the verb against the machine.
pub fn from_env() -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
///
/// `ls` sweeps the finished agents away while it is here. This does not: a
/// status line runs itself on a timer, and a chore on a timer is a background
/// job amx never offered to be.
pub fn run(root: &Path, now: u64, out: &mut impl Write) -> Result<i32> {
    let phases: Vec<Phase> = derive::views(root, rules::bundled(), now)?
        .iter()
        .map(View::phase)
        .collect();

    let line = summary(&phases);
    // Nothing is nothing. An empty line is still a line — it leaves the gap
    // where the counts go, and hands anything reading amx down a pipe a blank
    // to strip.
    if !line.is_empty() {
        writeln!(out, "{line}")?;
    }
    Ok(exit::OK)
}

/// Which glyph an agent counts under, if any.
///
/// Starting and working are one piece of news to somebody glancing at a corner
/// of their screen: something of theirs is running. Waiting and unknown are one
/// piece of news too: it is not running and it will not start again on its own.
/// `unknown` belongs there because a reader that cannot say what a pane is
/// doing is exactly the case worth walking over to.
///
/// The rest are quiet on purpose. Idle, done, failed and stopped have each
/// finished whatever they were going to do, and a glyph that never goes out is
/// one a person stops reading.
///
/// Matched to the phase and not to a group of them: a phase added later does
/// not compile until somebody has said which of the three it is.
fn glyph(phase: Phase) -> Option<char> {
    match phase {
        Phase::Starting | Phase::Working => Some(PULSE),
        Phase::Waiting | Phase::Unknown => Some(WARNING),
        Phase::Idle | Phase::Done | Phase::Failed | Phase::Stopped => None,
    }
}

/// The counts, in the order they are read: what is moving, then what is stuck.
///
/// A count of nobody prints no glyph, so the line is only ever as long as the
/// news in it.
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
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx".to_string()),
            pane: PaneId::new("%1").unwrap(),
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
        // A wall where every agent has finished is the same news to a corner
        // of the screen as a machine with no agents on it.
        assert_eq!(
            summary(&[Phase::Done, Phase::Failed, Phase::Stopped, Phase::Idle]),
            ""
        );
    }

    #[test]
    fn the_line_is_nothing_a_status_line_would_read_as_its_own() {
        // It is embedded in somebody's tmux `status-right`: an escape would
        // paint their line, and `#` is where tmux's own formats begin.
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
        // The records are read, and what they say is that there is nothing to
        // report: silence here is the answer, not a verb that skipped the disk.
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
}
