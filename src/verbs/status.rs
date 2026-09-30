//! `amx status`: one agent's phase, the evidence it was read from and how old
//! that evidence is. A waiting agent also gets its question, the numbered
//! choices and the `amx answer` command that unblocks it.

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
    // Messages sent and not yet taken: a working agent holds them behind its
    // turn, an idle one after a turn cut short by hand. An ended agent never
    // takes them.
    let queued = match view.phase() {
        Phase::Working | Phase::Idle => {
            send::still_queued(&view.meta, &Agent::open(root, id)?.events()?)
        }
        _ => Vec::new(),
    };
    if json {
        // Always present, empty or not, so callers need not check for it.
        let mut json = view.json();
        json["queued"] = serde_json::json!(queued);
        writeln!(out, "{}", serde_json::to_string_pretty(&json)?)?;
    } else {
        report(&view, &queued, now, out)?;
    }
    Ok(exit::OK)
}

/// The report for a person.
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
    // Tells a caller its message arrived and has not been read yet.
    for message in queued {
        say(out, "queued", message.lines().next().unwrap_or_default())?;
    }
    if let Some(exit) = view.state.exit {
        writeln!(out, "  exit      {exit}")?;
    }
    if let Some(role) = &view.meta.role {
        say(out, "role", role)?;
    }
    // Free text from whoever spawned the agent, so it is sanitized too.
    say(out, "task", &view.meta.task)?;
    writeln!(out, "  dir       {}", view.meta.dir.display())?;
    if let Some(branch) = &view.meta.branch {
        writeln!(out, "  branch    {branch}")?;
    }
    writeln!(out, "  pane      {}", view.meta.pane)?;
    Ok(())
}

/// One labelled field of text amx did not author, sanitized. Continuation
/// lines get a blank label.
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

/// `text` with control and format characters replaced, keeping line breaks,
/// so it cannot drive the terminal it is printed to.
fn inert(text: &str) -> String {
    crate::tmux::sanitize(text).trim().to_string()
}

/// What the reading is based on, and how old it is.
///
/// The age is the time since the agent was last heard from, except for a
/// parked agent, which is dated from when amx let its pane go.
fn evidence(view: &View, now: u64) -> String {
    let age = view.verdict.age;
    match &view.verdict.evidence {
        Evidence::Record => "the record says how it ended".to_string(),
        Evidence::Gone => "its pane is gone".to_string(),
        Evidence::LetGo => format!(
            "parked {}s ago; `amx resume`, `amx attach` or enter in the view brings it back",
            now.saturating_sub(view.state.parked_at)
        ),
        Evidence::Hooks => match &view.verdict.rule {
            // A rule matched the screen but was not allowed to end the turn.
            Some(rule) => format!(
                "the vendor's hooks, {age}s ago; the screen matches `{rule}` but is still changing"
            ),
            None => format!("the vendor's hooks, {age}s ago"),
        },
        Evidence::Screen => format!(
            "the screen, matching `{}`, with no hook for {age}s",
            view.verdict.rule.as_deref().unwrap_or("a rule")
        ),
        Evidence::Unknown => {
            format!("none: no hooks for {age}s, and no rule matches the screen")
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
        // A turn cut short by hand leaves the agent idle with the message
        // still unread.
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
        let root = tempfile::TempDir::new().unwrap();
        let mut meta = view(Phase::Idle, Evidence::Record, None, 0).meta;
        meta.socket = Socket::Name(format!("amx-no-such-server-{}", std::process::id()));
        Agent::create(root.path(), &meta).unwrap();

        let mut out = Vec::new();
        run(root.path(), &meta.id, true, 5_000, &mut out).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["queued"], serde_json::json!([]), "{json}");
    }

    /// The report at a given `now`, for evidence dated from the record.
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
        assert!(unknown.contains("no rule matches"), "{unknown}");
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
        assert!(held.contains("still changing"), "{held}");
    }

    #[test]
    fn reader_status_says_a_parked_agent_is_still_there_to_come_back_to() {
        // Last heard an hour ago, parked forty seconds ago: the age is the
        // parking.
        let mut parked = view(Phase::Idle, Evidence::LetGo, None, 3_640);
        parked.state.parked_at = 5_000;
        let text = printed_at(&parked, 5_040);

        assert!(text.starts_with("fix-login-a1b  done"), "{text}");
        assert!(text.contains("parked 40s ago"), "{text}");
        assert!(text.contains("`amx resume`"), "{text}");
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
