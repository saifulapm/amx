//! `amx result`: wait for the current turn to end and report how it ended.
//!
//! The exit code says which ending happened:
//!
//! * `0`: the answer is on stdout.
//! * `1`: no answer is coming. The agent failed, was stopped, was interrupted,
//!   or ended its turn without an answer amx could capture.
//! * `2`: the agent is asking a question, printed on stdout with its choices. A
//!   wait never continues past a question, since the caller has to answer it.
//! * `3`: the caller's timeout.
//!
//! The turn waited for is the one after the last message sent. `send` logs the
//! message before typing it, so the event log orders it against turn endings;
//! an answer from before the message belongs to the previous turn and must
//! never be returned for this one.

use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::derive::{self, Evidence, View};
use crate::store::{Agent, Event, Phase};
use crate::vendor::{Hooks, Moment};
use crate::verbs::interrupt::INTERRUPT;
use crate::verbs::park::PARKED;
use crate::verbs::send::{self, nothing_more_is_coming, waiting_on_a_question};
use crate::{complain, exit, paths, store};

/// How often the record is read while waiting.
pub(crate) const POLL: Duration = Duration::from_millis(200);

/// How often to read when the reading needs a screen capture, which costs a
/// tmux call where the record costs two small file reads.
pub(crate) const LOOK: Duration = Duration::from_secs(1);

fn moment(hooks: Option<&Hooks>, event: &Event) -> Option<Moment> {
    hooks?.moment(&event.kind)
}

/// Whether this event is the vendor's own turn-end hook.
fn turn_end(hooks: Option<&Hooks>, event: &Event) -> bool {
    moment(hooks, event) == Some(Moment::Ended)
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
    let mut turns = Turns::of(root, id)?;

    loop {
        let view = derive::view(root, id, store::now())?;
        let phase = view.phase();
        match settled(phase, turns.ended(phase)) {
            Settled::Answer => return answer(&view, to_terminal, out),
            Settled::Question => return waiting_on_a_question(&view, to_terminal, out),
            // A command exited before answering the last message.
            Settled::Unanswered => {
                complain!("amx: {id} ended without answering");
                return Ok(exit::FAILURE);
            }
            // The recorded answer belongs to the turn before the message.
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

/// Run the verb over a parent's children, against the machine.
pub fn family_from_env(parent: &str, timeout: Option<u64>, json: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run_family(
        &root,
        parent,
        timeout.map(Duration::from_secs),
        json,
        to_terminal,
        &mut out,
    )
}

/// `result` over a parent's children: wait as `amx wait` does, then report
/// every child.
///
/// Every child is reported, failed ones included. The exit code is the most
/// actionable one found: `3` timed out, `2` a child is asking, `1` a child
/// failed, `0` every child answered.
pub fn run_family(
    root: &Path,
    parent: &str,
    timeout: Option<Duration>,
    json: bool,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    let children = crate::verbs::wait::children_of(root, parent)?;
    // An empty family must not exit 0, which would say every child answered.
    if children.is_empty() {
        complain!("amx result: {parent} has no children");
        return Ok(exit::FAILURE);
    }
    let waited = crate::verbs::wait::run(
        root,
        &children,
        None,
        false,
        None,
        timeout,
        &mut std::io::sink(),
    )?;
    let timed_out = waited == exit::TIMEOUT;

    let mut waiting = false;
    let mut failed = false;
    let mut family = serde_json::Map::new();
    for id in &children {
        let view = derive::view(root, id, store::now())?;
        let phase = view.phase();
        let settled = settled(phase, Turns::of(root, id)?.ended(phase));
        // After an interrupt or an unanswered message the recorded answer is
        // the previous turn's.
        let answer = match settled {
            Settled::Interrupted | Settled::Unanswered => None,
            _ => view.state.result.clone().or_else(|| transcript(&view)),
        };
        match settled {
            Settled::Question => waiting = true,
            Settled::Nothing | Settled::Interrupted | Settled::Unanswered => failed = true,
            Settled::Answer if answer.is_none() => failed = true,
            _ => {}
        }
        if json {
            family.insert(id.clone(), answer_json(&view, answer));
        } else {
            writeln!(out, "{id} {phase}")?;
            if let Some(question) = &view.state.question {
                super::print_question(question, &view.state.options, to_terminal, out)?;
            } else if let Some(answer) = &answer {
                send::line(&send::rendered(answer, to_terminal), out)?;
            }
        }
    }
    if json {
        send::line(&serde_json::Value::Object(family).to_string(), out)?;
    }

    Ok(if timed_out {
        exit::TIMEOUT
    } else if waiting {
        exit::BLOCKED
    } else if failed {
        exit::FAILURE
    } else {
        exit::OK
    })
}

/// One agent's answer as `result --children --json` and `sub --json` print it.
pub(crate) fn answer_json(view: &View, answer: Option<String>) -> serde_json::Value {
    serde_json::json!({
        "phase": view.phase().as_str(),
        "answer": answer,
        "evidence": view.verdict.evidence,
        "question": view.state.question,
        "options": view.state.options,
        "kind": view.kind(),
    })
}

/// What one reading means to a caller waiting for an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settled {
    /// The turn is over; the captured answer, if any, is the result.
    Answer,
    /// Waiting on a question; the wait ends here.
    Question,
    /// A command ended before the last message was answered.
    Unanswered,
    /// The turn was cut short and has no answer.
    Interrupted,
    /// Failed or stopped; no answer is coming.
    Nothing,
    /// Still going.
    NotYet,
}

/// Whether the turn after the last message has ended, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    /// Nothing since the last message ends a turn.
    NotYet,
    /// The agent's turn ended; what it left is the answer.
    Turn,
    /// amx cut the turn short (interrupt, resume or park). There is no answer,
    /// and the recorded one belongs to the previous turn.
    Interrupted,
}

/// Classify one reading, given how the turn after the last message ended.
pub(crate) fn settled(phase: Phase, ended: Ended) -> Settled {
    match (phase, ended) {
        (Phase::Waiting, _) => Settled::Question,
        (Phase::Idle | Phase::Done, Ended::Interrupted) => Settled::Interrupted,
        (Phase::Idle | Phase::Done, Ended::Turn) => Settled::Answer,
        // Idle with a message not yet taken: its turn is still to come.
        (Phase::Idle, Ended::NotYet) => Settled::NotYet,
        (Phase::Done, Ended::NotYet) => Settled::Unanswered,
        (Phase::Failed | Phase::Stopped, _) => Settled::Nothing,
        (Phase::Starting | Phase::Working | Phase::Unknown, _) => Settled::NotYet,
    }
}

/// How long to wait before reading again, given what the last reading cost.
pub(crate) fn pace(evidence: &Evidence) -> Duration {
    match evidence {
        Evidence::Screen | Evidence::Unknown => LOOK,
        Evidence::Record | Evidence::Gone | Evidence::LetGo | Evidence::Hooks => POLL,
    }
}

/// The event `amx resume` logs; must match `crate::verbs::resume`.
const RESUMED: &str = "resume";

/// A [`Fold`] over one agent's event log that reads only what was appended
/// since the previous call, so polling does not reparse the whole log.
pub(crate) struct Turns {
    log: PathBuf,
    hooks: Option<Hooks>,
    read: u64,
    fold: Fold,
}

impl Turns {
    pub(crate) fn of(root: &Path, id: &str) -> Result<Turns> {
        let agent = Agent::open(root, id)?;
        let hooks = crate::vendor::hooks_for(agent.meta()?.agent.as_deref().unwrap_or_default());
        Ok(Turns {
            log: agent.events_path(),
            hooks,
            read: 0,
            fold: Fold::START,
        })
    }

    /// How the awaited turn ended, for a record in `phase`. Only idle and done
    /// records need the log; any other phase is `NotYet`.
    pub(crate) fn ended(&mut self, phase: Phase) -> Ended {
        if !matches!(phase, Phase::Idle | Phase::Done) {
            return Ended::NotYet;
        }
        if let Some((fresh, next)) = crate::verbs::events::grown(&self.log, self.read) {
            // A log shorter than what was read is a new record under the same
            // id; start over.
            if next < self.read {
                self.fold = Fold::START;
            }
            self.read = next;
            for event in fresh
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
            {
                self.fold.step(self.hooks.as_ref(), &event);
            }
        }
        self.fold.ended
    }
}

/// Whether the last turn has ended and how, folded over the log one event at a
/// time.
///
/// Log order decides, since the log is appended under one lock. A turn opens
/// at a sent message, at a prompt when none is open, and at spawn for the
/// initial task, so an agent never sent a message answers with its last turn.
/// The first ending of an open turn counts; a vendor Stop arriving after an
/// interrupt does not change it. Only a message makes the previous answer
/// stale. What counts as an ending is [`a_turn_ended`].
#[derive(Debug, Clone, Copy)]
struct Fold {
    ended: Ended,
    open: bool,
}

impl Fold {
    const START: Fold = Fold {
        ended: Ended::Turn,
        open: true,
    };

    fn step(&mut self, hooks: Option<&Hooks>, event: &Event) {
        if event.kind == send::SEND {
            self.ended = Ended::NotYet;
            self.open = true;
        } else if a_turn_ended(hooks, event) {
            if self.open {
                self.ended = match cut_short(event) {
                    true => Ended::Interrupted,
                    false => Ended::Turn,
                };
            }
            self.open = false;
        } else if a_turn_began(hooks, event) {
            self.open = true;
        }
    }
}

/// Whether this event is a prompt reaching the agent, from a hook or read off
/// the pane.
fn a_turn_began(hooks: Option<&Hooks>, event: &Event) -> bool {
    let moment = moment(hooks, event);
    (matches!(moment, Some(Moment::Prompted | Moment::Taken)) || event.kind == derive::READ_PROMPT)
        && event.payload["agent_id"].is_null()
}

/// Whether this event ends one of the agent's own turns.
///
/// Three sources count: the vendor's turn-end hook, [`derive::READ_TURN_END`]
/// for vendors without hooks (their turns end only on a screen reading), and
/// amx cutting the turn short (see [`cut_short`]), which vendors may never
/// report. Subagent events share the log and are ignored.
fn a_turn_ended(hooks: Option<&Hooks>, event: &Event) -> bool {
    (turn_end(hooks, event) || event.kind == derive::READ_TURN_END || cut_short(event))
        && event.payload["agent_id"].is_null()
}

/// Whether amx ended the turn itself: an interrupt, or a resume or park that
/// replaced the process the turn ran in.
fn cut_short(event: &Event) -> bool {
    [INTERRUPT, RESUMED, PARKED].contains(&event.kind.as_str())
}

/// Print the answer, or fail when none was captured.
///
/// The record holds the answer, taken from the Stop hook's payload or the
/// transcript. The vendor writes the transcript asynchronously, so a hook that
/// fired early may have found nothing, and the transcript is read again here.
/// For vendors without hooks or transcripts, the record holds what a reading
/// took off the screen (with source `screen`).
///
/// A missing answer is exit 1: exit 0 always means an answer is on stdout.
fn answer(view: &View, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    let Some(answer) = view.state.result.clone().or_else(|| transcript(view)) else {
        match why_it_stopped(view) {
            Some(why) => complain!(
                "amx: {} ended its turn, but amx captured no answer: {why}",
                view.id()
            ),
            None => complain!(
                "amx: {} ended its turn, but amx captured no answer",
                view.id()
            ),
        }
        return Ok(exit::FAILURE);
    };
    send::line(&send::rendered(&answer, to_terminal), out)?;
    Ok(exit::OK)
}

/// The answer at the end of the transcript, in the vendor's format.
fn transcript(view: &View) -> Option<String> {
    read_transcript(view, crate::conversation::answer)
}

/// Why the transcript's last message has no answer, where it says: a token
/// limit, a provider error, an aborted turn. Read only on the failure path.
fn why_it_stopped(view: &View) -> Option<String> {
    read_transcript(view, crate::conversation::why_it_stopped)
}

/// Ask `ask` of the tail of the record's transcript.
fn read_transcript(
    view: &View,
    ask: fn(crate::vendor::Transcript, &str) -> Option<String>,
) -> Option<String> {
    let format = crate::conversation::format_of(view.meta.agent.as_deref().unwrap_or_default())?;
    ask(format, &Agent::transcript_tail(&view.meta)?)
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

    fn turn_ended(hooks: Option<&Hooks>, events: &[Event]) -> Ended {
        let mut fold = Fold::START;
        for event in events {
            fold.step(hooks, event);
        }
        fold.ended
    }

    /// How the last turn ended, read with claude's hooks.
    fn claudes(events: &[Event]) -> Ended {
        turn_ended(Some(&crate::vendor::claude::HOOKS), events)
    }

    #[test]
    fn a_turn_ends_in_the_words_of_the_records_own_vendor() {
        // Each vendor's turn-end event counts only for that vendor.
        let pi = crate::vendor::pi::VENDOR.hooks;
        assert_eq!(
            turn_ended(pi.as_ref(), &log(&[send::SEND, "agent_settled"])),
            Ended::Turn
        );
        assert_eq!(
            turn_ended(pi.as_ref(), &log(&[send::SEND, "Stop"])),
            Ended::NotYet
        );
        assert_eq!(claudes(&log(&[send::SEND, "agent_settled"])), Ended::NotYet);
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
        // The recorded answer belongs to the turn before the message.
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
        // Idle with the message not yet taken: the awaited turn has not
        // started.
        assert_eq!(settled(Phase::Idle, Ended::NotYet), Settled::NotYet);
    }

    #[test]
    fn a_command_that_ended_on_an_unanswered_message_is_not_an_answer() {
        // No answer is coming, and the recorded one is the previous turn's.
        assert_eq!(settled(Phase::Done, Ended::NotYet), Settled::Unanswered);
    }

    #[test]
    fn a_wait_reads_the_record_often_and_the_pane_rarely() {
        for evidence in [Evidence::Record, Evidence::Gone, Evidence::Hooks] {
            assert_eq!(pace(&evidence), POLL, "{evidence:?}");
        }
        // A parked agent has no pane; the record alone says so.
        assert_eq!(pace(&Evidence::LetGo), POLL);

        for evidence in [Evidence::Screen, Evidence::Unknown] {
            assert_eq!(pace(&evidence), LOOK, "{evidence:?}");
        }
    }

    #[test]
    fn an_agent_nobody_has_written_to_answers_with_its_last_turn() {
        assert_eq!(claudes(&log(&[])), Ended::Turn);
        assert_eq!(
            claudes(&log(&["SessionStart", "UserPromptSubmit", "Stop"])),
            Ended::Turn
        );
    }

    #[test]
    fn an_answer_from_before_the_last_message_is_not_past_it() {
        assert_eq!(claudes(&log(&["Stop", send::SEND])), Ended::NotYet);
        assert_eq!(
            claudes(&log(&["Stop", send::SEND, "UserPromptSubmit"])),
            Ended::NotYet
        );
        assert_eq!(
            claudes(&log(&["Stop", send::SEND, "UserPromptSubmit", "Stop"])),
            Ended::Turn
        );
        // Only the last message counts.
        assert_eq!(
            claudes(&log(&[send::SEND, "Stop", send::SEND])),
            Ended::NotYet
        );
    }

    #[test]
    fn a_turn_a_reading_watched_end_ends_a_wait_like_any_other() {
        // A vendor without a Stop hook ends turns only through a screen
        // reading, so that must end a wait too.
        assert_eq!(
            claudes(&log(&[
                send::SEND,
                derive::READ_PROMPT,
                derive::READ_TURN_END,
            ])),
            Ended::Turn
        );
        assert_eq!(
            claudes(&log(&[send::SEND, derive::READ_PROMPT])),
            Ended::NotYet,
            "a turn a reading watched begin is under way, not over"
        );
        assert_eq!(
            claudes(&log(&[derive::READ_TURN_END, send::SEND])),
            Ended::NotYet,
            "and one that ended before the message is the turn before it"
        );
    }

    #[test]
    fn a_turn_amx_cut_short_is_a_turn_that_ended() {
        // Vendors may never report an interrupted turn, so the interrupt
        // itself ends it.
        assert_eq!(
            claudes(&log(&[send::SEND, "UserPromptSubmit", INTERRUPT])),
            Ended::Interrupted
        );
        // The first ending counts; a later vendor Stop does not replace it.
        assert_eq!(
            claudes(&log(&[send::SEND, INTERRUPT, TURN_END])),
            Ended::Interrupted
        );
        // An interrupt before the last message belongs to the previous turn.
        assert_eq!(claudes(&log(&[INTERRUPT, send::SEND])), Ended::NotYet);
    }

    #[test]
    fn result_over_an_interrupted_turn_hands_back_nothing_and_says_so() {
        // The recorded answer is the previous turn's and must not be returned.
        let root = tempfile::TempDir::new().unwrap();
        let waited_on = |id: &str, ended_on: &str| {
            let meta = Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
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
                    // Parked, so the reading returns the recorded phase.
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
        // Control: the same record ending on Stop returns the answer, so the
        // failure above comes from the interrupt.
        assert_eq!(
            waited_on("fix-login-c3d", TURN_END),
            (exit::OK, "the login bug is fixed\n".to_string())
        );
    }

    #[test]
    fn an_interrupt_with_no_message_before_it_still_cut_the_turn_short() {
        // The spawn task's turn, with no message sent, can be interrupted too.
        assert_eq!(
            claudes(&log(&["SessionStart", "UserPromptSubmit", INTERRUPT])),
            Ended::Interrupted
        );
        // So can a turn typed into the pane by hand.
        assert_eq!(
            claudes(&log(&[
                "UserPromptSubmit",
                TURN_END,
                "UserPromptSubmit",
                INTERRUPT
            ])),
            Ended::Interrupted
        );
        // An interrupt after the turn already ended changes nothing.
        assert_eq!(
            claudes(&log(&["UserPromptSubmit", TURN_END, INTERRUPT])),
            Ended::Turn
        );
    }

    #[test]
    fn a_resume_or_a_park_ends_the_turn_a_message_left_open() {
        // Both replace the process the turn ran in.
        assert_eq!(claudes(&log(&[send::SEND, RESUMED])), Ended::Interrupted);
        assert_eq!(claudes(&log(&[send::SEND, PARKED])), Ended::Interrupted);
        // A park after the turn ended is not a second ending.
        assert_eq!(claudes(&log(&[send::SEND, TURN_END, PARKED])), Ended::Turn);
        assert_eq!(claudes(&log(&[TURN_END, PARKED])), Ended::Turn);
        // A resume with a message logs the message after itself, so that turn
        // is still to come.
        assert_eq!(
            claudes(&log(&[send::SEND, RESUMED, send::SEND])),
            Ended::NotYet
        );
    }

    /// A parent `lead-a1b` and a parked child `scout-c3d` waiting on a question.
    fn a_family_with_a_question(root: &Path) {
        let meta = |id: &str, parent: Option<&str>, created: u64| Meta {
            role: None,
            parent: parent.map(str::to_string),
            depth: u32::from(parent.is_some()),
            id: id.to_string(),
            task: "find the flaky test".to_string(),
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created,
        };
        Agent::create(root, &meta("lead-a1b", None, 1)).unwrap();
        Agent::create(root, &meta("scout-c3d", Some("lead-a1b"), 2))
            .unwrap()
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Waiting;
                state.question = Some("Which runner?".to_string());
                state.options = vec!["Node".to_string(), "Deno".to_string()];
                state.kind = Some(crate::store::Kind::Question);
                state.parked_at = 4_600;
            })
            .unwrap();
    }

    #[test]
    fn result_children_json_carries_what_answer_needs() {
        let root = tempfile::TempDir::new().unwrap();
        a_family_with_a_question(root.path());

        let mut out = Vec::new();
        let code = run_family(root.path(), "lead-a1b", None, true, false, &mut out).unwrap();

        assert_eq!(code, exit::BLOCKED);
        let family: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let child = &family["scout-c3d"];
        assert_eq!(child["question"], "Which runner?");
        assert_eq!(child["options"], json!(["Node", "Deno"]));
        assert_eq!(child["kind"], "question");
    }

    #[test]
    fn result_children_numbers_the_choices_answer_takes() {
        let root = tempfile::TempDir::new().unwrap();
        a_family_with_a_question(root.path());

        let mut out = Vec::new();
        let code = run_family(root.path(), "lead-a1b", None, false, false, &mut out).unwrap();

        assert_eq!(code, exit::BLOCKED);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "scout-c3d waiting\nWhich runner?\n1. Node\n2. Deno\n"
        );
    }

    #[test]
    fn result_children_of_a_childless_parent_is_a_failure() {
        // Exit 0 with nothing printed would read as every child answering.
        let root = tempfile::TempDir::new().unwrap();
        a_family_with_a_question(root.path());

        for json in [false, true] {
            let mut out = Vec::new();
            let code = run_family(root.path(), "scout-c3d", None, json, false, &mut out).unwrap();
            assert_eq!(code, exit::FAILURE, "json {json}");
            assert!(out.is_empty(), "json {json}: {out:?}");
        }
    }

    #[test]
    fn result_children_refuses_a_child_whose_record_goes_mid_wait() {
        // A child removed mid-wait (`amx stop --delete`) ends the wait with an
        // error at once, as `amx wait` does.
        let root = tempfile::TempDir::new().unwrap();
        a_family_with_a_question(root.path());
        let child = Agent::open(root.path(), "scout-c3d").unwrap();
        let writer = child.writer().unwrap();
        writer
            .append(&Event::new(send::SEND, json!({ "text": "and the linter" })))
            .unwrap();
        writer
            .observe(|state| {
                state.state = Phase::Idle;
                state.question = None;
            })
            .unwrap();
        drop(writer);

        let started = Instant::now();
        let waited = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                child.remove().unwrap();
            });
            let patience = Some(Duration::from_secs(10));
            run_family(
                root.path(),
                "lead-a1b",
                patience,
                false,
                false,
                &mut Vec::new(),
            )
        });

        let refused = waited.expect_err("a child that is gone");
        assert!(refused.to_string().contains("scout-c3d"), "{refused:#}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn result_reads_the_answer_off_the_end_of_a_long_transcript() {
        let root = tempfile::TempDir::new().unwrap();
        let kept = tempfile::TempDir::new().unwrap();
        let transcript = kept.path().join("session.jsonl");
        let mut text = String::new();
        for n in 0..2_000 {
            let filler = "x".repeat(100);
            text.push_str(&format!(
                "{{\"type\":\"user\",\"message\":{{\"content\":\"step {n} {filler}\"}}}}\n"
            ));
        }
        text.push_str(concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":",
            "[{\"type\":\"text\",\"text\":\"the login bug is fixed\"}]}}\n",
        ));
        std::fs::write(&transcript, text).unwrap();

        a_family_with_a_question(root.path());
        let agent = Agent::open(root.path(), "scout-c3d").unwrap();
        let writer = agent.writer().unwrap();
        writer
            .update_meta(|meta| meta.transcript = Some(transcript.clone()))
            .unwrap();
        writer.append(&Event::new(TURN_END, json!({}))).unwrap();
        writer
            .observe(|state| {
                state.state = Phase::Idle;
                state.question = None;
                state.options.clear();
            })
            .unwrap();
        drop(writer);

        let mut out = Vec::new();
        let code = run(root.path(), "scout-c3d", None, false, &mut out).unwrap();
        assert_eq!(
            (code, String::from_utf8(out).unwrap()),
            (exit::OK, "the login bug is fixed\n".to_string())
        );
    }

    #[test]
    fn turns_fold_in_what_the_log_grew_since_the_last_look() {
        let root = tempfile::TempDir::new().unwrap();
        a_family_with_a_question(root.path());
        let agent = Agent::open(root.path(), "scout-c3d").unwrap();
        let said = |kind: &str| {
            agent
                .writer()
                .unwrap()
                .append(&Event::new(kind, json!({})))
                .unwrap();
        };

        let mut turns = Turns::of(root.path(), "scout-c3d").unwrap();
        assert_eq!(turns.ended(Phase::Idle), Ended::Turn);
        said(send::SEND);
        assert_eq!(turns.ended(Phase::Idle), Ended::NotYet);
        assert_eq!(turns.ended(Phase::Working), Ended::NotYet);
        said("UserPromptSubmit");
        said(TURN_END);
        assert_eq!(turns.ended(Phase::Idle), Ended::Turn);

        // A partial line is read once it is complete.
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(agent.events_path())
            .unwrap();
        write!(log, "{{\"at\":1,\"kind\":\"{}\"", send::SEND).unwrap();
        assert_eq!(turns.ended(Phase::Idle), Ended::Turn);
        writeln!(log, ",\"payload\":{{}}}}").unwrap();
        assert_eq!(turns.ended(Phase::Idle), Ended::NotYet);

        // A log shorter than what was read is new and is read from its start.
        std::fs::write(agent.events_path(), "").unwrap();
        said(TURN_END);
        assert_eq!(turns.ended(Phase::Idle), Ended::Turn);
    }

    /// claude's turn-end hook event.
    const TURN_END: &str = "Stop";

    #[test]
    fn a_subagents_turn_is_not_the_agents_turn() {
        let events = vec![
            Event::new(send::SEND, json!({ "text": "and now the linter" })),
            Event::new(TURN_END, json!({ "agent_id": "sub-1" })),
        ];
        assert_eq!(claudes(&events), Ended::NotYet);
    }
}
