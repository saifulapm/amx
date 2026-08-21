//! `amx send` — put a message in front of a running agent.
//!
//! The message goes into the pane as a bracketed paste and is submitted with
//! Enter, so the agent reads it the way it reads a person typing. Three things
//! around that are the whole verb:
//!
//! * **It refuses while the agent is waiting.** Text typed at a permission
//!   prompt answers the prompt, and answering a question by accident is not
//!   something a caller can take back. The question and the choices under it
//!   go to stdout, where the answer would have been, and the exit code says
//!   blocked.
//! * **It refuses a message that ends its own paste.** The brackets around the
//!   text are what make it text; a message carrying a copy of the closing one
//!   is typing at the agent from the middle of itself. See
//!   [`ends_its_own_paste`].
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

use anyhow::{Result, bail};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive::{self, View};
use crate::store::{Agent, Event, Kind, Phase};
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
        Phase::Waiting => return waiting_on_a_question(&view, to_terminal, out),
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
    if ends_its_own_paste(text) {
        bail!(
            "that message carries the end of a bracketed paste; \
             what follows it would be typed at `{}` rather than pasted into it",
            agent.id()
        );
    }

    let writer = agent.writer()?;
    writer.append(&Event::new(SEND, serde_json::json!({ "text": text })))?;
    writer.update_state(|state| state.seq += 1)?;
    drop(writer);

    server.paste(pane, text)?;
    server.send_keys(pane, &["Enter"])
}

/// Whether the message carries the brackets of the paste it travels in.
///
/// tmux writes `ESC [ 200 ~` before the text and `ESC [ 201 ~` after it, and
/// it does not look at the text in between. A message carrying the closing
/// pair ends its own paste early: the rest of it arrives at the vendor as
/// keystrokes, where a newline submits, an arrow moves a menu's cursor and a
/// line beginning with a slash is a command. That is a message writing itself
/// a second turn, and there is no escape for it — a paste is bytes, so the
/// only answer is to refuse.
///
/// The opening pair goes with it. Nothing amx has any business sending carries
/// either, and a message that is trying to open a paste of its own is a
/// message worth stopping on the same sentence.
fn ends_its_own_paste(text: &str) -> bool {
    // `ESC [` and the one character an 8-bit terminal takes in its place.
    ["\u{1b}[", "\u{9b}"]
        .iter()
        .any(|csi| text.contains(&format!("{csi}200~")) || text.contains(&format!("{csi}201~")))
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

/// Exit `BLOCKED`, with the pending question — and the choices under it —
/// where the answer would have gone.
///
/// stdout carries the question because that is the pipe a caller reads, and
/// stderr names the verb that unblocks it. The choices go on stdout too: they
/// are what the answer has to be one of, and a caller that has to capture the
/// pane and parse it for them is a caller amx has not finished the job for.
/// A question amx never captured still blocks — the state says so, and only
/// the text is missing.
pub fn waiting_on_a_question(view: &View, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    let id = view.id();
    if let Some(question) = &view.state.question {
        line(&rendered(question, to_terminal), out)?;
        for choice in numbered(&view.state.options) {
            line(&rendered(&choice, to_terminal), out)?;
        }
    }
    eprintln!(
        "amx: {id} is waiting on a question. answer it with `{}`",
        how_to_answer(id, view.kind())
    );
    Ok(exit::BLOCKED)
}

/// The choices under a question, numbered the way the screen numbers them.
///
/// Every surface that prints them prints them from here, so the number a
/// person reads off `ls` is the number `amx answer` takes.
pub fn numbered(options: &[String]) -> impl Iterator<Item = String> + '_ {
    options
        .iter()
        .enumerate()
        .map(|(at, label)| format!("{}. {label}", at + 1))
}

/// The command that answers this question, with the grammar it will take.
///
/// Which kind it is decides that grammar, so the offer is not the same
/// sentence twice: a permission box and the trust screen want one key, and a
/// question the vendor asked itself takes a choice or words of your own.
pub fn how_to_answer(id: &str, kind: Option<Kind>) -> String {
    match kind {
        Some(Kind::Question) => format!("amx answer {id} <1-9|\"words of your own\">"),
        _ => format!("amx answer {id} <y|n|1-9|enter|esc>"),
    }
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
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Meta, State};
    use crate::tmux::Socket;
    use serde_json::json;

    fn events(kinds: &[&str]) -> Vec<Event> {
        kinds
            .iter()
            .map(|kind| Event::new(*kind, json!({})))
            .collect()
    }

    /// An agent stopped on a question, as a reader hands it over.
    fn asking(question: Option<&str>, options: &[&str], kind: Option<Kind>) -> View {
        View {
            meta: Meta {
                id: "fix-login-a1b".to_string(),
                task: "fix the login bug".to_string(),
                dir: std::path::PathBuf::from("/srv/app"),
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
            state: State {
                state: Phase::Waiting,
                question: question.map(str::to_string),
                options: options.iter().map(|label| label.to_string()).collect(),
                kind,
                ..State::default()
            },
            verdict: Verdict {
                phase: Phase::Waiting,
                evidence: Evidence::Hooks,
                rule: None,
                age: 3,
            },
        }
    }

    #[test]
    fn hardening_a_message_may_not_end_its_own_paste() {
        assert!(!ends_its_own_paste(
            "fix the login bug\nand the tests with it"
        ));
        assert!(!ends_its_own_paste(
            "the escape \u{1b}[2J on its own is text"
        ));

        // What follows the terminator is not pasted, it is typed.
        assert!(ends_its_own_paste("done\u{1b}[201~/exit\r"));
        assert!(
            ends_its_own_paste("done\u{9b}201~/exit\r"),
            "including the one character an 8-bit terminal takes for ESC ["
        );
        assert!(
            ends_its_own_paste("\u{1b}[200~ another paste inside this one"),
            "and the end amx did not write is as bad as the start"
        );
    }

    #[test]
    fn hardening_a_message_amx_refuses_never_reaches_the_record() {
        // The record is written before the text is typed, so a refusal that
        // came afterwards would leave a send on the log that never happened
        // and a sequence number `result` reads as this turn's.
        let root = tempfile::TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &asking(None, &[], None).meta).unwrap();
        let server = Server::from_socket(Socket::Name("amx-no-such-server".to_string()));

        let refused = deliver(
            &agent,
            &server,
            &PaneId::new("%1").unwrap(),
            "harmless\u{1b}[201~\u{1b}[B\r",
        )
        .unwrap_err();

        let said = format!("{refused:#}");
        assert!(said.contains("paste"), "{said}");
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
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
            &asking(
                Some("Claude needs your permission to use Bash"),
                &["Yes", "No"],
                None,
            ),
            false,
            &mut out,
        )
        .unwrap();

        assert_eq!(code, exit::BLOCKED);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Claude needs your permission to use Bash\n1. Yes\n2. No\n",
            "the choices are what the answer has to be one of"
        );
    }

    #[test]
    fn a_question_nobody_captured_still_blocks() {
        let mut out = Vec::new();
        let code = waiting_on_a_question(&asking(None, &[], None), false, &mut out).unwrap();
        assert_eq!(code, exit::BLOCKED);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn surfaces_the_offer_says_what_this_kind_of_question_will_take() {
        // A permission box and the trust screen take one key. Offering words
        // at either would be an offer amx cannot keep.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            let offered = how_to_answer("fix-login-a1b", kind);
            assert!(offered.contains("y|n|1-9"), "{offered}");
            assert!(!offered.contains("words"), "{offered}");
        }

        let menu = how_to_answer("fix-login-a1b", Some(Kind::Question));
        assert!(menu.contains("words of your own"), "{menu}");
        assert!(menu.starts_with("amx answer fix-login-a1b"), "{menu}");
    }

    #[test]
    fn surfaces_the_choices_are_numbered_the_way_the_screen_numbers_them() {
        let options = ["the sqlite one".to_string(), "the docker one".to_string()];
        assert_eq!(
            numbered(&options).collect::<Vec<_>>(),
            ["1. the sqlite one", "2. the docker one"]
        );
        assert_eq!(numbered(&[]).count(), 0);
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
