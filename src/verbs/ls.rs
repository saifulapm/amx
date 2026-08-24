//! `amx ls` — every agent, and what it is doing.
//!
//! Two audiences read this. A person wants a short table they can take in at a
//! glance; a program wants a shape it can branch on without parsing English,
//! which is what `--json` is for. Both answer from the same reading, so they
//! can never disagree.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::derive::{self, View};
use crate::store::now;
use crate::verbs::send;
use crate::{exit, gc, paths, rules};

/// Run the verb against the machine.
pub fn from_env(json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, json, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
pub fn run(root: &Path, json: bool, now: u64, out: &mut impl Write) -> Result<i32> {
    // Listing is the moment amx tidies up after itself: it is run often, and
    // nobody is waiting on its answer the way a caller waits on `result`.
    let _ = gc::sweep(root, now);

    let views = derive::views(root, rules::bundled(), now)?;
    if json {
        let listed: Vec<_> = views.iter().map(View::json).collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&listed)?)?;
    } else {
        table(&views, out)?;
    }
    Ok(exit::OK)
}

/// The table a person reads.
fn table(views: &[View], out: &mut impl Write) -> Result<()> {
    if views.is_empty() {
        writeln!(out, "no agents")?;
        return Ok(());
    }

    let widest = views.iter().map(|view| view.id().len()).max().unwrap_or(0);
    for view in views {
        writeln!(
            out,
            "{:<8} {:<widest$}  {:>5}  {}",
            view.phase().as_str(),
            view.id(),
            age(view),
            doing(view),
        )?;
    }
    Ok(())
}

/// What this agent is up to, as a row can carry it: what it is waiting to be
/// told, with the choices it is waiting to be told from, else what it is doing.
///
/// The choices ride the row because they are short, they are numbered, and the
/// number is the whole of the answer — a person scanning a wall for the agent
/// that is blocked can answer it without opening anything. There are none to
/// carry unless a question is outstanding: they are cleared with it.
fn doing(view: &View) -> String {
    let mut said = inert(first_line(view.line().unwrap_or("")));
    for choice in send::numbered(&view.state.options) {
        said.push_str("  ");
        said.push_str(&inert(&choice));
    }
    said
}

/// A string amx did not author, on one line and unable to drive the terminal
/// it prints into. Both halves are the table's: a row is a row, and the bytes
/// in it came from a program amx does not control.
fn inert(text: &str) -> String {
    crate::tmux::sanitize(first_line(text)).trim().to_string()
}

/// The reading's own number, in the shortest form that says it: how long a
/// finished run took, how long a waiting agent has waited, and how long since
/// anything was heard from one still going.
fn age(view: &View) -> String {
    let seconds = view.verdict.age;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// One line of it, so a paragraph of an answer cannot take over the table.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;

    fn meta(id: &str, created: u64) -> Meta {
        Meta {
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
            created,
        }
    }

    fn view(id: &str, phase: Phase, age: u64, line: Option<&str>) -> View {
        View {
            meta: meta(id, 1),
            state: State {
                state: phase,
                summary: line.map(str::to_string),
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence: Evidence::Hooks,
                rule: None,
                age,
            },
        }
    }

    /// A row worked out from a record the way `ls` works one out, rather than
    /// written by hand: the last column is the reader's number, and this is
    /// the surface a person reads it off.
    fn reading(id: &str, state: State, created: u64, now: u64) -> View {
        let verdict =
            derive::read(&state, created, true, || None, rules::bundled(), now, 1).verdict;
        View::new(meta(id, created), state, verdict)
    }

    fn printed(views: &[View]) -> String {
        let mut out = Vec::new();
        table(views, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn reader_the_table_says_the_state_the_name_and_what_it_is_doing() {
        let text = printed(&[view(
            "fix-login-a1b",
            Phase::Working,
            12,
            Some("Running Bash"),
        )]);
        assert!(text.contains("working"), "{text}");
        assert!(text.contains("fix-login-a1b"), "{text}");
        assert!(text.contains("12s"), "{text}");
        assert!(text.contains("Running Bash"), "{text}");
    }

    #[test]
    fn reader_the_table_keeps_one_row_to_one_line() {
        // An answer is a paragraph and a row is a row.
        let text = printed(&[view(
            "fix-login-a1b",
            Phase::Idle,
            1,
            Some("I fixed it.\n\nHere is what I changed:\n- the parser"),
        )]);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("I fixed it."), "{text}");
        assert!(!text.contains("the parser"), "{text}");
    }

    #[test]
    fn reader_the_table_says_so_when_there_is_nothing_to_say() {
        assert_eq!(printed(&[]).trim(), "no agents");
    }

    #[test]
    fn reader_the_last_column_ticks_while_an_agent_runs_and_freezes_when_it_ends() {
        let mut record = State {
            state: Phase::Working,
            since: 1_000,
            last_event: 1_000,
            summary: Some("Running Bash".to_string()),
            ..State::default()
        };

        // Still going: how long since anything was heard, moving with the
        // clock, which is what says whether the rest of the row is worth
        // believing.
        for (now, said) in [(1_004, "4s"), (1_008, "8s")] {
            let text = printed(&[reading("fix-login-a1b", record.clone(), 1_000, now)]);
            assert!(text.contains(said), "{text}");
        }

        // Ended after five minutes. Read an hour later and a day later, it is
        // the run it was both times.
        record.state = Phase::Done;
        record.since = 1_300;
        record.last_event = 1_300;
        record.ended = 1_300;
        record.result = Some("the tests pass now".to_string());

        let hour = printed(&[reading("fix-login-a1b", record.clone(), 1_000, 4_900)]);
        assert!(hour.contains("5m"), "{hour}");
        assert_eq!(
            printed(&[reading("fix-login-a1b", record, 1_000, 90_000)]),
            hour,
            "a row of a run that took five minutes says five minutes"
        );
    }

    #[test]
    fn reader_ages_read_as_a_person_would_say_them() {
        let aged = |seconds| age(&view("x-a1b", Phase::Idle, seconds, None));
        assert_eq!(aged(0), "0s");
        assert_eq!(aged(59), "59s");
        assert_eq!(aged(60), "1m");
        assert_eq!(aged(3_599), "59m");
        assert_eq!(aged(3_600), "1h");
        assert_eq!(aged(86_400), "1d");
    }
}
