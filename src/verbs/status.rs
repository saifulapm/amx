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
use crate::store::{Agent, Phase, now};
use crate::verbs::send;
use crate::{exit, paths};

/// Run the verb against the machine.
pub fn from_env(id: &str, json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(&root, id, json, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
pub fn run(root: &Path, id: &str, json: bool, now: u64, out: &mut impl Write) -> Result<i32> {
    let view = derive::view(root, id, now)?;
    // What was sent and not yet taken, which a working agent holds behind its
    // turn and an idle one holds where a turn cut short by hand left it. One
    // that has ended will never take it.
    let queued = match view.phase() {
        Phase::Working | Phase::Idle => send::queued(&Agent::open(root, id)?.events()?),
        _ => Vec::new(),
    };
    if json {
        // Always there, empty or not, so a caller reads it without asking
        // first whether it is.
        let mut json = view.json();
        json["queued"] = serde_json::json!(queued);
        writeln!(out, "{}", serde_json::to_string_pretty(&json)?)?;
    } else {
        report(&view, &queued, now, out)?;
    }
    Ok(exit::OK)
}

/// What a person reads.
fn report(view: &View, queued: &[String], now: u64, out: &mut impl Write) -> Result<()> {
    writeln!(out, "{}  {}", view.id(), view.phase().word())?;
    writeln!(out, "  evidence  {}", evidence(view, now))?;
    if let Some(question) = &view.state.question {
        say(out, "asking", question)?;
        for choice in send::numbered(&view.state.options) {
            say(out, "", &choice)?;
        }
    }
    if view.phase() == Phase::Waiting {
        writeln!(out, "  answer    {}", send::how_to_answer(view))?;
    }
    if let Some(summary) = &view.state.summary {
        say(out, "doing", summary)?;
    }
    // Held behind the turn under way, so a caller who sent it and sees the
    // agent still working knows it arrived and knows it has not been read.
    for message in queued {
        say(out, "queued", message.lines().next().unwrap_or_default())?;
    }
    if let Some(exit) = view.state.exit {
        writeln!(out, "  exit      {exit}")?;
    }
    if let Some(role) = &view.meta.role {
        say(out, "role", role)?;
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
///
/// The age is the reading's own, which is how long since the agent was heard
/// from. A pane amx took away is dated from the record instead: an agent
/// parked after an hour of quiet was heard from an hour ago and let go a
/// moment ago, and the moment is the one a person is reading this for.
fn evidence(view: &View, now: u64) -> String {
    let age = view.verdict.age;
    match &view.verdict.evidence {
        Evidence::Record => "the record says how it ended".to_string(),
        Evidence::Gone => "its pane is gone".to_string(),
        Evidence::LetGo => format!(
            "amx let the process go {}s ago; enter, attach or resume bring it back",
            now.saturating_sub(view.state.parked_at)
        ),
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
    use crate::store::{Event, Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;

    fn view(phase: Phase, evidence: Evidence, rule: Option<&str>, age: u64) -> View {
        View {
            meta: Meta {
                role: None,
                parent: None,
                depth: 0,
                id: "fix-login-a1b".to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                model: None,
                effort: None,
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
            doing: None,
        }
    }

    fn printed(view: &View) -> String {
        printed_at(view, 0)
    }

    #[test]
    fn reader_status_says_what_was_sent_and_not_yet_taken() {
        let view = view(Phase::Working, Evidence::Hooks, None, 1);
        let queued = [
            "and the linter".to_string(),
            "then the docs\nwith care".to_string(),
        ];
        let mut out = Vec::new();
        report(&view, &queued, 0, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let doing = text.find("  doing").expect("a doing line");
        let first = text
            .find("  queued    and the linter")
            .expect("the first, in order");
        let second = text
            .find("  queued    then the docs")
            .expect("the second, first line only");
        assert!(doing < first && first < second, "{text}");
        assert!(!text.contains("with care"), "{text}");
    }

    #[test]
    fn reader_status_says_what_an_idle_agent_was_sent_and_has_not_taken() {
        // A turn cut short by hand leaves the agent idle with a message still
        // in front of it, and a caller who sent it wants to know it waits.
        let root = tempfile::TempDir::new().unwrap();
        let mut meta = view(Phase::Idle, Evidence::Record, None, 0).meta;
        meta.socket = Socket::Name(format!("amx-no-such-server-{}", std::process::id()));
        let agent = Agent::create(root.path(), &meta).unwrap();
        let writer = agent.writer().unwrap();
        for event in [
            Event::new("Stop", serde_json::json!({})),
            Event::new(send::SEND, serde_json::json!({ "text": "and the linter" })),
        ] {
            writer.append(&event).unwrap();
        }
        writer
            .observe(|state| {
                state.state = Phase::Idle;
                state.parked_at = 4_600;
            })
            .unwrap();
        drop(writer);

        let mut out = Vec::new();
        run(root.path(), &meta.id, false, 5_000, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("  queued    and the linter"), "{text}");

        let mut out = Vec::new();
        run(root.path(), &meta.id, true, 5_000, &mut out).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["queued"], serde_json::json!(["and the linter"]));
    }

    #[test]
    fn reader_status_json_says_nothing_is_queued_as_an_empty_list() {
        // A caller reads the key without asking first whether it is there.
        let root = tempfile::TempDir::new().unwrap();
        let mut meta = view(Phase::Idle, Evidence::Record, None, 0).meta;
        meta.socket = Socket::Name(format!("amx-no-such-server-{}", std::process::id()));
        Agent::create(root.path(), &meta).unwrap();

        let mut out = Vec::new();
        run(root.path(), &meta.id, true, 5_000, &mut out).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["queued"], serde_json::json!([]), "{json}");
    }

    /// The same report, read at a given moment: what amx did to a pane is
    /// dated from the record rather than from the last thing the agent said.
    fn printed_at(view: &View, now: u64) -> String {
        let mut out = Vec::new();
        report(view, &[], now, &mut out).unwrap();
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
    fn reader_status_says_a_parked_agent_is_still_there_to_come_back_to() {
        // Its turn ended an hour ago, and amx took the pane away forty
        // seconds ago. The one a person needs is the second.
        let mut parked = view(Phase::Idle, Evidence::LetGo, None, 3_640);
        parked.state.parked_at = 5_000;
        let text = printed_at(&parked, 5_040);

        assert!(text.starts_with("fix-login-a1b  done"), "{text}");
        assert!(text.contains("let the process go 40s ago"), "{text}");
        assert!(text.contains("enter, attach or resume"), "{text}");
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
