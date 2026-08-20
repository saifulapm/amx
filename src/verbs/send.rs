//! `amx send` — put a message in front of a running agent.
//!
//! The message goes into the pane as a bracketed paste and is submitted with
//! Enter, so the agent reads it the way it reads a person typing. Two things
//! around that are the whole verb:
//!
//! * **It refuses while the agent is waiting.** Text typed at a permission
//!   prompt answers the prompt, and answering a question by accident is not
//!   something a caller can take back. The question goes to stdout, where the
//!   answer would have been, and the exit code says blocked.
//! * **It records itself before it types.** A `result` in another shell must
//!   not hand back the last turn's answer as this send's, so the send is on the
//!   record — the event log and the sequence number — before a byte reaches the
//!   pane.
//!
//! Then it waits for the vendor's own word that the text arrived: a
//! `UserPromptSubmit`. Without one within [`CONFIRM`] the text went nowhere
//! that amx can see, and saying so beats reporting a success the caller would
//! then wait on.
//!
//! The refusals here are shared: `result` and `answer` speak the same three
//! sentences, and a caller reading amx's stderr should not find three ways of
//! saying that an agent has ended.

use anyhow::Result;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive;
use crate::store::{Agent, Event, Phase};
use crate::tmux::{PaneId, Server};
use crate::{exit, paths, rules, store};

/// The event amx records for a message it sent.
pub const SEND: &str = "send";

/// The vendor's hook that says a message was taken.
const SUBMITTED: &str = "UserPromptSubmit";

/// How long a send waits for the agent to take what it was given. Long enough
/// for a vendor that is redrawing its screen, short enough that a caller
/// scripting a conversation is not left holding a lie.
const CONFIRM: Duration = Duration::from_secs(5);

/// How often the event log is read while waiting for that word.
const POLL: Duration = Duration::from_millis(50);

/// Run the verb against the machine.
pub fn from_env(id: &str, text: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run(&root, id, text, to_terminal, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    id: &str,
    text: &str,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    let view = derive::view(root, id, rules::bundled(), store::now())?;
    let phase = view.phase();
    match phase {
        Phase::Waiting => {
            return waiting_on_a_question(id, view.state.question.as_deref(), to_terminal, out);
        }
        phase if phase.is_terminal() => return Ok(nothing_more_is_coming(id, phase)),
        _ => {}
    }

    let agent = Agent::open(root, id)?;
    let taken = submissions(&agent.events()?);

    let server = Server::from_socket(view.meta.socket.clone());
    deliver(&agent, &server, &view.meta.pane, text)?;

    // An agent that is mid-turn will not submit this until the turn it is on
    // ends, which is not a stall and may be a long way off.
    if phase == Phase::Working {
        eprintln!("amx: {id} is working; the message is queued behind the turn it is on");
        return Ok(exit::OK);
    }

    if took_it(&agent, taken, CONFIRM)? {
        return Ok(exit::OK);
    }
    eprintln!(
        "amx: {id} did not start working within {}s; the message may not have reached it",
        CONFIRM.as_secs()
    );
    Ok(exit::FAILURE)
}

/// Put the text in front of the agent, recorded before it is typed.
///
/// The record comes first, under the writer's lock, because it is what tells a
/// reader in another process that the answer it can see belongs to the turn
/// before this message. The view acts through this too: what is shared is the
/// order, which is the part that must not be got wrong twice.
pub fn deliver(agent: &Agent, server: &Server, pane: &PaneId, text: &str) -> Result<()> {
    let writer = agent.writer()?;
    writer.append(&Event::new(SEND, serde_json::json!({ "text": text })))?;
    writer.update_state(|state| state.seq += 1)?;
    drop(writer);

    server.paste(pane, text)?;
    server.send_keys(pane, &["Enter"])
}

/// Wait for the vendor to say it took the message.
fn took_it(agent: &Agent, before: usize, patience: Duration) -> Result<bool> {
    let deadline = Instant::now() + patience;
    loop {
        if submissions(&agent.events()?) > before {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(POLL);
    }
}

/// How many prompts this agent has submitted.
///
/// A subagent's events ride the same log and are not the agent's doing, so
/// they do not confirm anybody's send.
fn submissions(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| event.kind == SUBMITTED && event.payload["agent_id"].is_null())
        .count()
}

/// Exit `BLOCKED`, with the pending question where the answer would have gone.
///
/// stdout carries the question because that is the pipe a caller reads, and
/// stderr names the verb that unblocks it. A question amx never captured still
/// blocks — the state says so, and only the text is missing.
pub fn waiting_on_a_question(
    id: &str,
    question: Option<&str>,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    if let Some(question) = question {
        line(&rendered(question, to_terminal), out)?;
    }
    eprintln!(
        "amx: {id} is waiting on a question. answer it with \
         `amx answer {id} <y|n|1-9|enter|esc>`"
    );
    Ok(exit::BLOCKED)
}

/// Exit `FAILURE`: this agent is not going to answer anybody.
pub fn nothing_more_is_coming(id: &str, phase: Phase) -> i32 {
    eprintln!("amx: {id} is {phase}. {}", remedy(id, phase));
    exit::FAILURE
}

/// What to do about an agent in this state — the same offer `ls` makes on a
/// row that needs a person.
fn remedy(id: &str, phase: Phase) -> String {
    match phase {
        Phase::Stopped => format!("run: amx resume {id}"),
        Phase::Failed => format!("it ended badly; run: amx status {id}"),
        Phase::Done => "its command has ended".to_string(),
        _ => format!("run: amx status {id}"),
    }
}

/// Write text amx did not author, with the newline a terminal expects.
pub fn line(text: &str, out: &mut impl Write) -> Result<()> {
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

/// The vendor's own words, as this stdout should receive them.
///
/// Down a pipe they are the payload and stay verbatim: a caller reading
/// `$(amx result …)` is reading what the agent said, not a rendering of it.
/// A terminal is the other case, and a terminal is an interpreter — the same
/// bytes can retitle the window or clear the screen, so a person gets them
/// inert. The agent's own line breaks survive either way.
pub fn rendered(text: &str, to_terminal: bool) -> String {
    match to_terminal {
        true => crate::tmux::sanitize(text),
        false => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn events(kinds: &[&str]) -> Vec<Event> {
        kinds
            .iter()
            .map(|kind| Event::new(*kind, json!({})))
            .collect()
    }

    #[test]
    fn send_counts_the_prompts_the_agent_itself_submitted() {
        assert_eq!(submissions(&events(&[])), 0);
        assert_eq!(
            submissions(&events(&["SessionStart", SUBMITTED, "Stop", SUBMITTED])),
            2
        );

        // A subagent's prompt rides the same log and is not this agent's.
        let mixed = vec![
            Event::new(SUBMITTED, json!({})),
            Event::new(SUBMITTED, json!({ "agent_id": "sub-1" })),
        ];
        assert_eq!(submissions(&mixed), 1);
    }

    #[test]
    fn send_puts_a_pending_question_where_the_answer_would_have_gone() {
        let mut out = Vec::new();
        let code = waiting_on_a_question(
            "fix-login-a1b",
            Some("Claude needs your permission to use Bash"),
            false,
            &mut out,
        )
        .unwrap();

        assert_eq!(code, exit::BLOCKED);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Claude needs your permission to use Bash\n"
        );
    }

    #[test]
    fn a_question_nobody_captured_still_blocks() {
        let mut out = Vec::new();
        let code = waiting_on_a_question("fix-login-a1b", None, false, &mut out).unwrap();
        assert_eq!(code, exit::BLOCKED);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn every_ending_says_what_to_do_about_it() {
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
            assert_eq!(
                nothing_more_is_coming("fix-login-a1b", phase),
                exit::FAILURE
            );
            assert!(
                !remedy("fix-login-a1b", phase).is_empty(),
                "{phase} says nothing"
            );
        }
        assert!(remedy("fix-login-a1b", Phase::Stopped).contains("amx resume fix-login-a1b"));
    }

    #[test]
    fn vendor_text_down_a_pipe_is_the_bytes_the_vendor_wrote() {
        let said = "done\u{1b}]0;PWNED\u{7}\n\tindented\n";
        assert_eq!(rendered(said, false), said);
    }

    #[test]
    fn vendor_text_on_a_terminal_cannot_drive_the_terminal_it_prints_into() {
        let shown = rendered("done\u{1b}]0;PWNED\u{7}\nand more\n", true);
        assert!(shown.contains("done"), "{shown:?}");
        assert!(shown.contains("]0;PWNED"), "still readable: {shown:?}");
        assert!(
            shown.contains("\nand more"),
            "the line breaks are the agent's"
        );
        assert_eq!(
            shown
                .chars()
                .filter(|c| c.is_control() && *c != '\n')
                .count(),
            0,
            "and inert: {shown:?}"
        );
    }

    #[test]
    fn a_line_ends_in_one_newline_however_it_arrived() {
        let mut out = Vec::new();
        line("no newline", &mut out).unwrap();
        line("has one\n", &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "no newline\nhas one\n");
    }
}
