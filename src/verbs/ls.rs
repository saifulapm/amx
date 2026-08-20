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
        let line = view.line().unwrap_or("");
        writeln!(
            out,
            "{:<8} {:<widest$}  {:>5}  {}",
            view.phase().as_str(),
            view.id(),
            age(view),
            first_line(line),
        )?;
    }
    Ok(())
}

/// How long since anything was heard, in the shortest form that says it.
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

    fn view(id: &str, phase: Phase, age: u64, line: Option<&str>) -> View {
        View {
            meta: Meta {
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
            },
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
