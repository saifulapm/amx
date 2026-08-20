//! `amx status` — one agent, and which signal amx is trusting.
//!
//! The state on its own is half an answer. A reader that says `working`
//! because a hook arrived a second ago and one that says `working` because
//! nothing has been heard for two minutes are telling a person two different
//! things, so this says which it is.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::derive::{self, Evidence, View};
use crate::store::now;
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
        writeln!(out, "  asking    {}", question.trim())?;
    }
    if let Some(summary) = &view.state.summary {
        writeln!(out, "  doing     {}", summary.trim())?;
    }
    if let Some(exit) = view.state.exit {
        writeln!(out, "  exit      {exit}")?;
    }
    writeln!(out, "  task      {}", view.meta.task.trim())?;
    writeln!(out, "  dir       {}", view.meta.dir.display())?;
    if let Some(branch) = &view.meta.branch {
        writeln!(out, "  branch    {branch}")?;
    }
    writeln!(out, "  pane      {}", view.meta.pane)?;
    Ok(())
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
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: Some("amx/fix-login-a1b".to_string()),
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
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
