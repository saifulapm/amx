//! `amx status` — one agent, and which signal amx is trusting.
//!
//! The state on its own is half an answer. A reader that says `working`
//! because a hook arrived a second ago and one that says `working` because
//! nothing has been heard for two minutes are telling a person two different
//! things, so this says which it is.
//!
//! An agent that is waiting gets the rest of the answer: what it is asking,
//! the choices under that, and the command that answers it. That last line is
//! the point of the other two — a person reading this is the one who has to
//! unblock it.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::derive::{self, Evidence, View};
use crate::store::{Phase, now};
use crate::verbs::send;
use crate::{exit, paths, rules};

/// Run the verb against the machine.
pub fn from_env(id: &str, json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, id, json, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
pub fn run(root: &Path, id: &str, json: bool, now: u64, out: &mut impl Write) -> Result<i32> {
    let view = derive::view(root, id, rules::bundled(), now)?;
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&view.json())?)?;
    } else {
        report(&view, out)?;
    }
    Ok(exit::OK)
}

/// What a person reads.
fn report(view: &View, out: &mut impl Write) -> Result<()> {
    writeln!(out, "{}  {}", view.id(), view.phase())?;
    writeln!(out, "  evidence  {}", evidence(view))?;
    if let Some(question) = &view.state.question {
        say(out, "asking", question)?;
        for choice in send::numbered(&view.state.options) {
            say(out, "", &choice)?;
        }
    }
    if view.phase() == Phase::Waiting {
        writeln!(
            out,
            "  answer    {}",
            send::how_to_answer(view.id(), view.kind())
        )?;
    }
    if let Some(summary) = &view.state.summary {
        say(out, "doing", summary)?;
    }
    if let Some(exit) = view.state.exit {
        writeln!(out, "  exit      {exit}")?;
    }
    // The task is free text typed by whoever spawned the agent, so it goes
    // the way of every other word amx did not author.
    say(out, "task", &view.meta.task)?;
    writeln!(out, "  dir       {}", view.meta.dir.display())?;
    if let Some(branch) = &view.meta.branch {
        writeln!(out, "  branch    {branch}")?;
    }
    writeln!(out, "  pane      {}", view.meta.pane)?;
    Ok(())
}

/// One field of the report, in words amx did not author.
///
/// The label is left blank on a line that continues the one above it: a
/// question the vendor wrapped across a screen is one thing being asked, and
/// so are the choices under it.
fn say(out: &mut impl Write, label: &str, text: &str) -> Result<()> {
    for (at, line) in inert(text).lines().enumerate() {
        let label = match at {
            0 => label,
            _ => "",
        };
        writeln!(out, "  {label:<8}  {line}")?;
    }
    Ok(())
}

/// A string amx did not author, as a person should receive it.
///
/// A terminal is an interpreter, and the same bytes that read as a question
/// can retitle the window or clear the screen. What the vendor wrote arrives
/// here inert, keeping only the line breaks the layout above is ready for.
fn inert(text: &str) -> String {
    crate::tmux::sanitize(text).trim().to_string()
}

/// The sentence that says what amx is going on, and how old it is.
fn evidence(view: &View) -> String {
    let age = view.verdict.age;
    match &view.verdict.evidence {
        Evidence::Record => "the record says how it ended".to_string(),
        Evidence::Gone => "its pane is gone".to_string(),
        Evidence::Hooks => match &view.verdict.rule {
            // The screen was read and was not allowed to end a running turn.
            Some(rule) => format!(
                "the vendor's hooks, {age}s ago; the screen looks like `{rule}` but has not held still"
            ),
            None => format!("the vendor's hooks, {age}s ago"),
        },
        Evidence::Screen => format!(
            "the screen, matching `{}`, with nothing heard for {age}s",
            view.verdict.rule.as_deref().unwrap_or("a rule")
        ),
        Evidence::Unknown => {
            format!("nothing amx knows: no hooks for {age}s, and no rule claims the screen")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::Verdict;
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;

    fn view(phase: Phase, evidence: Evidence, rule: Option<&str>, age: u64) -> View {
        View {
            meta: Meta {
                id: "fix-login-a1b".to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: Some("amx/fix-login-a1b".to_string()),
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
            state: State {
                state: phase,
                summary: Some("Running Bash".to_string()),
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence,
                rule: rule.map(str::to_string),
                age,
                worked: age,
            },
        }
    }

    fn printed(view: &View) -> String {
        let mut out = Vec::new();
        report(view, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn reader_status_says_which_signal_it_is_going_on() {
        let fresh = printed(&view(Phase::Working, Evidence::Hooks, None, 2));
        assert!(fresh.contains("hooks"), "{fresh}");
        assert!(fresh.contains("2s"), "how old it is is the point: {fresh}");

        let screen = printed(&view(
            Phase::Idle,
            Evidence::Screen,
            Some("idle_prompt"),
            90,
        ));
        assert!(screen.contains("screen"), "{screen}");
        assert!(screen.contains("idle_prompt"), "{screen}");

        let unknown = printed(&view(Phase::Unknown, Evidence::Unknown, None, 300));
        assert!(unknown.contains("no rule claims"), "{unknown}");
        assert!(unknown.contains("300s"), "{unknown}");

        let gone = printed(&view(Phase::Stopped, Evidence::Gone, None, 4));
        assert!(gone.contains("pane is gone"), "{gone}");
    }

    #[test]
    fn reader_status_says_when_the_screen_was_read_and_not_believed() {
        let held = printed(&view(
            Phase::Working,
            Evidence::Hooks,
            Some("idle_prompt"),
            40,
        ));
        assert!(held.contains("idle_prompt"), "{held}");
        assert!(held.contains("held still"), "{held}");
    }

    #[test]
    fn reader_status_names_the_agent_and_where_it_works() {
        let text = printed(&view(Phase::Working, Evidence::Hooks, None, 1));
        assert!(text.starts_with("fix-login-a1b  working"), "{text}");
        assert!(text.contains("/srv/app"), "{text}");
        assert!(text.contains("amx/fix-login-a1b"), "{text}");
        assert!(text.contains("%1"), "{text}");
    }
}
