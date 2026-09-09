//! `amx result` — wait for the turn to end, and say how it ended.
//!
//! This is the verb the machine-facing half of amx exists for: a caller waits
//! here instead of polling a state file, reading a transcript or scraping a
//! screen. Four endings, and the exit code is which one happened:
//!
//! * `0` — the agent's answer is on stdout.
//! * `1` — nothing is coming: the agent failed, was stopped, had its turn cut
//!   short by `amx interrupt`, or ended its turn without an answer amx could
//!   capture.
//! * `2` — it is asking a question, which is on stdout with the choices under
//!   it. **A wait never goes through a question**: the question usually
//!   arrives *during* the wait, and a caller that cannot see it cannot answer
//!   it.
//! * `3` — the caller's own deadline.
//!
//! **The turn it waits for is the one after the last message.** `send` records
//! itself before it types, so the event log says whether the turn amx can see
//! ended before or after that message — and an answer from before it belongs to
//! the turn before it. Serving that one as this turn's is the mistake nothing
//! downstream can undo.

use anyhow::Result;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive::{self, Evidence, View};
use crate::store::{Agent, Event, Phase};
use crate::vendor::Moment;
use crate::verbs::interrupt::INTERRUPT;
use crate::verbs::send::{self, nothing_more_is_coming, waiting_on_a_question};
use crate::{complain, exit, paths, store};

/// How often the record is read while waiting. Short enough that a caller
/// chaining turns is not waiting on amx, long enough to cost nothing.
const POLL: Duration = Duration::from_millis(200);

/// How often the *pane* is read, once the record has gone quiet enough that a
/// reading needs one. Reading two small files five times a second is free;
/// asking tmux for a screen five times a second is not.
const LOOK: Duration = Duration::from_secs(1);

/// Whether this event is a vendor saying a turn ended: the moment the table
/// calls `Ended`, under whichever vendor's word for it arrived.
fn turn_end(event: &Event) -> bool {
    crate::vendor::moment_of(&event.kind) == Some(Moment::Ended)
}

/// Run the verb against the machine.
pub fn from_env(id: &str, timeout: Option<u64>) -> Result<i32> {
    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run(
        &root,
        id,
        timeout.map(Duration::from_secs),
        to_terminal,
        &mut out,
    )
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    id: &str,
    timeout: Option<Duration>,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    let deadline = timeout.map(|patience| Instant::now() + patience);

    loop {
        let view = derive::view(root, id, store::now())?;
        let phase = view.phase();
        // Only an ending needs the log: while an agent is working there is no
        // turn to place against the last message.
        let ended = match phase {
            Phase::Idle | Phase::Done => past_the_last_message(root, id)?,
            _ => Ended::NotYet,
        };

        match settled(phase, ended) {
            Settled::Answer => return answer(&view, to_terminal, out),
            Settled::Question => return waiting_on_a_question(&view, to_terminal, out),
            // The command ended, and the message it was sent went with it.
            Settled::Unanswered => {
                complain!("amx: {id} ended without answering");
                return Ok(exit::FAILURE);
            }
            // Somebody stopped the turn this wait was for. Whatever is on the
            // record answers the turn before the message.
            Settled::Interrupted => {
                complain!("amx: {id} was interrupted; the turn ended with no answer");
                return Ok(exit::FAILURE);
            }
            Settled::Nothing => return Ok(nothing_more_is_coming(id, phase)),
            Settled::NotYet => {}
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(exit::TIMEOUT);
        }
        std::thread::sleep(pace(&view.verdict.evidence));
    }
}

/// What one reading means to a caller waiting on an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// The turn is over; whatever was captured is the answer.
    Answer,
    /// It is asking, and the wait ends here rather than behind the question.
    Question,
    /// The command ended before the last message was answered.
    Unanswered,
    /// The turn was cut short: there is no answer to it and never will be.
    Interrupted,
    /// It failed or was stopped: no answer is coming.
    Nothing,
    /// Still going.
    NotYet,
}

/// Whether the turn amx is waiting for has ended, and what ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// Nothing since the last message says a turn is over.
    NotYet,
    /// A turn of the agent's own ended, and whatever it left is its answer.
    Turn,
    /// `amx interrupt` ended it — see [`crate::verbs::interrupt`]. A turn
    /// nobody let finish leaves no answer, and the one on the record belongs
    /// to the turn before the message.
    Interrupted,
}

/// Weigh one reading. `ended` is how the turn after the last message ended, if
/// it has.
fn settled(phase: Phase, ended: Ended) -> Settled {
    match (phase, ended) {
        (Phase::Waiting, _) => Settled::Question,
        (Phase::Idle | Phase::Done, Ended::Interrupted) => Settled::Interrupted,
        (Phase::Idle | Phase::Done, Ended::Turn) => Settled::Answer,
        // Idle, but the last thing amx did was hand it a message it has not
        // finished with. That turn is still to come.
        (Phase::Idle, Ended::NotYet) => Settled::NotYet,
        (Phase::Done, Ended::NotYet) => Settled::Unanswered,
        (Phase::Failed | Phase::Stopped, _) => Settled::Nothing,
        (Phase::Starting | Phase::Working | Phase::Unknown, _) => Settled::NotYet,
    }
}

/// How long to wait before reading again, given what the last reading cost.
fn pace(evidence: &Evidence) -> Duration {
    match evidence {
        Evidence::Screen | Evidence::Unknown => LOOK,
        Evidence::Record | Evidence::Gone | Evidence::LetGo | Evidence::Hooks => POLL,
    }
}

/// Whether a turn has ended since the last message amx sent, and what ended
/// it.
fn past_the_last_message(root: &Path, id: &str) -> Result<Ended> {
    Ok(ended_past_the_last_message(
        &Agent::open(root, id)?.events()?,
    ))
}

/// The same question, asked of the log.
///
/// Order, not a clock: the log is appended to under one lock, so "a turn ended
/// after the last message" is a position in it. An agent nobody has sent
/// anything to has no message to be past, and whatever it last answered is its
/// answer.
///
/// The first ending after the message is the one that counts. An interrupt the
/// vendor caught up with afterwards is still the thing that ended the turn,
/// and a turn that ended on its own before anybody typed at it ended on its
/// own.
///
/// Whose word said the turn ended is not this question — see [`a_turn_ended`].
fn ended_past_the_last_message(events: &[Event]) -> Ended {
    let Some(sent) = events.iter().rposition(|event| event.kind == send::SEND) else {
        return Ended::Turn;
    };
    match events[sent..].iter().find(|event| a_turn_ended(event)) {
        None => Ended::NotYet,
        Some(event) if cut_short(event) => Ended::Interrupted,
        Some(_) => Ended::Turn,
    }
}

/// Whether this event says a turn of the agent's own ended.
///
/// The vendor's own word where there was a vendor to say it, and amx's name for
/// the same moment where a reading of the pane is the only thing that will ever
/// place it — see [`derive::READ_TURN_END`]. Both say a turn ended; a wait that
/// took only the first waited on a word half the table never sends, so `result`
/// on a hookless agent that had been sent a message ran to its own deadline
/// over a turn that had ended and an answer that was on the record beside it.
///
/// An interrupt is the third, and the one nothing else may ever say twice: a
/// turn cancelled at the pane is over whether or not the vendor mentions it,
/// and a wait holding out for a word that may never come is a wait that runs
/// to its own deadline.
///
/// A subagent's events ride the same log and are not the agent's turn.
fn a_turn_ended(event: &Event) -> bool {
    (turn_end(event) || event.kind == derive::READ_TURN_END || cut_short(event))
        && event.payload["agent_id"].is_null()
}

/// Whether this event is amx cutting the turn short.
fn cut_short(event: &Event) -> bool {
    event.kind == INTERRUPT
}

/// The answer, or the honest absence of one.
///
/// The record is where the answer lives: the Stop hook takes it from the
/// vendor's own payload, and falls back to the transcript. That transcript is
/// written asynchronously, so a hook that arrived a moment too early left
/// nothing behind — and this call, which is standing at the end of the turn
/// anyway, is the right place to look again.
///
/// A vendor that has neither — no hooks to send an answer and no conversation
/// to read one out of — reaches the record another way, and this reads it the
/// same: what a reader read off the pane when it read the screen as a finished
/// turn is written down there like anything else, with `screen` recorded beside
/// it as where it came from. That is `crate::derive`'s reading of a picture
/// rather than the vendor's own words, and a caller that cares which asks the
/// source; nothing here knows one vendor from another.
///
/// A turn that ends with nothing captured is a failure to answer, never an
/// empty success: exit 0 means there is an answer on stdout, and a caller that
/// cannot trust that has nothing to branch on.
fn answer(view: &View, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    let Some(answer) = view.state.result.clone().or_else(|| transcript(view)) else {
        complain!(
            "amx: {} ended its turn, but amx captured no answer",
            view.id()
        );
        return Ok(exit::FAILURE);
    };
    send::line(&send::rendered(&answer, to_terminal), out)?;
    Ok(exit::OK)
}

/// The transcript's own last word, read at the end of the turn, by the shape
/// the record's vendor writes it in.
fn transcript(view: &View) -> Option<String> {
    let path = view.meta.transcript.as_ref()?;
    let text = std::fs::read_to_string(path).ok()?;
    let format = crate::conversation::format_of(view.meta.agent.as_deref().unwrap_or_default())?;
    crate::conversation::answer(format, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Meta;
    use crate::tmux::{PaneId, Socket};
    use serde_json::json;

    fn log(kinds: &[&str]) -> Vec<Event> {
        kinds
            .iter()
            .map(|kind| Event::new(*kind, json!({})))
            .collect()
    }

    #[test]
    fn a_turn_ends_a_wait_and_a_question_interrupts_it() {
        assert_eq!(settled(Phase::Idle, Ended::Turn), Settled::Answer);
        assert_eq!(settled(Phase::Done, Ended::Turn), Settled::Answer);
        assert_eq!(settled(Phase::Waiting, Ended::NotYet), Settled::Question);
        assert_eq!(
            settled(Phase::Waiting, Ended::Turn),
            Settled::Question,
            "a question is a question whatever the log says"
        );
        assert_eq!(settled(Phase::Failed, Ended::NotYet), Settled::Nothing);
        assert_eq!(settled(Phase::Stopped, Ended::NotYet), Settled::Nothing);
    }

    #[test]
    fn a_turn_nobody_let_finish_leaves_no_answer_to_hand_back() {
        // The one ending with an answer on the record that is not this turn's:
        // what amx captured belongs to the turn before the message.
        assert_eq!(
            settled(Phase::Idle, Ended::Interrupted),
            Settled::Interrupted
        );
        assert_eq!(
            settled(Phase::Done, Ended::Interrupted),
            Settled::Interrupted
        );
    }

    #[test]
    fn a_wait_keeps_waiting_while_there_is_a_turn_to_wait_for() {
        for phase in [Phase::Starting, Phase::Working, Phase::Unknown] {
            assert_eq!(settled(phase, Ended::NotYet), Settled::NotYet, "{phase}");
        }
        // Idle, but the turn amx is waiting for has not started yet: the
        // message it was sent is still in front of it.
        assert_eq!(settled(Phase::Idle, Ended::NotYet), Settled::NotYet);
    }

    #[test]
    fn a_command_that_ended_on_an_unanswered_message_is_not_an_answer() {
        // Nothing more is coming, and what is on the record answers the turn
        // before the message. Saying so is the only honest ending.
        assert_eq!(settled(Phase::Done, Ended::NotYet), Settled::Unanswered);
    }

    #[test]
    fn a_wait_reads_the_record_often_and_the_pane_rarely() {
        for evidence in [Evidence::Record, Evidence::Gone, Evidence::Hooks] {
            assert_eq!(pace(&evidence), POLL, "{evidence:?}");
        }
        // An agent amx let go has no pane to read either: what says it is
        // parked is the record, and reading that costs nothing.
        assert_eq!(pace(&Evidence::LetGo), POLL);

        for evidence in [Evidence::Screen, Evidence::Unknown] {
            assert_eq!(pace(&evidence), LOOK, "{evidence:?}");
        }
    }

    #[test]
    fn an_agent_nobody_has_written_to_answers_with_its_last_turn() {
        assert_eq!(ended_past_the_last_message(&log(&[])), Ended::Turn);
        assert_eq!(
            ended_past_the_last_message(&log(&["SessionStart", "UserPromptSubmit", "Stop"])),
            Ended::Turn
        );
    }

    #[test]
    fn an_answer_from_before_the_last_message_is_not_past_it() {
        assert_eq!(
            ended_past_the_last_message(&log(&["Stop", send::SEND])),
            Ended::NotYet
        );
        assert_eq!(
            ended_past_the_last_message(&log(&["Stop", send::SEND, "UserPromptSubmit"])),
            Ended::NotYet
        );
        assert_eq!(
            ended_past_the_last_message(&log(&["Stop", send::SEND, "UserPromptSubmit", "Stop"])),
            Ended::Turn
        );
        // And it is the *last* message that counts.
        assert_eq!(
            ended_past_the_last_message(&log(&[send::SEND, "Stop", send::SEND])),
            Ended::NotYet
        );
    }

    #[test]
    fn a_turn_a_reading_watched_end_ends_a_wait_like_any_other() {
        // On the vendor that sends no Stop, a reading of the pane is the only
        // thing that will ever place the end of a turn, so a wait that took
        // the vendor's word alone was waiting on a word never coming.
        assert_eq!(
            ended_past_the_last_message(&log(&[
                send::SEND,
                derive::READ_PROMPT,
                derive::READ_TURN_END,
            ])),
            Ended::Turn
        );
        assert_eq!(
            ended_past_the_last_message(&log(&[send::SEND, derive::READ_PROMPT])),
            Ended::NotYet,
            "a turn a reading watched begin is under way, not over"
        );
        assert_eq!(
            ended_past_the_last_message(&log(&[derive::READ_TURN_END, send::SEND])),
            Ended::NotYet,
            "and one that ended before the message is the turn before it"
        );
    }

    #[test]
    fn a_turn_amx_cut_short_is_a_turn_that_ended() {
        // A wait that held out for the vendor's word about a turn it was
        // interrupted out of would run to its own deadline over a turn that
        // ended the moment the key landed.
        assert_eq!(
            ended_past_the_last_message(&log(&[send::SEND, "UserPromptSubmit", INTERRUPT])),
            Ended::Interrupted
        );
        // The first word for the turn's end is the one that ended it: a vendor
        // catching up afterwards is not a second ending.
        assert_eq!(
            ended_past_the_last_message(&log(&[send::SEND, INTERRUPT, TURN_END])),
            Ended::Interrupted
        );
        // And an interrupt from before the last message belongs to the turn
        // before it, like any other ending.
        assert_eq!(
            ended_past_the_last_message(&log(&[INTERRUPT, send::SEND])),
            Ended::NotYet
        );
    }

    #[test]
    fn result_over_an_interrupted_turn_hands_back_nothing_and_says_so() {
        // The answer on the record is the turn before the message's, and
        // serving it as this turn's is the mistake nothing downstream can
        // undo. A turn nobody let finish has no answer at all.
        let root = tempfile::TempDir::new().unwrap();
        let waited_on = |id: &str, ended_on: &str| {
            let meta = Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                dir: std::path::PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
                pane: PaneId::new("%404").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            };
            let agent = Agent::create(root.path(), &meta).unwrap();
            let writer = agent.writer().unwrap();
            writer
                .append(&Event::new(
                    send::SEND,
                    json!({ "text": "and now the linter" }),
                ))
                .unwrap();
            writer.append(&Event::new(ended_on, json!({}))).unwrap();
            writer
                .observe(|state| {
                    state.state = Phase::Idle;
                    state.result = Some("the login bug is fixed".to_string());
                    // Parked, so the record's own phase is what a reader hands
                    // back with no pane left to look at.
                    state.parked_at = 4_600;
                })
                .unwrap();
            drop(writer);

            let mut out = Vec::new();
            let code = run(root.path(), id, None, false, &mut out).unwrap();
            (code, String::from_utf8(out).unwrap())
        };

        assert_eq!(
            waited_on("fix-login-a1b", INTERRUPT),
            (exit::FAILURE, String::new())
        );
        // The same wait over a turn the agent was left to finish hands back
        // what it answered, which is what says the ending above is the
        // interrupt rather than the record being unreadable.
        assert_eq!(
            waited_on("fix-login-c3d", TURN_END),
            (exit::OK, "the login bug is fixed\n".to_string())
        );
    }

    /// claude's word for a turn ending, as its entry spells it.
    const TURN_END: &str = "Stop";

    #[test]
    fn a_subagents_turn_is_not_the_agents_turn() {
        let events = vec![
            Event::new(send::SEND, json!({ "text": "and now the linter" })),
            Event::new(TURN_END, json!({ "agent_id": "sub-1" })),
        ];
        assert_eq!(ended_past_the_last_message(&events), Ended::NotYet);
    }
}
