//! `amx interrupt`: end the turn an agent is working on by pressing Escape.
//!
//! The key is sent only to a working agent. Everything else is refused, with
//! nothing typed and nothing recorded:
//!
//! - A waiting agent (exit 2): Escape would answer its question. The question
//!   is printed, and `amx answer <id> esc` named as the way to dismiss it.
//! - A command row (exit 1): no vendor reads the key. `amx stop` ends it.
//! - A parked, idle or ended agent (exit 1): there is no turn to cut short.
//!
//! The interrupt is logged before the key is sent, as `send` logs a message,
//! so a `result` in another shell never returns the previous turn's answer.
//! The event also ends the turn for [`crate::verbs::result`], since vendors
//! may send no hook for an interrupted turn. The phase is not changed: the
//! next hook or reading of the pane does that.

use anyhow::Result;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::derive::{self, Evidence, View};
use crate::store::{Agent, Event, Phase};
use crate::tmux::Server;
use crate::verbs::send;
use crate::{complain, exit, paths, store, warn};

/// The event amx records for a turn it cut short.
pub const INTERRUPT: &str = "interrupt";

/// The key that ends the turn a vendor is in the middle of.
const CANCELS: &str = "Escape";

/// The gap between presses for a vendor that needs several, short enough that
/// the first press still has the cancel armed.
const BETWEEN_PRESSES: Duration = Duration::from_millis(300);

/// Run the verb against the machine.
pub fn from_env(id: &str) -> Result<i32> {
    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run(&root, id, to_terminal, &mut out)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    match cut_the_turn(root, id)? {
        Cut::Turn => Ok(exit::OK),
        // A refusal changed nothing, so reading again gives the same answer.
        Cut::Question => a_question_is_answered(&reading(root, id)?, to_terminal, out),
        Cut::Command => {
            complain!("amx: {}", end_the_command(id));
            Ok(exit::FAILURE)
        }
        Cut::Nothing => {
            complain!(
                "amx: {}",
                nothing_is_running(id, doing(&reading(root, id)?))
            );
            Ok(exit::FAILURE)
        }
    }
}

/// Cut the turn short if one is running, and say what was at the pane.
///
/// Only [`Cut::Turn`] records the interrupt and sends Escape; the other
/// outcomes touch nothing. The view calls this too, so both apply the same
/// rules.
pub fn cut_the_turn(root: &Path, id: &str) -> Result<Cut> {
    let view = reading(root, id)?;
    let cut = what_is_running(&view);
    if cut == Cut::Turn {
        let agent = Agent::open(root, id)?;
        recorded(&agent, view.meta.agent.as_deref())?;
        let server = Server::from_socket(view.meta.socket.clone());
        // An unknown vendor gets one press.
        let presses = crate::registry::entry(view.meta.agent.as_deref().unwrap_or_default())
            .map_or(1, |vendor| vendor.cancel_presses);
        pressed(presses, || server.send_keys(&view.meta.pane, &[CANCELS]))?;
    }
    Ok(cut)
}

/// Press the key `presses` times, [`BETWEEN_PRESSES`] apart.
fn pressed(presses: u8, mut press: impl FnMut() -> Result<()>) -> Result<()> {
    for n in 0..presses {
        if n > 0 {
            std::thread::sleep(BETWEEN_PRESSES);
        }
        press()?;
    }
    Ok(())
}

fn reading(root: &Path, id: &str) -> Result<View> {
    derive::view(root, id, store::now())
}

/// What is in an agent's pane, as far as Escape is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// A running turn: the key was sent.
    Turn,
    /// A question, which Escape would answer.
    Question,
    /// A shell command, with no vendor to read the key.
    Command,
    /// No turn to cut short.
    Nothing,
}

/// Classify one reading.
///
/// A parked agent is checked first: its record keeps the phase it had when the
/// pane was taken, which may read as working.
fn what_is_running(view: &View) -> Cut {
    if view.verdict.evidence == Evidence::LetGo {
        return Cut::Nothing;
    }
    match view.phase() {
        Phase::Waiting => Cut::Question,
        Phase::Working if derive::runs_a_command(&view.meta, &view.state) => Cut::Command,
        Phase::Working => Cut::Turn,
        _ => Cut::Nothing,
    }
}

/// Record the interrupt, before the key is sent.
///
/// Logs an `interrupt` event and stamps `interrupted_at` on the state, so a
/// reader goes to the pane at once instead of waiting out the hook window (see
/// [`crate::derive::read`]). Written with `observe`, since nothing was heard
/// from the agent.
///
/// For a vendor that puts queued messages back in its composer on cancel, the
/// queue is read off the log first and kept in
/// [`crate::store::State::composer_holds`], which `send` refuses to type after.
fn recorded(agent: &Agent, vendor: Option<&str>) -> Result<()> {
    let writer = agent.writer()?;
    let holds = match crate::registry::entry(vendor.unwrap_or_default())
        .is_some_and(|vendor| vendor.restores_queued_on_cancel)
    {
        true => send::queued(
            crate::vendor::hooks_for(vendor.unwrap_or_default()).as_ref(),
            &agent.events()?,
        ),
        false => Vec::new(),
    };
    let event = Event::new(INTERRUPT, serde_json::json!({}));
    writer.append(&event)?;
    writer.observe(|state| {
        state.interrupted_at = event.at;
        state.composer_holds.extend(holds);
    })?;
    Ok(())
}

/// Exit `BLOCKED` with the pending question on stdout, as `send` does, and
/// name `amx answer <id> esc` on stderr.
fn a_question_is_answered(view: &View, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    if let Some(question) = &view.state.question {
        super::print_question(question, &view.state.options, to_terminal, out)?;
    }
    warn!("amx: {}", answer_it_instead(view.id()));
    Ok(exit::BLOCKED)
}

fn answer_it_instead(id: &str) -> String {
    format!("{id} is waiting on a question, not working. dismiss it with `amx answer {id} esc`")
}

fn end_the_command(id: &str) -> String {
    format!("{id} is a command rather than an agent; there is no turn in it. run: amx stop {id}")
}

fn nothing_is_running(id: &str, doing: &str) -> String {
    format!("{id} is {doing}; nothing is running to interrupt")
}

/// The word a refusal uses for this agent's state: its phase, or `parked`
/// when amx let its pane go. Shared with the view so both say the same.
pub fn doing(view: &View) -> &'static str {
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

    /// A view of an agent in `phase`, read off `evidence`.
    fn reading(phase: Phase, evidence: Evidence) -> View {
        View {
            meta: Meta {
                role: None,
                parent: None,
                depth: 0,
                id: "fix-login-a1b".to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
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
            doing: None,
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

        // No turn is running.
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

        // A parked agent has no pane to type at, and its refusal says parked.
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
    fn interrupt_presses_as_many_times_as_the_vendor_asks_300_ms_apart() {
        // opencode's first Escape only arms the cancel and the second cuts the
        // turn. The test vendor asks for three; claude for one.
        let three = crate::vendor::second::ELSEWHERE.cancel_presses;
        let one = crate::registry::entry("claude").unwrap().cancel_presses;
        for presses in [three, one] {
            let mut at = Vec::new();
            pressed(presses, || {
                at.push(std::time::Instant::now());
                Ok(())
            })
            .unwrap();
            assert_eq!(at.len(), usize::from(presses));
            for pair in at.windows(2) {
                assert!(
                    pair[1] - pair[0] >= BETWEEN_PRESSES,
                    "{:?}",
                    pair[1] - pair[0]
                );
            }
        }
        assert_eq!(three, 3);
        assert_eq!(BETWEEN_PRESSES, std::time::Duration::from_millis(300));
    }

    #[test]
    fn interrupt_a_command_has_no_vendor_in_it_to_read_a_key() {
        // A command's record has no vendor and stays at starting, and its live
        // pane reads as working.
        let mut command = reading(Phase::Working, Evidence::Screen);
        command.meta.agent = None;
        command.state.state = Phase::Starting;
        assert_eq!(what_is_running(&command), Cut::Command);

        // An older agent record with no vendor has moved off starting, which
        // a command's never does.
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
        // Escape would answer the prompt, so the question is printed instead.
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
        // Refused, and nothing is logged, so a `result` waiting on this agent
        // does not see an interrupt.
        let root = tempfile::TempDir::new().unwrap();
        let meta = Meta {
            parent: None,
            depth: 0,
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
        // Logged before the key, so a concurrent `result` never returns the
        // previous turn's answer.
        let root = tempfile::TempDir::new().unwrap();
        let agent =
            Agent::create(root.path(), &reading(Phase::Working, Evidence::Hooks).meta).unwrap();
        let turn = agent
            .writer()
            .unwrap()
            .update_state(|state| state.state = Phase::Working)
            .unwrap();

        recorded(&agent, None).unwrap();

        let events = agent.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, INTERRUPT);

        // Stamped with `observe`: `last_event` and the phase do not move.
        let state = agent.state().unwrap();
        assert_eq!(state.interrupted_at, events[0].at);
        assert_eq!(state.last_event, turn.last_event, "and nothing was heard");
        assert_eq!(state.state, Phase::Working, "nor did the phase move");
    }

    #[test]
    fn interrupt_a_pi_holding_sends_stamps_what_goes_back_in_its_composer() {
        // pi puts queued messages back in its composer on cancel, so the record
        // keeps them for `send`. claude does not.
        for (vendor, held) in [("pi", vec!["and the linter"]), ("claude", vec![])] {
            let root = tempfile::TempDir::new().unwrap();
            let meta = Meta {
                agent: Some(vendor.to_string()),
                ..reading(Phase::Working, Evidence::Hooks).meta
            };
            let agent = Agent::create(root.path(), &meta).unwrap();
            let writer = agent.writer().unwrap();
            writer
                .append(&Event::new(
                    send::SEND,
                    serde_json::json!({ "text": "and the linter" }),
                ))
                .unwrap();
            writer
                .update_state(|state| state.state = Phase::Working)
                .unwrap();
            drop(writer);

            recorded(&agent, meta.agent.as_deref()).unwrap();
            assert_eq!(agent.state().unwrap().composer_holds, held, "{vendor}");
        }

        // A pi with nothing queued gets nothing put back.
        let root = tempfile::TempDir::new().unwrap();
        let meta = Meta {
            agent: Some("pi".to_string()),
            ..reading(Phase::Working, Evidence::Hooks).meta
        };
        let agent = Agent::create(root.path(), &meta).unwrap();
        recorded(&agent, meta.agent.as_deref()).unwrap();
        assert!(agent.state().unwrap().composer_holds.is_empty());
    }
}
