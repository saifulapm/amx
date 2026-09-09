//! `amx interrupt` — stop the turn an agent is in the middle of.
//!
//! Escape is the key. A vendor at work reads it as drop what you are doing:
//! the turn ends where it stands, whatever it had half written, and the agent
//! is back at its prompt with the conversation behind it intact. That is the
//! whole of the verb, and the rest of this file is about the panes it must not
//! be typed at.
//!
//! * **A question is not a turn.** Escape at a permission prompt dismisses the
//!   prompt, which is an answer to it, and answering a question by accident is
//!   not something a caller can take back. The question goes to stdout where
//!   `send` puts it, and the line beside it names the verb that does mean to
//!   answer: `amx answer <id> esc`.
//! * **A command is not an agent.** A row running a shell command has no
//!   vendor in it to read a key: Escape reaches whatever the command makes of
//!   it, which is usually nothing, and the command runs on. `amx stop` is what
//!   ends one.
//! * **Nothing running is nothing to interrupt.** Parked, idle or ended, the
//!   key would land in the agent's next turn rather than in this one, and a
//!   verb that reported success would leave a caller believing a turn had been
//!   cut short when none was.
//!
//! It records itself before it types, for the reason `send` does: a `result`
//! in another shell must not hand back the last turn's answer as this turn's.
//! The event is also what says the turn is over. Whether a vendor mentions a
//! turn it was interrupted out of is that vendor's business, and its hooks are
//! written around turns that run to their end, so a wait holding out for one
//! is a wait that may sit there until its own deadline over a turn that ended
//! the moment the key landed. See [`crate::verbs::result`].
//!
//! What it does not do is move the phase. amx typed at the agent and heard
//! nothing back, and a keystroke is not news about the screen it was typed at;
//! the next hook, or the next reader at the pane, is what says the agent has
//! stopped. `answer` leaves a key it cannot name on the record the same way.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::derive::{self, Evidence, View};
use crate::store::{Agent, Event, Phase};
use crate::tmux::Server;
use crate::verbs::send;
use crate::{complain, exit, paths, store, warn};

/// The event amx records for a turn it cut short.
pub const INTERRUPT: &str = "interrupt";

/// The key that ends the turn a vendor is in the middle of.
const CANCELS: &str = "Escape";

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run(&root, id, to_terminal, &mut out)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    let view = derive::view(root, id, store::now())?;
    match what_is_running(&view) {
        Cut::Turn => {}
        Cut::Question => return a_question_is_answered(&view, to_terminal, out),
        Cut::Command => {
            complain!("amx: {}", end_the_command(id));
            return Ok(exit::FAILURE);
        }
        Cut::Nothing => {
            complain!("amx: {}", nothing_is_running(id, doing(&view)));
            return Ok(exit::FAILURE);
        }
    }

    let agent = Agent::open(root, id)?;
    recorded(&agent)?;
    Server::from_socket(view.meta.socket.clone()).send_keys(&view.meta.pane, &[CANCELS])?;
    Ok(exit::OK)
}

/// What is in this agent's pane, as far as a key is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// A turn, which is the one thing Escape ends.
    Turn,
    /// A question. Escape there answers it rather than ends anything.
    Question,
    /// A command somebody ran, with no vendor in it to read a key at all.
    Command,
    /// Nothing: there is no turn for a key to cut short.
    Nothing,
}

/// Weigh one reading.
///
/// A pane amx let go is asked about before the phase is, because letting a
/// pane go is no ending: the record reads whatever the agent was doing at the
/// moment the pane was taken, so nothing below would say a word about a key
/// being sent to a pane that is not there.
fn what_is_running(view: &View) -> Cut {
    if view.verdict.evidence == Evidence::LetGo {
        return Cut::Nothing;
    }
    match view.phase() {
        Phase::Waiting => Cut::Question,
        Phase::Working if runs_a_command(view) => Cut::Command,
        Phase::Working => Cut::Turn,
        _ => Cut::Nothing,
    }
}

/// Whether this row is a command somebody ran rather than an agent.
///
/// The two things [`crate::derive`] asks of it. A command spawn writes no
/// vendor on the record because it runs none, and nothing ever reports about a
/// command — no hook is sent for it and no rule is held against its pane — so
/// its record sits at the phase the spawn wrote for the whole of its life. A
/// reading of `working` over that pair is tmux saying the pane is still there,
/// which is the whole of what amx knows about a command.
fn runs_a_command(view: &View) -> bool {
    view.meta.agent.is_none() && view.state.state == Phase::Starting
}

/// Write the interrupt down, before the key that causes it is sent.
///
/// The record comes first for the reason `send`'s does: what it tells a reader
/// in another process is that the answer it can see belongs to the turn this
/// key is about to end. A `result` reading the log a moment later would
/// otherwise hand that answer back as this turn's, which is the mistake
/// nothing downstream can undo.
///
/// The log is for whoever reads the whole history; the stamp beside it is for
/// whoever reads the state document, which is every reader of a row. It says
/// the turn on that document is one amx ended itself, so a reader need not sit
/// out the wait a turn nobody cut short is owed — see [`crate::derive::read`].
/// Written with the observing hand, because nothing was heard: the phase is
/// still the vendor's last word, and a stamp for a key amx typed must not have
/// the next reader believe this document over the pane it was typed at.
fn recorded(agent: &Agent) -> Result<()> {
    let writer = agent.writer()?;
    let event = Event::new(INTERRUPT, serde_json::json!({}));
    writer.append(&event)?;
    writer.observe(|state| state.interrupted_at = event.at)?;
    Ok(())
}

/// Exit `BLOCKED`, with the question this agent is stopped on where the answer
/// would have gone.
///
/// The three lines `send` writes at the same pane, for the same reasons: a
/// caller reads stdout, the choices are what an answer has to be one of, and a
/// question amx never captured still blocks. What differs is the sentence
/// beside them, because what is being offered here is not the grammar of an
/// answer but the one key that takes the question away.
fn a_question_is_answered(view: &View, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    if let Some(question) = &view.state.question {
        send::line(&send::rendered(question, to_terminal), out)?;
        for choice in send::numbered(&view.state.options) {
            send::line(&send::rendered(&choice, to_terminal), out)?;
        }
    }
    warn!("amx: {}", answer_it_instead(view.id()));
    Ok(exit::BLOCKED)
}

/// What sends Escape at a question, which is the other verb.
///
/// The same key either way, and which verb sends it is the difference between
/// dismissing a prompt on purpose and cutting short a turn that is not
/// running.
fn answer_it_instead(id: &str) -> String {
    format!("{id} is waiting on a question, not working. dismiss it with `amx answer {id} esc`")
}

/// What ends a command, which is not a key at its pane.
fn end_the_command(id: &str) -> String {
    format!("{id} is a command rather than an agent; there is no turn in it. run: amx stop {id}")
}

/// What a pane with no turn in it has to say for itself.
fn nothing_is_running(id: &str, doing: &str) -> String {
    format!("{id} is {doing}; nothing is running to interrupt")
}

/// What this agent is doing, for a refusal to name it by.
///
/// `parked` where the phase alone would say `idle`: the record keeps the phase
/// the agent was at when amx took its pane, and a refusal naming that without
/// saying the pane has gone is one somebody reads as an agent sitting at a
/// prompt.
fn doing(view: &View) -> &'static str {
    match view.verdict.evidence == Evidence::LetGo {
        true => "parked",
        false => view.phase().as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Agent, Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};

    /// An agent as a reader hands it over.
    fn reading(phase: Phase, evidence: Evidence) -> View {
        View {
            meta: Meta {
                id: "fix-login-a1b".to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
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
                state: phase,
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence,
                rule: None,
                age: 3,
                worked: 3,
            },
        }
    }

    #[test]
    fn interrupt_a_turn_is_the_only_thing_a_key_can_cut_short() {
        assert_eq!(
            what_is_running(&reading(Phase::Working, Evidence::Hooks)),
            Cut::Turn
        );
        assert_eq!(
            what_is_running(&reading(Phase::Waiting, Evidence::Hooks)),
            Cut::Question
        );

        // Nothing is running, so the key would land in whatever the agent does
        // next rather than in the turn it was meant for.
        for phase in [
            Phase::Starting,
            Phase::Idle,
            Phase::Done,
            Phase::Failed,
            Phase::Stopped,
            Phase::Unknown,
        ] {
            assert_eq!(
                what_is_running(&reading(phase, Evidence::Hooks)),
                Cut::Nothing,
                "{phase}"
            );
        }

        // A pane amx let go is no ending, so the record reads whatever the
        // agent was doing when the pane was taken — and there is no pane left
        // to type at.
        assert_eq!(
            what_is_running(&reading(Phase::Idle, Evidence::LetGo)),
            Cut::Nothing
        );
        assert_eq!(
            doing(&reading(Phase::Idle, Evidence::LetGo)),
            "parked",
            "and the refusal says the pane has gone, not that the agent is idle"
        );
    }

    #[test]
    fn interrupt_a_command_has_no_vendor_in_it_to_read_a_key() {
        // A command spawn writes no vendor on the record and nothing ever
        // moves its record off the phase that spawn wrote, so a live pane is
        // the whole of what amx knows about one — which reads as working.
        let mut command = reading(Phase::Working, Evidence::Screen);
        command.meta.agent = None;
        command.state.state = Phase::Starting;
        assert_eq!(what_is_running(&command), Cut::Command);

        // An agent from before amx kept the vendor's name is still an agent:
        // its record has moved off starting, which a command's never does.
        let mut older = reading(Phase::Working, Evidence::Hooks);
        older.meta.agent = None;
        assert_eq!(what_is_running(&older), Cut::Turn);
    }

    #[test]
    fn interrupt_every_refusal_names_what_to_do_instead() {
        let question = answer_it_instead("fix-login-a1b");
        assert!(
            question.contains("amx answer fix-login-a1b esc"),
            "{question}"
        );
        let command = end_the_command("fix-login-a1b");
        assert!(command.contains("amx stop fix-login-a1b"), "{command}");
    }

    #[test]
    fn interrupt_a_waiting_agent_gets_its_question_back_rather_than_a_key() {
        // Escape at a permission prompt answers the prompt, and answering a
        // question by accident is not something a caller can take back. The
        // question goes where the answer would have gone.
        let mut view = reading(Phase::Waiting, Evidence::Hooks);
        view.state.question = Some("Claude needs your permission to use Bash".to_string());
        view.state.options = vec!["Yes".to_string(), "No".to_string()];

        let mut out = Vec::new();
        let code = a_question_is_answered(&view, false, &mut out).unwrap();

        assert_eq!(code, exit::BLOCKED);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Claude needs your permission to use Bash\n1. Yes\n2. No\n"
        );
    }

    #[test]
    fn interrupt_says_so_when_amx_has_let_the_agents_pane_go() {
        // A parked agent reads idle, which is where it was when its pane was
        // taken, so without a word here the key would go to a pane that is not
        // there. The record is untouched: nothing was cut short, and a
        // `result` waiting on this agent must not read one.
        let root = tempfile::TempDir::new().unwrap();
        let meta = Meta {
            socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
            pane: PaneId::new("%404").unwrap(),
            ..reading(Phase::Idle, Evidence::LetGo).meta
        };
        let agent = Agent::create(root.path(), &meta).unwrap();
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.parked_at = 4_600;
            })
            .unwrap();

        let mut out = Vec::new();
        let code = run(root.path(), &meta.id, false, &mut out).unwrap();
        assert_eq!(code, exit::FAILURE);
        assert!(out.is_empty(), "{out:?}");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
    }

    #[test]
    fn interrupt_the_turn_is_on_the_record_before_the_key_is_sent() {
        // A `result` in another shell reads the log to tell this turn's end
        // from the end of the turn before it, so an interrupt that reached the
        // pane first would leave that reader handing back the last turn's
        // answer as this one's.
        let root = tempfile::TempDir::new().unwrap();
        let agent =
            Agent::create(root.path(), &reading(Phase::Working, Evidence::Hooks).meta).unwrap();
        let turn = agent
            .writer()
            .unwrap()
            .update_state(|state| state.state = Phase::Working)
            .unwrap();

        recorded(&agent).unwrap();

        let events = agent.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, INTERRUPT);

        // And the stamp beside it, which is what tells a reader the turn on the
        // record is one amx ended itself. Written with the observing hand: amx
        // typed at the pane and heard nothing back, so the record is no fresher
        // than it was.
        let state = agent.state().unwrap();
        assert_eq!(state.interrupted_at, events[0].at);
        assert_eq!(state.last_event, turn.last_event, "and nothing was heard");
        assert_eq!(state.state, Phase::Working, "nor did the phase move");
    }
}
