//! Works out what an agent is doing at the moment somebody asks.
//!
//! No amx process stays resident to track agents, so each reader derives the
//! phase from the record, the vendor's hooks and the pane, in this order:
//!
//! 1. An exit code or `stop` on the record is final.
//! 2. No pane, or a pane that now answers for another agent, is `stopped`.
//!    See [`crate::tmux::Server::pane_answers_for`].
//! 3. A record naming no vendor is a command: a live pane means it is running,
//!    and its last printed line is the summary. See [`read_a_command`].
//! 4. Hooks or a heartbeat heard within [`FRESH`] seconds decide. The pane is
//!    still read for a question the hooks did not name, and for the spinner
//!    line of a turn with no tool call yet. A turn `amx interrupt` cut short
//!    skips this step until the vendor's next event; see [`cut_short`].
//! 5. Otherwise the pane is matched against the vendor's screen rules; see
//!    [`own_screens`].
//! 6. A screen no rule claims is `unknown`, except that a reporting vendor's
//!    record at idle or waiting keeps its phase; see [`reports`].
//!
//! A reader mostly concludes and forgets. It writes down only what the pane
//! alone can say:
//!
//! - a question and its choices read off the screen ([`note`]);
//! - when the current screen first appeared, so a quiescent rule can wait
//!   across processes ([`held_still`]);
//! - on a vendor without hooks, the phase and turn edges it read, and the
//!   screen a finished turn left as its answer ([`write_the_reading`]);
//! - on a vendor with hooks, the turn edges it never reports, such as esc in
//!   the pane ([`hear_what_went_unsaid`]).
//!
//! Every surface is handed one [`View`], whose fields agree with its phase.
//! Where a project sets `summary_command`, a reader that stays (the view) also
//! has it write a one-line summary of the turn; see [`wants_a_line`].

use anyhow::Result;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use crate::rules::{Claim, Ruleset, SETTLED_LOOKS};
use crate::store::{Agent, Edge, Event, Meta, Phase, Question, Source, State, Still};
use crate::tmux::Server;
use crate::vendor::{Capability, Moment, Vendor};

/// How long the vendor's own events are trusted over the pane, in seconds.
///
/// Long enough to cover the gap between two tool calls, short enough that an
/// interrupted agent stops reading as working while somebody watches.
pub const FRESH: u64 = 8;

/// The log event for a turn start that a reader saw on the pane.
///
/// Only a vendor without hooks gets these. They carry amx's own name so the
/// log never passes a reading off as the vendor's account of itself.
pub const READ_PROMPT: &str = "read.prompt";

/// The log event for a turn end that a reader saw on the pane. See
/// [`READ_PROMPT`].
pub const READ_TURN_END: &str = "read.turn-end";

/// Where a verdict came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Evidence {
    /// The record says how the run ended.
    Record,
    /// The pane is gone.
    Gone,
    /// amx closed the pane of an idle, unwatched agent. The phase is the
    /// record's, since nothing about the agent ended. See
    /// [`crate::store::State`]'s `parked_at`.
    #[serde(rename = "parked")]
    LetGo,
    /// The vendor's own events, recent enough to trust.
    Hooks,
    /// The screen: a rule that claimed it, or a command's own output.
    Screen,
    /// Nothing accounts for the screen.
    Unknown,
}

/// A reader's conclusion about an agent, and its evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub phase: Phase,
    pub evidence: Evidence,
    /// The rule that claimed the screen, if one did.
    pub rule: Option<String>,
    /// The seconds shown beside the agent; see [`clock`]. For an ended run,
    /// how long it worked; for a waiting agent, how long it has waited;
    /// otherwise, how long since it was last heard from.
    pub age: u64,
    /// Seconds worked so far; see [`worked`]. Ticks while working, holds while
    /// waiting or idle, and freezes at the end. Rows and tables print this;
    /// the card and `--json` use `age`.
    pub worked: u64,
}

/// An agent as a reader sees it: the record and the verdict on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub meta: Meta,
    pub state: State,
    pub verdict: Verdict,
    /// The vendor's spinner line as found on the pane, e.g.
    /// `Nesting… (15s · ↓ 1.3k tokens)`, for the card's rule. Kept apart from
    /// the summary. `None` if the pane was not read or nothing was spinning.
    pub doing: Option<String>,
}

impl View {
    /// Builds a view whose parts agree.
    ///
    /// A question belongs to the turn waiting on it, so it is dropped unless
    /// the verdict is waiting or `unknown` (which cannot tell the question was
    /// answered). A placeholder question is dropped too; see
    /// [`forget_the_placeholder`].
    pub fn new(meta: Meta, mut state: State, verdict: Verdict) -> View {
        if !matches!(verdict.phase, Phase::Waiting | Phase::Unknown) {
            state.asks(None);
        }
        forget_the_placeholder(own_screens(&meta), &mut state);
        View {
            meta,
            state,
            verdict,
            doing: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.meta.id
    }

    pub fn phase(&self) -> Phase {
        self.verdict.phase
    }

    /// The one line describing the agent: its question, else its summary,
    /// else its answer.
    ///
    /// An answer read off a pane is the whole cut screen (see [`said`]), so
    /// only its last paragraph is used; see [`last_said`].
    pub fn line(&self) -> Option<&str> {
        let said = self
            .state
            .result
            .as_deref()
            .map(|result| match self.state.source {
                Some(Source::Screen) => last_said(result),
                _ => result,
            });
        self.state
            .question
            .as_deref()
            .or(self.state.summary.as_deref())
            .or(said)
    }

    /// What kind of thing the agent is being asked, if anything.
    ///
    /// The record's kind wins over the one read off the screen, as hooks win
    /// over rules everywhere, except for the vendor's question menu. That
    /// screen is unambiguous, and older amx versions wrote `permission` over
    /// every menu; a permission box's key sent to a menu answers a question
    /// nobody chose.
    pub fn kind(&self) -> Option<crate::store::Kind> {
        match asked_kind(own_screens(&self.meta), self.verdict.rule.as_deref()) {
            seen @ Some(crate::store::Kind::Question) => seen,
            seen => self.state.kind.or(seen),
        }
    }

    /// The stable shape `--json` prints. Fields are added, never renamed or
    /// removed: callers branch on them.
    ///
    /// Pull requests are read from what the last look saved beside the record
    /// (see [`crate::pr::written`]); nothing here waits on the forge.
    pub fn json(&self) -> serde_json::Value {
        self.json_beside(&crate::pr::written(&self.meta))
    }

    /// [`View::json`] with the pull requests already read.
    fn json_beside(&self, prs: &[crate::pr::Pr]) -> serde_json::Value {
        // Read once for both fields.
        let (context, last_words) = match &transcript(&self.meta) {
            Some((format, tail)) => crate::conversation::context_and_last_words(*format, tail),
            None => (None, None),
        };
        serde_json::json!({
            "id": self.meta.id,
            // Parent and depth let a program nest rows as the view does. Null
            // and 0 for a root.
            "parent": self.meta.parent,
            "depth": self.meta.depth,
            // The role the spawn named, if any.
            "role": self.meta.role,
            "state": self.verdict.phase.as_str(),
            "evidence": self.verdict.evidence,
            "rule": self.verdict.rule,
            // The row's seconds (see `clock`), plus the stamps behind them for
            // a caller that wants a different measure.
            "age": self.verdict.age,
            "since": self.state.since,
            "last_event": self.state.last_event,
            "ended": self.state.ended,
            // The spans of work added up so far, excluding the one in progress.
            "worked": self.state.worked,
            "seq": self.state.seq,
            "summary": self.state.summary,
            "question": self.state.question,
            "options": self.state.options,
            // Whether `options` carry amx's own numbers (a walked list) rather
            // than the vendor's, which changes the key that picks one.
            "walked": self.state.walked,
            // The whole call the question came from: each question with its
            // choices and descriptions. `multi` is for the one showing.
            "questions": self.state.asking,
            "multi": self.state.multi(),
            "result": self.state.result,
            "source": self.state.source.map(source_name),
            "exit": self.state.exit,
            "kind": self.kind(),
            // The branch's open pull requests, newest live first. `standing`
            // carries what the row's colour means.
            "pr": prs,
            "task": self.meta.task,
            // How the vendor was launched. `agent` is null on a command row;
            // a dial nobody set is null.
            "agent": self.meta.agent,
            "model": self.meta.model,
            "effort": self.meta.effort,
            "dir": self.meta.dir,
            "worktree": self.meta.worktree,
            "branch": self.meta.branch,
            "base": self.meta.base,
            "pane": self.meta.pane.as_str(),
            "socket": self.meta.socket,
            "session": self.meta.session,
            // The name somebody gave this agent (see `crate::verbs::rename`),
            // or null. The id is unchanged either way.
            "name": self.state.name,
            // The vendor's title for the session, which the wall shows in
            // place of the id.
            "session_title": self.state.session_title,
            "created": self.meta.created,
            // Read off the transcript, not the record: what the next turn
            // would send back to the vendor, and the reader's last words. Null
            // without a transcript.
            "context": context,
            "last_words": last_words,
        })
    }
}

/// The vendor's transcript format for this record and the tail of its
/// transcript, or `None` unless both are there to read.
fn transcript(meta: &Meta) -> Option<(crate::vendor::Transcript, String)> {
    let format = crate::conversation::format_of(meta.agent.as_deref().unwrap_or_default())?;
    let tail = Agent::transcript_tail(meta)?;
    Some((format, tail))
}

/// The screen rules for the vendor the record says runs in this pane.
///
/// A record naming no command (every command row, and records from before the
/// field existed) reads the default vendor's rules.
fn own_screens(meta: &Meta) -> &'static Ruleset {
    crate::rules::of(meta.agent.as_deref().unwrap_or_default())
}

/// The kind of answer the claimed screen wants, looked up by rule name in the
/// document that named it. See [`View::kind`].
fn asked_kind(screens: &Ruleset, rule: Option<&str>) -> Option<crate::store::Kind> {
    let name = rule?;
    screens.rules().iter().find(|rule| rule.name == name)?.kind
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Payload => "payload",
        Source::Transcript => "transcript",
        Source::Screen => "screen",
    }
}

/// What a reader made of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub verdict: Verdict,
    /// The question read off the screen, when the screen was read. Hook
    /// questions are on the record already; the screen adds their choices.
    pub asking: Option<Question>,
    /// The vendor's spinner line, when a rule read a running turn. Current
    /// only for the second it was read, so never recorded.
    ///
    /// For a command, its last printed line; see [`read_a_command`].
    pub doing: Option<String>,
    /// The screen with the vendor's chrome cut off, when a rule read a
    /// finished turn; see [`said`]. Whether it is worth keeping is decided at
    /// the write; see [`answers_on_the_pane`].
    pub said: Option<String>,
    /// Whether a quiescent rule was allowed to read the screen as a finished
    /// turn with no shell or agent still running on the footer. The only
    /// reading that may end a reporting vendor's turn; see
    /// [`hear_what_went_unsaid`].
    pub settled: bool,
}

/// Whether a question says nothing about what is asked: empty, or one of the
/// vendor's placeholder sentences listed in its screens document.
fn placeholder(screens: &Ruleset, question: &str) -> bool {
    let question = question.trim();
    question.is_empty() || screens.placeholder(question)
}

/// Drops a placeholder question, and its choices, from the state.
///
/// A placeholder reads like an answer, so a caller would stop looking. The
/// kind stays: the vendor did say that much, and it decides what may be sent.
fn forget_the_placeholder(screens: &Ruleset, state: &mut State) {
    if state
        .question
        .as_deref()
        .is_some_and(|question| placeholder(screens, question))
    {
        state.question = None;
        state.options.clear();
        state.walked = false;
    }
}

/// The vendor's spinner line, e.g. `✽ Nesting… (15s · still thinking)`.
///
/// Found by the vendor's spinner anchors, on the lowest matching row within
/// [`crate::rules::FLOOR_LINES`], so agent output higher up is not taken for
/// chrome. The leading glyph is dropped.
///
/// If the row above is the vendor's hidden-reasoning row (pi's `Thinking...`,
/// see [`Furniture::thinking`]), that row is returned: the status line says
/// `Working` either way.
///
/// [`Furniture::thinking`]: crate::furniture::Furniture::thinking
fn doing(screens: &Ruleset, capture: &str) -> Option<String> {
    let furniture = screens.furniture();
    let rows: Vec<&str> = capture.lines().collect();
    let floor = rows.len().saturating_sub(crate::rules::FLOOR_LINES);
    let at = rows[floor..]
        .iter()
        .rposition(|row| furniture.spinning(row))?
        + floor;
    if let Some(above) = rows[..at].iter().rev().find(|row| !row.trim().is_empty())
        && furniture.thinking(above)
    {
        return Some(above.trim().to_string());
    }
    Some(unglyphed(furniture.unruled(rows[at])))
}

/// Whether the screen shows a turn cut short in the pane; see
/// [`crate::furniture::Furniture::cut_by_hand`].
fn cut_by_hand_in_the_pane(rules: &Ruleset, capture: &str) -> bool {
    let rows: Vec<&str> = capture.lines().collect();
    rules.furniture().cut_by_hand(&rows)
}

/// Whether the footer says a shell is still running. A vendor that never says
/// is taken at the record's count: treating silence as "none" would end every
/// turn with a shell left running on unmeasured vendors.
fn still_has_a_shell(rules: &Ruleset, capture: &str) -> bool {
    let rows: Vec<&str> = capture.lines().collect();
    rules.furniture().shells_running(&rows).unwrap_or(true)
}

/// Whether the footer shows a shell or agent running. Unlike
/// [`still_has_a_shell`], a vendor that never says shows none.
fn shows_a_shell(rules: &Ruleset, capture: &str) -> bool {
    let rows: Vec<&str> = capture.lines().collect();
    rules.furniture().shells_running(&rows) == Some(true)
}

/// The screen of a finished turn with the vendor's chrome and the blank rows
/// at either end cut off: the same walk the card and `amx logs` use.
fn said(screens: &Ruleset, capture: &str) -> Option<String> {
    let rows: Vec<&str> = capture.lines().collect();
    let mut said = screens.furniture().cut(&rows);
    while said.first().is_some_and(|row| row.trim().is_empty()) {
        said = &said[1..];
    }
    while said.last().is_some_and(|row| row.trim().is_empty()) {
        said = &said[..said.len() - 1];
    }
    (!said.is_empty()).then(|| said.join("\n"))
}

/// The last paragraph of a cut screen from [`said`]: the rows after the last
/// blank row.
///
/// The answer is at the bottom of the cut screen, with the turn's tool calls
/// above it, so a one-line column would otherwise show an old command. Only
/// the one-line summary uses this; `amx result`, `amx logs` and the card show
/// the whole screen.
fn last_said(said: &str) -> &str {
    let (mut answer, mut under, mut at) = (0, 0, 0);
    for row in said.split('\n') {
        at += row.len() + 1;
        match row.trim().is_empty() {
            // A blank row marks where the next block would start; it becomes
            // the answer only once a non-blank row follows.
            true => under = at,
            false => answer = under,
        }
    }
    &said[answer..]
}

/// Whether the pane is the only place this agent's answers will ever be.
///
/// A vendor with hooks sends its answer in the Stop payload, and one with a
/// transcript leaves it on disk; either way the record has the vendor's own
/// words. An unregistered command has neither measured, so it counts as
/// neither.
fn answers_on_the_pane(vendor: Option<&Vendor>) -> bool {
    vendor
        .is_some_and(|vendor| !vendor.can(Capability::Hooks) && !vendor.can(Capability::Transcript))
}

/// The record's vendor, or `None` for an unregistered command or none at all.
fn vendor_of(meta: &Meta) -> Option<&'static Vendor> {
    crate::registry::entry(meta.agent.as_deref().unwrap_or_default())
}

/// Whether the vendor reports its turns and questions through hooks.
///
/// If so, a settled record survives a screen no rule claims: the vendor would
/// have reported the end of the turn or the question itself. pi's prompt under
/// its update notice is such a screen.
fn reports(vendor: Option<&Vendor>) -> bool {
    vendor.is_some_and(|vendor| vendor.can(Capability::Hooks))
}

/// Whether a reading of this pane is the only account of the agent there is.
///
/// True for a vendor without hooks: nothing else ever moves its record off
/// the phase the spawn wrote, so the reading is written as the record. A
/// vendor with hooks keeps its own record, and an unregistered command is
/// neither.
fn reads_its_own_record(vendor: Option<&Vendor>) -> bool {
    vendor.is_some_and(|vendor| !vendor.can(Capability::Hooks))
}

/// Whether the record is a command rather than an agent.
///
/// A command spawn writes no vendor (see [`crate::verbs::new`]), and nothing
/// moves a command's phase off `starting` until it exits. A record with no
/// vendor at another phase is an agent written by an older amx.
fn runs_a_command(meta: &Meta, state: &State) -> bool {
    meta.agent.is_none() && state.state == Phase::Starting
}

/// Whether this reading replaces the record's phase: a vendor with no record
/// of its own (see [`reads_its_own_record`]) and a screen a rule claimed with
/// permission to decide ([`Evidence::Screen`]). Checked only by
/// [`write_the_reading`].
fn is_the_record(meta: &Meta, reading: &Reading) -> bool {
    reads_its_own_record(vendor_of(meta)) && reading.verdict.evidence == Evidence::Screen
}

/// The finished screen to record as the turn's answer, on a vendor whose pane
/// is the only place its answers are (see [`answers_on_the_pane`]). Whether
/// the turn actually ended is checked under the lock in [`write_the_reading`].
fn worth_writing_down<'a>(meta: &Meta, reading: &'a Reading) -> Option<&'a str> {
    answers_on_the_pane(vendor_of(meta))
        .then_some(reading.said.as_deref())
        .flatten()
}

/// The spinner line without its leading glyph. The glyph cycles through
/// `✻ ✽ ✢ ✶ · *` and may gain more, so any one non-alphanumeric character
/// before the first space is dropped.
fn unglyphed(row: &str) -> String {
    let row = row.trim();
    match row.split_once(' ') {
        Some((glyph, rest))
            if glyph.chars().count() == 1 && !glyph.chars().all(char::is_alphanumeric) =>
        {
            rest.trim_start().to_string()
        }
        _ => row.to_string(),
    }
}

/// Whether the pane should be read for a question the record lacks.
///
/// `PermissionRequest` fires when the box goes up but may name no tool, and
/// the describing notification lands about six seconds later. That is within
/// [`FRESH`], so waiting for the record to go stale would hand back a waiting
/// agent with nothing to answer.
fn wants_the_question(screens: &Ruleset, state: &State) -> bool {
    state.state == Phase::Waiting
        && state
            .question
            .as_deref()
            .is_none_or(|question| placeholder(screens, question))
}

/// Whether reading this agent needs a capture of its pane.
///
/// [`read`] asks the same questions off the record alone, so a wall can batch
/// every wanted capture into one tmux call (see [`Server::captures`]). The two
/// must agree; a test checks it.
///
/// A command's pane is always wanted: nothing else reports on it.
fn wants_the_screen(
    screens: &Ruleset,
    state: &State,
    command: bool,
    alive: bool,
    now: u64,
    heartbeat: Option<u64>,
) -> bool {
    if state.state.is_terminal() || !alive {
        return false;
    }
    if command {
        return true;
    }
    if !cut_short(state) && now.saturating_sub(heard(state, heartbeat)) <= FRESH {
        return wants_the_question(screens, state) || wants_the_doing(screens, state);
    }
    true
}

/// Whether a running turn is worth reading off the spinner line.
///
/// Before a turn's first tool call the record says nothing about what is
/// running, and the spinner line does. Read even when a tool is named, since
/// the card shows the vendor's own words for the whole turn. Only on a vendor
/// whose document names a spinner line.
fn wants_the_doing(screens: &Ruleset, state: &State) -> bool {
    state.state == Phase::Working && screens.furniture().spins()
}

/// When anything was last heard from the agent: the latest of `last_event`,
/// `since`, and the vendor's heartbeat.
///
/// A record written before its first event has only `since`. The heartbeat
/// says a turn is still running during a long tool call, the same as a hook
/// would; see [`Agent::heartbeat`].
fn heard(state: &State, heartbeat: Option<u64>) -> u64 {
    state
        .last_event
        .max(state.since)
        .max(heartbeat.unwrap_or_default())
}

/// Whether `amx interrupt` ended this turn and the vendor has not spoken
/// since.
///
/// claude sends no hook on an interrupt, so the record still says working.
/// The stamp (see [`crate::verbs::interrupt`]) is amx's own word that the turn
/// ended, so neither the [`FRESH`] window nor [`SETTLED_LOOKS`] of stillness
/// applies. The vendor's next event clears it (see [`crate::hook::apply`]).
/// No timestamps are compared: both are whole seconds and would tie.
fn cut_short(state: &State) -> bool {
    state.interrupted_at > 0
}

/// The seconds shown beside an agent, which answer a different question per
/// phase.
///
/// - Ended: how long it worked, per the spans the record added up. This never
///   moves again. A span left open (the pane went and no exit was recorded) is
///   closed at the end stamp or, lacking one, the last thing heard. A record
///   with no spans falls back to how long the run was alive.
/// - Waiting: how long since it stopped. The vendor's notification lands after
///   the box, so the last hook would undercount. A wait read off a screen
///   while the record says working falls back to time since last heard.
/// - Anything else: how long since it was last heard from.
fn clock(phase: Phase, state: &State, created: u64, now: u64, heartbeat: Option<u64>) -> u64 {
    if phase.is_terminal() {
        return worked(phase, state, created, now, heartbeat);
    }
    if phase == Phase::Waiting && state.state == Phase::Waiting && state.since > 0 {
        return now.saturating_sub(state.since);
    }
    now.saturating_sub(heard(state, heartbeat))
}

/// The seconds of work shown beside an agent: the added-up spans plus the one
/// open while working, so it holds still while waiting or idle.
///
/// Once ended, the same frozen value as [`clock`], including its fallback,
/// which [`off_the_log`] may refine from the event log.
fn worked(phase: Phase, state: &State, created: u64, now: u64, heartbeat: Option<u64>) -> u64 {
    if phase.is_terminal() {
        let ended = ended_at(state, heartbeat);
        return match state.worked_by(ended) {
            0 => ended.saturating_sub(created),
            worked => worked,
        };
    }
    state.worked_by(now)
}

/// When a run ended: the stamp the ending wrote, or else the last thing heard.
fn ended_at(state: &State, heartbeat: Option<u64>) -> u64 {
    match state.ended {
        0 => heard(state, heartbeat),
        at => at,
    }
}

/// The seconds of work in an agent's event log, per its vendor's turn edges;
/// see [`crate::store::worked_in`]. `None` for a command or a log with no
/// turn edges.
pub fn worked_off_the_log(agent: &Agent, meta: &Meta) -> Option<u64> {
    let hooks = crate::vendor::hooks_for(meta.agent.as_deref()?)?;
    let events = agent.events().ok()?;
    crate::store::worked_in(&events, |kind| match hooks.moment(kind) {
        Some(Moment::Prompted) => Some(Edge::Opens),
        Some(Moment::Ended) => Some(Edge::Closes),
        _ => None,
    })
}

/// How many logs [`logged_work`] remembers before it starts over.
const LOGGED: usize = 1024;

/// [`worked_off_the_log`], parsed again only when the log's size or mtime has
/// changed since the last call in this process.
///
/// An ended record with no spans would otherwise have its whole log parsed on
/// every refresh of a view. Emptied once it holds [`LOGGED`] logs, so records
/// removed while a view is open cannot grow it for good.
fn logged_work(agent: &Agent, meta: &Meta) -> Option<u64> {
    type Logs = std::collections::HashMap<PathBuf, (u64, SystemTime, Option<u64>)>;
    static LOGS: std::sync::LazyLock<Mutex<Logs>> = std::sync::LazyLock::new(Mutex::default);

    let path = agent.events_path();
    // Taken before the log is read, so an append in between reads as a change
    // next time rather than being cached under the newer stamp.
    let Some((len, modified)) = crate::paths::stamped(&path) else {
        return worked_off_the_log(agent, meta);
    };
    let mut logs = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&(was, then, worked)) = logs.get(&path)
        && (was, then) == (len, modified)
    {
        return worked;
    }
    let worked = worked_off_the_log(agent, meta);
    if logs.len() >= LOGGED {
        logs.clear();
    }
    logs.insert(path, (len, modified, worked));
    worked
}

/// Replaces an ended run's clock with the event log's count, where the record
/// has no spans and [`worked`] fell back to the whole run.
///
/// A record whose spans were lost can still have every turn edge in its log,
/// and a minute's turn in a day's run is not a day's work. The log is read only
/// when the fallback applied, and then only when it changed; see
/// [`logged_work`].
fn off_the_log(agent: &Agent, meta: &Meta, state: &State, verdict: &mut Verdict) {
    if !verdict.phase.is_terminal() || state.worked_by(ended_at(state, agent.heartbeat())) > 0 {
        return;
    }
    if let Some(worked) = logged_work(agent, meta) {
        verdict.worked = worked;
        verdict.age = worked;
    }
}

/// The reading's seconds in the shortest unit: `42s`, `3m`, `5h`, `2d`. Every
/// surface prints through this so they agree.
pub fn in_words(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Works out what an agent is doing.
///
/// - `alive`: whether the pane still answers for this agent.
/// - `capture`: called only if the screen is needed, so a fresh record costs
///   no tmux call.
/// - `created`: when the agent started, for an ended run with no spans.
/// - `held`: seconds the screen has held still; see [`held_still`].
/// - `reports`: whether the vendor reports through hooks; see [`reports`].
/// - `heartbeat`: when the vendor last beat; see [`heard`].
#[allow(clippy::too_many_arguments)]
pub fn read(
    state: &State,
    created: u64,
    alive: bool,
    capture: impl FnOnce() -> Option<String>,
    rules: &Ruleset,
    reports: bool,
    now: u64,
    held: u64,
    heartbeat: Option<u64>,
) -> Reading {
    // Staleness decides whether the record is believed over the pane. The
    // seconds shown beside the agent are `clock`'s question.
    let quiet = now.saturating_sub(heard(state, heartbeat));
    let told = |phase, evidence, rule: Option<&str>| Reading {
        verdict: Verdict {
            phase,
            evidence,
            rule: rule.map(str::to_string),
            age: clock(phase, state, created, now, heartbeat),
            worked: worked(phase, state, created, now, heartbeat),
        },
        asking: None,
        doing: None,
        said: None,
        settled: false,
    };

    if state.state.is_terminal() {
        return told(state.state, Evidence::Record, None);
    }

    if !alive {
        // amx closed an idle, unwatched pane; the record stands until the
        // next enter, attach or resume gives it a pane.
        if state.parked_at > 0 {
            return told(state.state, Evidence::LetGo, None);
        }
        // The pane went without recording an exit: killed, or its server died.
        return told(Phase::Stopped, Evidence::Gone, None);
    }

    // One capture, taken if anything below needs it: always once the
    // hooks are stale, and within the window only for a missing question
    // or a spinner line.
    let fresh = quiet <= FRESH && !cut_short(state);
    let wanted = !fresh || wants_the_question(rules, state) || wants_the_doing(rules, state);
    let seen = wanted.then(capture).flatten();

    // Whether the turn was cut short by esc in the pane. The vendor sends
    // nothing for that, so the hooks are the older account here, and
    // without this the row stayed `working` through the fresh window and
    // then a full quiescent wait.
    let by_hand = seen
        .as_deref()
        .is_some_and(|screen| cut_by_hand_in_the_pane(rules, screen));

    if fresh && !by_hand {
        // The hooks decide the phase. The pane is read only for what the
        // record cannot carry: a question it could not name, or the spinner
        // line of a turn with no tool call yet.
        let mut reading = told(state.state, Evidence::Hooks, None);
        if wants_the_question(rules, state) {
            reading.asking = seen.as_deref().and_then(|screen| rules.asking(screen));
        } else if wants_the_doing(rules, state) {
            reading.doing = seen.as_deref().and_then(|screen| doing(rules, screen));
        }
        return reading;
    }

    let Some(screen) = seen else {
        return told(Phase::Unknown, Evidence::Unknown, None);
    };

    // A turn cut short, by `amx interrupt` or by esc in the pane, has already
    // ended, so a quiescent rule need not wait for stillness.
    let held = match cut_short(state) || by_hand {
        true => SETTLED_LOOKS,
        false => held,
    };

    match rules.claim(&screen, state.state, held) {
        // A prompt with shells still running behind it is not a finished
        // turn; the count the Stop hook wrote tells the two apart (see
        // `State::background`). Nothing is sent when somebody stops a shell in
        // the pane, so the count goes stale, but claude's footer says how many
        // shells it still has. A vendor that never says is taken at the count.
        Claim::Ruled(rule)
            if rule.state == Phase::Idle
                && state.background > 0
                && still_has_a_shell(rules, &screen) =>
        {
            told(Phase::Working, Evidence::Hooks, Some(&rule.name))
        }
        Claim::Ruled(rule) => Reading {
            verdict: Verdict {
                phase: rule.state,
                evidence: Evidence::Screen,
                rule: Some(rule.name.clone()),
                age: clock(rule.state, state, created, now, heartbeat),
                worked: worked(rule.state, state, created, now, heartbeat),
            },
            asking: rule.question(&screen),
            // The spinner line is fresher than anything on the record.
            doing: (rule.state == Phase::Working)
                .then(|| doing(rules, &screen))
                .flatten(),
            // A finished turn: the agent at its prompt with its answer above.
            said: (rule.state == Phase::Idle)
                .then(|| said(rules, &screen))
                .flatten(),
            settled: rule.state == Phase::Idle && rule.quiescent && !shows_a_shell(rules, &screen),
        },
        // The rule may not end a running turn yet, so the record stands.
        Claim::Unsettled(rule) => told(state.state, Evidence::Hooks, Some(&rule.name)),
        // A reporting vendor's settled record stands on a screen nobody
        // claims, since the vendor would have reported the turn ending or its
        // question. A record mid-turn does not: that screen may be a question
        // amx missed.
        Claim::Unclaimed if reports && matches!(state.state, Phase::Idle | Phase::Waiting) => {
            told(state.state, Evidence::Hooks, None)
        }
        Claim::Unclaimed => told(Phase::Unknown, Evidence::Unknown, None),
    }
}

/// Reads one record: an agent through [`read`], a running command through
/// [`read_a_command`]. An ended or paneless command goes through [`read`],
/// whose first two checks do not depend on a vendor.
#[allow(clippy::too_many_arguments)]
fn conclude(
    meta: &Meta,
    state: &State,
    alive: bool,
    capture: impl FnOnce() -> Option<String>,
    rules: &Ruleset,
    now: u64,
    held: u64,
    heartbeat: Option<u64>,
) -> Reading {
    if alive && runs_a_command(meta, state) {
        return read_a_command(state, meta.created, capture().as_deref(), now, heartbeat);
    }
    read(
        state,
        meta.created,
        alive,
        capture,
        rules,
        reports(vendor_of(meta)),
        now,
        held,
        heartbeat,
    )
}

/// Reads a running command: working for as long as its pane is there.
///
/// No rules or hooks apply, so the summary is the last line it printed, read
/// and never recorded. Its work is its whole life, `now - created`: the phase
/// never leaves `starting`, so no span is ever opened.
fn read_a_command(
    state: &State,
    created: u64,
    screen: Option<&str>,
    now: u64,
    heartbeat: Option<u64>,
) -> Reading {
    Reading {
        verdict: Verdict {
            phase: Phase::Working,
            evidence: Evidence::Screen,
            rule: None,
            age: clock(Phase::Working, state, created, now, heartbeat),
            worked: now.saturating_sub(created),
        },
        asking: None,
        doing: screen.and_then(last_printed).map(str::to_string),
        said: None,
        settled: false,
    }
}

/// Seconds the screen this look found has been on the pane. Records the
/// screen and when it appeared, for the next look in whatever process.
///
/// Most readers look once and exit, so stillness cannot be counted in memory.
/// The chrome is cut off first, since a ticking statusline or spinner is not
/// the screen a quiescent rule waits on, and the rest is hashed so the record
/// holds one number. A different screen restarts the clock.
fn held_still(
    agent: &Agent,
    state: &mut State,
    screen: Option<&str>,
    rules: &Ruleset,
    now: u64,
) -> u64 {
    let Some(screen) = screen else { return 0 };
    let rows: Vec<&str> = screen.lines().collect();
    let seen = hashed(rules.furniture().cut(&rows));

    let still = match state.still {
        Some(still) if still.screen == seen => still,
        _ => Still {
            screen: seen,
            since: now,
        },
    };
    if state.still != Some(still) {
        write_the_screen(agent, state, still);
    }
    now.saturating_sub(still.since)
}

/// Records the screen and when it went up, without moving the record's
/// freshness: looking is not hearing from the agent. Only called when the
/// screen changed, so an unchanged pane takes no lock.
fn write_the_screen(agent: &Agent, state: &mut State, still: Still) {
    let heard = state.last_event;
    let noted = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            // A hook that landed meanwhile is newer than this picture.
            if current.last_event == heard {
                current.still = Some(still);
            }
        })
    });

    match noted {
        Ok(current) => *state = current,
        // Unwritable: keep it for this reading; the next look retries.
        Err(_) => state.still = Some(still),
    }
}

/// A hash of the rows, only ever compared with hashes from the same build. A
/// new build may hash differently, which restarts the clock once.
fn hashed(rows: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut hasher);
    hasher.finish()
}

/// Records the question a screen is asking.
///
/// The one thing a reader records rather than forgets: somebody has to answer
/// it, and the pane is the only place its choices are written. How far the
/// screen may change the record's question is [`replaces_the_question`]'s
/// call. A placeholder is dropped first, since it holds the field the screen
/// was going to fill. The writer's lock is taken only when there is something
/// new to write.
fn note(agent: &Agent, screens: &Ruleset, state: &mut State, reading: &Reading) {
    let asking = reading.asking.as_ref();

    // Drop a placeholder first: a record without a question can be filled.
    forget_the_placeholder(screens, state);
    let corrects = replaces_the_question(state, reading);
    let worth = match corrects {
        true => state.corrected_by(asking),
        false => asking.is_some_and(|asking| state.learns_from(asking)),
    };
    if !worth {
        return;
    }

    let heard = state.last_event;
    let noted = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            // A hook that landed meanwhile is newer than this picture.
            if current.last_event == heard {
                forget_the_placeholder(screens, current);
                // Decided again against the record under the lock; the answer
                // above only said the lock was worth taking.
                take_the_question(current, asking, replaces_the_question(current, reading));
            }
        })
    });

    match noted {
        Ok(current) => *state = current,
        // Unwritable: still report the question; the next look retries.
        Err(_) => take_the_question(state, asking, corrects),
    }
}

/// Whether the screen replaces the record's question rather than filling in
/// what it lacks.
///
/// Decided by where the question came from. A hook-reported question (see
/// [`State::reported`]) is the vendor's own words, so the screen only fills
/// gaps. One an earlier look read is replaced whole, since a pane holds one
/// screen at a time. Deciding by vendor fails: pi reports through an
/// extension but draws `/login`, `/trust`, `/model` and its startup gate
/// itself, and those questions reach the record only from the screen.
///
/// Only a screen a rule claimed with permission to decide
/// ([`Evidence::Screen`]) may replace anything.
fn replaces_the_question(state: &State, reading: &Reading) -> bool {
    !state.reported && reading.verdict.evidence == Evidence::Screen
}

/// Puts a screen's question on the state. Against a reported question it
/// fills in the options (no hook carries them) and the text if missing.
/// Against a screen-read one it replaces question and choices together, and
/// clears them when the screen asks nothing.
fn take_the_question(state: &mut State, asking: Option<&Question>, corrects: bool) {
    match (corrects, asking) {
        (true, asking) => state.correct(asking),
        (false, Some(asking)) => state.learn(asking),
        (false, None) => {}
    }
}

/// Records a finished turn's screen as its answer, on a vendor where nothing
/// else ever will. Called only from [`write_the_reading`], which saw the turn
/// end.
fn keep_the_answer(state: &mut State, said: &str) {
    state.result = Some(said.to_string());
    state.source = Some(Source::Screen);
}

/// Clears the summary line as a turn ends.
///
/// The line described the turn in progress (the last tool, or a
/// `summary_command` rewrite) and would otherwise stand in front of the
/// answer; a leftover rewrite would also stop the finished turn being
/// summarised. The vendor's end-of-turn hook does the same elsewhere.
fn the_line_goes_with_the_turn(state: &mut State) {
    state.summary = None;
}

/// Writes a reading's phase as the record, on a vendor with no hooks (see
/// [`reads_its_own_record`]).
///
/// Under one lock: the phase, the turn edge it crosses ([`READ_PROMPT`] or
/// [`READ_TURN_END`]) appended to the log, and, at a turn's end on a vendor
/// whose answers are only on its pane, the screen as the answer. A second
/// reader reaching the same edge finds the phase moved and writes nothing.
/// Rules only name working, waiting and idle, so nothing here ends a run.
///
/// No stamps move: not `last_event`, since looking is not hearing, and not
/// `since`, which would make the record look freshly spoken to. The span of
/// work is left alone for the same reason.
///
/// The answer is recorded only on the edge. A pane does not say which turn
/// drew it, so a finished screen found later may belong to an earlier turn:
/// on pi, a turn with no prose leaves the previous turn's tail on screen.
fn write_the_reading(agent: &Agent, state: &mut State, verdict: &Verdict, said: Option<&str>) {
    if state.state == verdict.phase {
        return;
    }

    let heard = state.last_event;
    let noted = agent.writer().and_then(|writer| {
        let mut crossed = None;
        let current = writer.observe(|current| {
            // A hook that landed meanwhile wins, and a phase another reader
            // just wrote is an edge already crossed.
            if current.last_event != heard || current.state == verdict.phase {
                return;
            }
            crossed = boundary(current.state, verdict.phase);
            current.state = verdict.phase;
            if crossed == Some(READ_TURN_END) {
                the_line_goes_with_the_turn(current);
                if let Some(said) = said {
                    keep_the_answer(current, said);
                }
            }
        })?;
        if let Some(kind) = crossed {
            // The claiming rule is the evidence for this reading.
            writer.append(&Event::new(
                kind,
                serde_json::json!({ "rule": verdict.rule }),
            ))?;
        }
        Ok(current)
    });

    match noted {
        Ok(current) => *state = current,
        // Unwritable: the verdict already carries the phase, but the answer
        // has nowhere else to go, so keep it on this reading. The next look
        // reaches the same edge and writes again.
        Err(_) => {
            if boundary(state.state, verdict.phase) == Some(READ_TURN_END) {
                the_line_goes_with_the_turn(state);
                if let Some(said) = said {
                    keep_the_answer(state, said);
                }
            }
        }
    }
}

/// The turn edge a phase change crosses, if any. Never called with equal
/// phases.
///
/// - Arriving at idle ends a turn, from anywhere.
/// - Starting work from anything but waiting begins one. Back to work after a
///   question is the same turn; reporting vendors fire no prompt event there
///   either.
/// - Stopping on a question is no edge.
fn boundary(from: Phase, to: Phase) -> Option<&'static str> {
    match (from, to) {
        (_, Phase::Idle) => Some(READ_TURN_END),
        (Phase::Waiting, Phase::Working) => None,
        (_, Phase::Working) => Some(READ_PROMPT),
        _ => None,
    }
}

/// Records a turn edge a reporting vendor never reported.
///
/// claude fires no hook when a turn ends by hand (esc, `amx interrupt`, a box
/// answered at the keyboard), so the record stays working or waiting and
/// `result`, `wait`, parking and `on_idle` wait forever. Two edges are
/// written from the screen:
///
/// - Working or waiting to idle, off a settled prompt with nothing running
///   (see [`Reading::settled`]). The question and summary are cleared and the
///   turn end is logged.
/// - Waiting to working, off a running turn while a box is on the record:
///   somebody answered it in the pane. No edge is logged.
///
/// Written under the lock, and only if the record is still the one this look
/// read. The span closes at the later of the last hook and the last look that
/// saw work (see [`saw_it_working`]), since a turn cut mid-tool was working
/// until then. The process that moves the phase to idle runs what idle
/// triggers; see [`crate::hook::after_the_write`].
fn hear_what_went_unsaid(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    state: &mut State,
    reading: &Reading,
    config: &crate::config::Config,
) {
    let Some(to) = went_unsaid(meta, state, reading) else {
        return;
    };

    let (heard, phase) = (state.last_event, state.state);
    let written = agent.writer().and_then(|writer| {
        let current = writer.state()?;
        if current.last_event != heard || current.state != phase {
            return Ok(None);
        }
        let last_sign = agent.heartbeat().max(agent.seen());
        let written = writer.update_state_heard(last_sign, |current| {
            current.state = to;
            current.asks(None);
            if to == Phase::Idle {
                the_line_goes_with_the_turn(current);
                current.background = 0;
            }
        })?;
        let ended = (to == Phase::Idle).then(|| {
            Event::new(
                READ_TURN_END,
                serde_json::json!({ "rule": reading.verdict.rule }),
            )
        });
        if let Some(event) = &ended {
            writer.append(event)?;
        }
        Ok(Some((written, ended)))
    });

    // Unwritable: leave it; the next look reaches the same edge.
    let Ok(Some((written, ended))) = written else {
        return;
    };
    *state = written;
    if let Some(event) = ended {
        crate::hook::after_the_write(root, agent, meta, phase, state, &event, config);
    }
}

/// Stamps [`crate::store::SEEN`] when a reporting vendor's pane reads as a
/// running turn, so a turn later cut by hand books its work up to here.
///
/// Not fed to [`heard`]: that would skip the screen for [`FRESH`] seconds and
/// delay the look that sees the prompt.
fn saw_it_working(agent: &Agent, meta: &Meta, reading: &Reading, now: u64) {
    if reports(vendor_of(meta))
        && reading.verdict.evidence == Evidence::Screen
        && reading.verdict.phase == Phase::Working
    {
        // A missed stamp only costs the span its tail.
        let _ = agent.saw_working(now);
    }
}

/// The phase [`hear_what_went_unsaid`] should write, if this reading is one of
/// its two edges.
fn went_unsaid(meta: &Meta, state: &State, reading: &Reading) -> Option<Phase> {
    if !reports(vendor_of(meta)) || reading.verdict.evidence != Evidence::Screen {
        return None;
    }
    match (state.state, reading.verdict.phase) {
        (Phase::Working | Phase::Waiting, Phase::Idle) if reading.settled => Some(Phase::Idle),
        (Phase::Waiting, Phase::Working) => Some(Phase::Working),
        _ => None,
    }
}

/// The view of one agent, with the freshest summary line available.
///
/// For a working agent, freshest first: the vendor's live stream
/// ([`Agent::live`]), the transcript's newest line ([`newest_said`]), then the
/// record's own `Running <tool>`. A `summary_command` rewrite of this turn
/// beats the transcript line but not the stream; see [`a_rewrite_stands`].
/// None of it is written back. A finished turn is answered off the record.
fn seen(agent: &Agent, meta: Meta, mut state: State, reading: Reading) -> View {
    let fresher = (reading.verdict.phase == Phase::Working)
        .then(|| {
            agent
                .live()
                .as_deref()
                .and_then(first_said)
                .map(str::to_string)
                .or_else(|| newest_said(agent, &meta, &state))
        })
        .flatten();
    // An agent's spinner line goes to `view.doing` only: it is chrome (a
    // gerund, a clock, a token count), not a word about the work. A command's
    // `doing` is its last printed line, the only thing known about it, so
    // that becomes the summary.
    let printed = runs_a_command(&meta, &state);
    if let Some(line) = fresher.or_else(|| reading.doing.clone().filter(|_| printed)) {
        state.summary = Some(line);
    }
    let mut verdict = reading.verdict;
    off_the_log(agent, &meta, &state, &mut verdict);
    let mut view = View::new(meta, state, verdict);
    view.doing = reading.doing;
    view
}

/// The newest line of the record's transcript (see
/// [`crate::conversation::latest`]), unless a rewrite of this turn stands.
///
/// The vendor writes the transcript before the hook reaches amx, so this is
/// fresher than the record. Read on every look and never recorded.
fn newest_said(agent: &Agent, meta: &Meta, state: &State) -> Option<String> {
    if a_rewrite_stands(agent, meta, state) {
        return None;
    }
    let (format, tail) = transcript(meta)?;
    crate::conversation::latest(format, &tail)
}

/// Whether the summary on the record is a `summary_command` rewrite of this
/// turn that the transcript has not moved past.
///
/// The rewrite covers the whole turn, so it beats the transcript's last line
/// until the transcript is written after the ask came back. A rewrite of an
/// earlier turn, or an ask still out, does not count.
fn a_rewrite_stands(agent: &Agent, meta: &Meta, state: &State) -> bool {
    state.summary.is_some()
        && asked(agent.dir()).is_some_and(|asked| {
            asked.over
                && asked.turn == state.since
                && written_at(meta).is_some_and(|at| asked.at > at)
        })
}

/// When the record's transcript was last written, in epoch seconds.
fn written_at(meta: &Meta) -> Option<u64> {
    crate::store::modified_at(meta.transcript.as_ref()?)
}

/// The first non-blank line of what an agent is saying.
fn first_said(said: &str) -> Option<&str> {
    said.lines().map(str::trim).find(|line| !line.is_empty())
}

/// The last non-blank line a command printed. An agent's sentence is read
/// from its start ([`first_said`]); a log is read from its end.
fn last_printed(printed: &str) -> Option<&str> {
    printed
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
}

/// Whether a finished turn with an answer still needs a summary line.
///
/// An answer opens with `Done.` or the first of several paragraphs rather
/// than a summary of itself; [`ask_for_a_line`] gets one. Asked once per turn.
fn wants_a_line(state: &State) -> bool {
    (state.state == Phase::Idle || state.state.is_terminal())
        && state.summary.is_none()
        && state
            .result
            .as_deref()
            .is_some_and(|answer| !answer.trim().is_empty())
}

/// Whether a turn is running, which gets its line rewritten from the
/// conversation so far ([`the_turn_so_far`]). The pacing is [`worth_asking`]'s.
fn wants_a_rewrite(state: &State) -> bool {
    state.state == Phase::Working
}

/// The project's `summary_command`, if set. The project comes from the
/// record, and [`crate::config::for_project`] reads each project's file once.
fn summary_command(meta: &Meta) -> Option<&'static str> {
    crate::config::for_project(&crate::spawn::project_dir(meta))
        .summary_command
        .as_deref()
}

/// Runs the configured command and returns the first non-blank line it
/// prints, or `None` on failure or silence.
///
/// Run through `sh` in the agent's directory, with the text on stdin (it is
/// arbitrary, so never argv) and [`crate::hook::ID_ENV`] set. Also sets
/// [`crate::hook::NESTED_ENV`]: the usual command is `claude -p`, and a claude
/// started with an agent id in its environment would run its hooks as that
/// agent.
///
/// Stdin is written from its own thread, since a command that echoes its
/// input would fill its stdout pipe and deadlock a single-threaded writer.
fn ask_for_a_line(command: &str, at: &Path, id: &str, answer: &str) -> Option<String> {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(at)
        .env(crate::hook::ID_ENV, id)
        .env(crate::hook::NESTED_ENV, "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let feeding = child.stdin.take().map(|mut stdin| {
        let answer = answer.to_string();
        std::thread::spawn(move || {
            // A command that stops reading early is fine. Dropping the handle
            // closes stdin.
            let _ = stdin.write_all(answer.as_bytes());
        })
    });
    let said = child.wait_with_output().ok();
    if let Some(feeding) = feeding {
        let _ = feeding.join();
    }

    let said = said?;
    if !said.status.success() {
        return None;
    }
    String::from_utf8_lossy(&said.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Asks for a turn's line and writes it to the record, without moving the
/// record's freshness and only onto the turn it was asked about; see
/// [`About`].
fn write_the_line(
    root: &Path,
    id: &str,
    turn: u64,
    at: &Path,
    command: &str,
    said: &str,
    about: About,
) {
    let line = ask_for_a_line(command, at, id, said);
    let Ok(agent) = Agent::open(root, id) else {
        return;
    };
    // Settle the ask either way, so the next reader does not repeat it.
    settle_the_ask(&agent, turn, crate::store::now());

    let Some(line) = line else {
        return;
    };
    let _ = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            if still_the_turn_asked_about(current, turn, said, about) {
                current.summary = Some(line);
            }
        })
    });
}

/// Whether the record is still on the turn the line was asked about.
///
/// A finished turn is matched by its answer, since two short turns can share
/// a second, and must have no line yet. A running turn is matched by `since`,
/// which moves with every phase change; any line already there is a tool name
/// or an older rewrite, which this replaces.
fn still_the_turn_asked_about(current: &State, turn: u64, said: &str, about: About) -> bool {
    match about {
        About::TheAnswer => current.result.as_deref() == Some(said) && current.summary.is_none(),
        About::TheTurnSoFar => current.since == turn,
    }
}

thread_local! {
    /// Whether this thread stays around for answers that arrive after a read,
    /// as the view does.
    ///
    /// Thread-local because a test binary runs many tests as threads of one
    /// process; one test opening a view must not make the others look like
    /// the view.
    static STAYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks this thread as staying for answers. Called once where the view's
/// loop starts. One-shot verbs never call it, so they never claim a turn they
/// could not hear back about.
pub(crate) fn will_stay_for_the_answer() {
    STAYING.with(|staying| staying.set(true));
}

/// Whether this thread called [`will_stay_for_the_answer`].
fn staying() -> bool {
    STAYING.with(std::cell::Cell::get)
}

/// Whether a summary ask is in flight in this process.
///
/// One at a time: a view opened over many finished agents would otherwise
/// start a model call for every row at once.
static ASKING: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// Takes the in-flight slot if it is free. A turn refused here is not
/// claimed, so a later reading offers it again.
fn may_ask() -> bool {
    let Ok(mut asking) = ASKING.lock() else {
        return false;
    };
    if *asking {
        return false;
    }
    *asking = true;
    true
}

/// Frees the in-flight slot.
fn done_asking() {
    if let Ok(mut asking) = ASKING.lock() {
        *asking = false;
    }
}

/// The file beside the record holding the last [`Asked`].
const ASKED: &str = "summary.asked";

/// The last summary ask about an agent: which turn, when, and whether it came
/// back.
///
/// Kept beside the record because `amx ls --json` in a caller's loop is a new
/// process each time, and each would otherwise start the command again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
struct Asked {
    turn: u64,
    at: u64,
    /// Whether the ask came back. An empty answer counts: asking a model the
    /// same question every few minutes would quietly cost money.
    over: bool,
}

/// Seconds after which an ask that never came back is taken as lost.
///
/// The ask runs on an unjoined thread, so a verb that exits takes it along.
/// Long enough not to double up on a slow command, short enough that the next
/// reader tries again.
const AGAIN: u64 = 300;

/// Seconds a running turn's line stands before it is rewritten. Short enough
/// to follow the turn, long enough that a watched agent costs a few calls an
/// hour rather than one per redraw.
const REWRITE: u64 = 180;

/// Which text the summary command is asked about. A finished answer is
/// summarised once; a running turn is summarised again as it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum About {
    /// The answer a finished turn left.
    TheAnswer,
    /// The running turn, from its transcript.
    TheTurnSoFar,
}

/// Whether to ask about this turn, given the last ask.
///
/// A running turn is paced by the clock alone: `since` changes each time the
/// agent stops on a permission box, so the last ask counts whatever turn it
/// was about.
fn worth_asking(asked: Option<Asked>, turn: u64, now: u64, about: About) -> bool {
    match asked {
        // Answered: rewrite a running turn once `REWRITE` has passed.
        Some(asked) if asked.over && about == About::TheTurnSoFar => {
            now.saturating_sub(asked.at) >= REWRITE
        }
        Some(asked) if asked.turn == turn => match asked.over {
            // Still out: being answered, or lost with the verb that asked.
            false => now.saturating_sub(asked.at) >= AGAIN,
            // Answered, and a finished answer does not change.
            true => false,
        },
        // Never asked, or asked about an earlier turn.
        _ => true,
    }
}

/// Claims a turn so only one amx asks about it.
///
/// Checked without the writer's lock and again under it; that lock is the one
/// thing shared by every amx on the machine.
fn claim_the_turn(agent: &Agent, turn: u64, now: u64, about: About) -> bool {
    if !worth_asking(asked(agent.dir()), turn, now, about) {
        return false;
    }
    let Ok(_writer) = agent.writer() else {
        return false;
    };
    // Again under the lock, so two amx that both saw no claim make one.
    if !worth_asking(asked(agent.dir()), turn, now, about) {
        return false;
    }
    write_asked(
        agent.dir(),
        Asked {
            turn,
            at: now,
            over: false,
        },
    )
}

/// Marks the ask as come back, whatever it returned.
fn settle_the_ask(agent: &Agent, turn: u64, now: u64) {
    let Ok(_writer) = agent.writer() else {
        return;
    };
    write_asked(
        agent.dir(),
        Asked {
            turn,
            at: now,
            over: true,
        },
    );
}

/// The last ask about the agent in `dir`, if any.
fn asked(dir: &Path) -> Option<Asked> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(ASKED)).ok()?).ok()
}

/// Writes the ask atomically ([`crate::store::write_atomic`]), so a torn
/// write never reads back as no claim.
fn write_asked(dir: &Path, asked: Asked) -> bool {
    let Ok(said) = serde_json::to_string(&asked) else {
        return false;
    };
    crate::store::write_atomic(&dir.join(ASKED), said.as_bytes()).is_ok()
}

/// Asks the project's summary command about this record, if one is set and
/// the record is a finished turn ([`wants_a_line`]) or a running one
/// ([`wants_a_rewrite`]).
fn have_a_line_where_one_is_wanted(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    state: &State,
    now: u64,
) {
    // Looking the command up reads the tree's `.git` and canonicalizes a
    // path, on every look at every agent, so the cheap checks go first.
    let (line, rewrite) = (wants_a_line(state), wants_a_rewrite(state));
    if !staying() || !(line || rewrite) {
        return;
    }
    let Some(command) = summary_command(meta) else {
        return;
    };
    if line {
        have_a_line_written(root, agent, meta, state, command, now);
    } else {
        have_a_line_rewritten(root, agent, meta, state, command, now);
    }
}

/// Asks for a finished turn's line, from its answer.
fn have_a_line_written(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    state: &State,
    command: &str,
    now: u64,
) {
    ask_about_the_turn(root, agent, meta, state, command, now, About::TheAnswer);
}

/// Asks for a running turn's line, from the conversation so far. Paced by
/// [`REWRITE`].
fn have_a_line_rewritten(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    state: &State,
    command: &str,
    now: u64,
) {
    ask_about_the_turn(root, agent, meta, state, command, now, About::TheTurnSoFar);
}

/// Starts the summary command on its own thread, for a reader that stays.
///
/// The thread is never joined; the view picks the line up on a later reading.
/// A verb that exits never gets here (see [`staying`]), so it never leaves a
/// claim unsettled. A command that never returns holds its thread and the
/// in-flight slot.
fn ask_about_the_turn(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    state: &State,
    command: &str,
    now: u64,
    about: About,
) {
    // Only a reader that stays can hear the answer, or should pay for it.
    if !staying() {
        return;
    }
    // Checked before taking the slot, so a turn that will never be asked
    // about does not hold up every row after it.
    if !worth_asking(asked(agent.dir()), state.since, now, about) {
        return;
    }
    // Also before reading the transcript, which a turn not worth asking about
    // does not need.
    let said = match about {
        About::TheAnswer => state.result.clone(),
        About::TheTurnSoFar => the_turn_so_far(meta),
    };
    let Some(said) = said else {
        return;
    };
    if !may_ask() {
        return;
    }
    if !claim_the_turn(agent, state.since, now, about) {
        done_asking();
        return;
    }

    let (root, id, turn) = (root.to_path_buf(), meta.id.clone(), state.since);
    let at = where_it_ran(meta);
    let command = command.to_string();
    let asking = std::thread::Builder::new()
        .name("amx-summary".to_string())
        .spawn(move || {
            write_the_line(&root, &id, turn, &at, &command, &said, about);
            done_asking();
        });
    if asking.is_err() {
        done_asking();
    }
}

/// The running turn as the summary command is given it: the transcript tail
/// in the plain shape `amx logs` prints (see [`crate::conversation::plain`]).
/// `None` when there is nothing readable to ask about.
fn the_turn_so_far(meta: &Meta) -> Option<String> {
    let (format, tail) = transcript(meta)?;
    let said = crate::conversation::plain(&crate::conversation::read(format, &tail));
    (!said.trim().is_empty()).then_some(said)
}

/// Where the command runs: the agent's worktree while it exists, else the
/// directory the run was started in.
fn where_it_ran(meta: &Meta) -> std::path::PathBuf {
    match &meta.worktree {
        Some(tree) if tree.is_dir() => tree.clone(),
        _ => meta.dir.clone(),
    }
}

/// Reads one agent.
pub fn view(root: &Path, id: &str, now: u64) -> Result<View> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let state = agent.state()?;
    let server = Server::from_socket(meta.socket.clone());
    let rules = own_screens(&meta);

    let alive = match state.state.is_terminal() {
        true => true,
        false => match server.answers_for_now(&meta.pane, &meta.id) {
            Ok(answers) => answers,
            Err(_) => {
                let verdict = as_written(&agent, &state, meta.created, now);
                return Ok(View::new(meta, state, verdict));
            }
        },
    };
    // Read once, so the capture decision and the reading see the same beat.
    let beat = agent.heartbeat();
    // Captured here rather than in the closure, so `held_still` can compare
    // it with the recorded screen first.
    let screen = wants_the_screen(
        rules,
        &state,
        runs_a_command(&meta, &state),
        alive,
        now,
        beat,
    )
    .then(|| server.capture(&meta.pane).ok())
    .flatten();
    Ok(look(
        root,
        Record { agent, meta, state },
        alive,
        screen,
        now,
        beat,
    ))
}

/// Conclude about one agent off a screen already taken, write down what the
/// reading leaves on the record, and hand back the view.
fn look(
    root: &Path,
    record: Record,
    alive: bool,
    screen: Option<String>,
    now: u64,
    beat: Option<u64>,
) -> View {
    let Record {
        agent,
        meta,
        mut state,
    } = record;
    let rules = own_screens(&meta);
    // Only a quiescent rule reads `held`, and no rule reads a command's pane.
    let held = match runs_a_command(&meta, &state) {
        true => 0,
        false => held_still(&agent, &mut state, screen.as_deref(), rules, now),
    };
    let reading = conclude(&meta, &state, alive, || screen, rules, now, held, beat);
    note(&agent, rules, &mut state, &reading);
    if is_the_record(&meta, &reading) {
        let said = worth_writing_down(&meta, &reading);
        write_the_reading(&agent, &mut state, &reading.verdict, said);
    }
    saw_it_working(&agent, &meta, &reading, now);
    let config = crate::config::current();
    hear_what_went_unsaid(root, &agent, &meta, &mut state, &reading, config);
    have_a_line_where_one_is_wanted(root, &agent, &meta, &state, now);

    seen(&agent, meta, state, reading)
}

/// One agent's record read off disk, parsed once for both the sweep and the
/// reading.
pub struct Record {
    pub agent: Agent,
    pub meta: Meta,
    pub state: State,
}

/// Every agent's record, in directory order. A record whose meta or state
/// cannot be read is skipped rather than failing the whole listing.
pub fn records(root: &Path) -> Result<Vec<Record>> {
    Ok(records_of(root, crate::store::list(root)?))
}

/// The records of `ids`, skipping any that cannot be read.
fn records_of(root: &Path, ids: Vec<String>) -> Vec<Record> {
    let mut records = Vec::new();
    for id in ids {
        // Gone since the listing, when another amx removed it.
        let Ok(agent) = Agent::open(root, &id) else {
            continue;
        };
        let Ok(meta) = agent.meta() else { continue };
        let Ok(state) = agent.state() else { continue };
        records.push(Record { agent, meta, state });
    }
    records
}

/// A record plus what a wall reading knows before capturing: whether its pane
/// still answers for it, and its heartbeat.
struct Pending {
    record: Record,
    alive: bool,
    /// Read once, so the capture decision and the reading agree; see [`view`].
    beat: Option<u64>,
}

/// Reads every agent, oldest first.
pub fn views(root: &Path, now: u64) -> Result<Vec<View>> {
    Ok(views_of(root, records(root)?, now))
}

/// [`views`] over records already read.
///
/// Two passes with one round of tmux between them: one pane listing per
/// server, then one capture call per server for every screen needed. Each
/// record is read against its own vendor's screens.
pub fn views_of(root: &Path, records: Vec<Record>, now: u64) -> Vec<View> {
    let mut pending: Vec<Pending> = Vec::new();
    // `None` for a server whose tmux could not be asked.
    let mut owners: Vec<(crate::tmux::Socket, Option<crate::tmux::PaneOwners>)> = Vec::new();
    let mut views = Vec::new();

    for record in records {
        let meta = &record.meta;
        let alive = if record.state.state.is_terminal() {
            true
        } else {
            let listed = match owners.iter().find(|(socket, _)| socket == &meta.socket) {
                Some((_, listed)) => listed,
                None => {
                    let listed = Server::from_socket(meta.socket.clone())
                        .owners_for_now()
                        .ok();
                    owners.push((meta.socket.clone(), listed));
                    &owners.last().expect("just pushed").1
                }
            };
            let Some(listed) = listed else {
                let Record { agent, meta, state } = record;
                let verdict = as_written(&agent, &state, meta.created, now);
                views.push(View::new(meta, state, verdict));
                continue;
            };
            listed.pane_answers_for(&meta.pane, &meta.id)
        };

        let beat = record.agent.heartbeat();
        pending.push(Pending {
            record,
            alive,
            beat,
        });
    }

    let mut screens = screens_of(&pending, now);
    for (at, item) in pending.into_iter().enumerate() {
        let Pending {
            record,
            alive,
            beat,
        } = item;
        // Taken, since only one reading uses it.
        let screen = screens[at].take();
        views.push(look(root, record, alive, screen, now, beat));
    }

    views.sort_by_key(|view| (view.meta.created, view.meta.id.clone()));
    views
}

/// The screens the readings need, captured a server at a time and indexed
/// like `pending`. Unwanted screens and panes gone since the listing are
/// `None`.
fn screens_of(pending: &[Pending], now: u64) -> Vec<Option<String>> {
    let mut screens: Vec<Option<String>> = vec![None; pending.len()];
    let mut wanted: Vec<(crate::tmux::Socket, Vec<usize>)> = Vec::new();

    for (at, item) in pending.iter().enumerate() {
        let (meta, state) = (&item.record.meta, &item.record.state);
        if !wants_the_screen(
            own_screens(meta),
            state,
            runs_a_command(meta, state),
            item.alive,
            now,
            item.beat,
        ) {
            continue;
        }
        match wanted
            .iter_mut()
            .find(|(socket, _)| socket == &item.record.meta.socket)
        {
            Some((_, asking)) => asking.push(at),
            None => wanted.push((item.record.meta.socket.clone(), vec![at])),
        }
    }

    for (socket, asking) in wanted {
        let panes: Vec<crate::tmux::PaneId> = asking
            .iter()
            .map(|at| pending[*at].record.meta.pane.clone())
            .collect();
        let taken = Server::from_socket(socket).captures(&panes);
        for (at, screen) in asking.into_iter().zip(taken) {
            screens[at] = screen;
        }
    }
    screens
}

/// Every agent from its record alone, oldest first, without asking tmux.
///
/// The first frame the view draws, before a reading has captured anything. A
/// gone pane still reads as its last phase, and an unnamed question stays
/// unnamed, until a reading corrects them.
pub fn recorded(root: &Path, now: u64) -> Result<Vec<View>> {
    let mut views: Vec<View> = records(root)?
        .into_iter()
        .map(|record| {
            let mut verdict = from_the_record(
                &record.state,
                record.meta.created,
                now,
                record.agent.heartbeat(),
            );
            off_the_log(&record.agent, &record.meta, &record.state, &mut verdict);
            View::new(record.meta, record.state, verdict)
        })
        .collect();

    views.sort_by_key(|view| (view.meta.created, view.meta.id.clone()));
    Ok(views)
}

/// The record as written, for when tmux could not be asked about the pane.
/// Not knowing is no reason to call the pane gone.
fn as_written(agent: &Agent, state: &State, created: u64, now: u64) -> Verdict {
    Verdict {
        evidence: Evidence::Record,
        ..from_the_record(state, created, now, agent.heartbeat())
    }
}

/// The verdict the record alone gives: its phase with the same clock a
/// reading would use, and evidence `Record` for an ended run or `Hooks`
/// otherwise.
fn from_the_record(state: &State, created: u64, now: u64, heartbeat: Option<u64>) -> Verdict {
    let phase = state.state;
    Verdict {
        phase,
        evidence: match phase.is_terminal() {
            true => Evidence::Record,
            false => Evidence::Hooks,
        },
        rule: None,
        age: clock(phase, state, created, now, heartbeat),
        worked: worked(phase, state, created, now, heartbeat),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules;
    use crate::vendor::second::SECOND;
    use tempfile::TempDir;

    const IDLE_SCREEN: &str = "\
✻ Worked for 2m 26s
❯
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The same prompt with claude 2.1.278's count of running shells on its
    /// mode footer: what says a Stop hook's count is still true.
    const A_SCREEN_WITH_A_SHELL: &str = "\
✻ Worked for 2m 26s · 1 shell still running
❯
  ⏵⏵ bypass permissions on · 1 shell
";

    /// The same screen once the shell is stopped: the footer has no tail.
    const A_SCREEN_WITH_NO_SHELL: &str = "\
✻ Worked for 2m 26s
❯
  ⏵⏵ bypass permissions on (shift+tab to cycle)
";

    /// claude 2.1.278 after esc in the pane: its row for an interrupted tool
    /// call with only chrome under it. No hook is sent.
    const AN_INTERRUPTED_SCREEN: &str = "\
● Bash(sleep 120)
  Ran 1 shell command
  ⎿  Interrupted · What should Claude do instead?

────────────────────────────────────────
❯
────────────────────────────────────────
  Opus 5 │ ◈ 2% │ amx-measure (HEAD)
  ⏵⏵ bypass permissions on (shift+tab to cycle)
";

    /// The same row with a later turn running over it, where it stays for the
    /// rest of the session.
    const A_SCREEN_PAST_AN_INTERRUPT: &str = "\
  ⎿  Interrupted · What should Claude do instead?

❯ carry on

● Read(src/main.rs)

✢ Forging… (22s · ↓ 1.3k tokens)

────────────────────────────────────────
❯
────────────────────────────────────────
  Opus 5 │ ◈ 2% │ amx-measure (HEAD)
  ⏵⏵ bypass permissions on (shift+tab to cycle)
";

    const A_SHELL: &str = "$ ls\nCargo.toml  src\n$\n";

    /// A command's pane part way through, with the blank rows a capture of a
    /// taller screen ends on.
    const A_COMMAND: &str = "\
running 2 tests
test reads_the_row ... ok
test reads_the_line ... ok

";

    /// A running turn in claude 2.1.240: output, the spinner line over the
    /// composer, and the mode footer that is on every claude screen.
    const A_WORKING_SCREEN: &str = "\
● Read(src/main.rs)
  ⎿  Read 210 lines

✢ Forging… (22s · ↓ 1.3k tokens)
────────────────────────────────────────
❯
────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The vendor's notification about a dialog, whole. It says a question
    /// exists and nothing about what it asks.
    const A_PLACEHOLDER: &str = "Claude needs your permission";

    /// claude 2.1.240's menu at 220 columns, cut to the rows a floor of 24
    /// reaches. The two rows under the choices are chrome, not part of the
    /// tool call's payload.
    const A_MENU: &str = "\
────────────────────────────────────────────────────────
 ☐ License

Which license should the LICENSE file contain?

❯ 1. MIT
     Short and permissive
  2. Apache-2.0
     Permissive with a patent grant
  3. Type something.
────────────────────────────────────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// pi's prompt after a turn (0.84.4): the agent's rows, the composer box,
    /// and the directory and stats line under every pi screen. Nothing on it
    /// is claude's. Starts on the row itself, since pi indents by a column and
    /// a trailing `\` would eat it.
    const A_PI_PROMPT: &str = " ran the migration

 Took 15.2s

────────────────────────────

────────────────────────────
~/srv/app
↑1.5k ↓69 R1.3k CH90.3% $0.001 (sub) 0.5%/264k (auto)
";

    /// What reading a finished pi turn writes to the record: the transcript
    /// above the box with the chrome cut off. The startup banner, the prompt,
    /// a bash call with its output, a read, and under them the one sentence
    /// asked for.
    ///
    /// pi 0.84.4 at 100 columns; see `docs/pi-screens.md`. Starts on the row
    /// itself, like [`A_PI_PROMPT`].
    const A_PI_TURN: &str = " pi v0.84.4
 escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
 Press ctrl+o to show full startup help and loaded resources.

 Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


 Run: wc -l notes.md using the bash tool. Then read notes.md. Then say in one short sentence what
 the file describes. Use no other tools.



 $ wc -l notes.md

 4 notes.md

 Took 0.0s



 read notes.md


 The file describes recent changes to caching and timeout configuration.";

    /// The same turn at 40 columns, where the answer wraps onto two rows. The
    /// tool rows are unchanged.
    const A_PI_TURN_AT_40: &str = " look up its docs. Ask it how to use or
 extend Pi.


 Run: wc -l notes.md using the bash
 tool. Then read notes.md. Then say in
 one short sentence what the file
 describes. Use no other tools.



 $ wc -l notes.md

 4 notes.md

 Took 0.0s



 read notes.md


 The file describes recent changes to
 caching and timeout configuration.";

    /// A permission box: a screen asking a question.
    const A_BLOCKING_SCREEN: &str = "\
────────────────────────────────
 Bash command
   rm -rf build
 Do you want to proceed?
 ❯ 1. Yes
   2. No
 Esc to cancel · Tab to amend
";

    fn state(phase: Phase, last_event: u64) -> State {
        State {
            state: phase,
            last_event,
            since: last_event,
            ..State::default()
        }
    }

    fn meta() -> Meta {
        Meta {
            role: None,
            parent: None,
            depth: 0,
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: crate::tmux::Socket::Name("amx".to_string()),
            pane: crate::tmux::PaneId::new("%7").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        }
    }

    /// An agent's directory on disk, for readings that look beside the record
    /// for what the agent is saying now.
    fn an_agent(root: &TempDir) -> Agent {
        Agent::create(root.path(), &meta()).expect("a record")
    }

    #[test]
    fn a_view_prints_who_an_agent_is_a_child_of() {
        // `--json` carries the two fields a program nests rows by.
        let mut child = meta();
        child.parent = Some("scout-a1b".to_string());
        child.depth = 2;
        let view = View::new(
            child,
            state(Phase::Working, 1_000),
            verdict(Phase::Working, Evidence::Hooks, None),
        );
        assert_eq!(view.json()["parent"], "scout-a1b");
        assert_eq!(view.json()["depth"], 2);

        // A root prints null and 0, as does a record from an older amx.
        let root = View::new(
            meta(),
            state(Phase::Working, 1_000),
            verdict(Phase::Working, Evidence::Hooks, None),
        );
        assert_eq!(root.json()["parent"], serde_json::Value::Null);
        assert_eq!(root.json()["depth"], 0);
    }

    fn verdict(phase: Phase, evidence: Evidence, rule: Option<&str>) -> Verdict {
        Verdict {
            phase,
            evidence,
            rule: rule.map(str::to_string),
            age: 30,
            worked: 30,
        }
    }

    fn reading(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Reading {
        started(0, state, alive, screen, now)
    }

    /// [`reading`] for an agent started at `created`, which an ended run with
    /// no spans is measured from.
    fn started(
        created: u64,
        state: &State,
        alive: bool,
        screen: Option<&str>,
        now: u64,
    ) -> Reading {
        heard_from(true, created, state, alive, screen, now)
    }

    /// [`reading`] on a vendor that does or does not report through hooks;
    /// see [`reports`].
    fn heard_from(
        reports: bool,
        created: u64,
        state: &State,
        alive: bool,
        screen: Option<&str>,
        now: u64,
    ) -> Reading {
        read(
            state,
            created,
            alive,
            || screen.map(str::to_string),
            rules::of("claude"),
            reports,
            now,
            1,
            None,
        )
    }

    /// [`reading`] over a screen that has held still long enough for a
    /// quiescent rule to end a turn; see [`SETTLED_LOOKS`].
    fn settled(state: &State, screen: &str, now: u64) -> Verdict {
        read(
            state,
            0,
            true,
            || Some(screen.to_string()),
            rules::of("claude"),
            true,
            now,
            SETTLED_LOOKS,
            None,
        )
        .verdict
    }

    fn decided(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Verdict {
        reading(state, alive, screen, now).verdict
    }

    /// [`reading`] with a heartbeat on the record, in epoch seconds; see
    /// [`Agent::heartbeat`]. Against pi's document, since pi's report beats.
    fn beating(state: &State, screen: Option<&str>, now: u64, heartbeat: Option<u64>) -> Reading {
        read(
            state,
            0,
            true,
            || screen.map(str::to_string),
            rules::of("pi"),
            true,
            now,
            1,
            heartbeat,
        )
    }

    #[test]
    fn reader_takes_the_question_off_the_screen_that_answered() {
        let reading = reading(
            &state(Phase::Working, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_100,
        );
        assert_eq!(reading.verdict.phase, Phase::Waiting);

        let asking = reading.asking.expect("a blocking screen is asking");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);
    }

    #[test]
    fn reader_reads_the_pane_the_moment_the_hooks_say_waiting() {
        // The record says waiting without saying what for: the event that put
        // the box up named no tool, and the describing notification is six
        // seconds behind. The hooks are fresh, and the pane is still the only
        // place the question and its choices are.
        let first_look = reading(
            &state(Phase::Waiting, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_000,
        );
        assert_eq!(first_look.verdict.phase, Phase::Waiting);
        assert_eq!(
            first_look.verdict.evidence,
            Evidence::Hooks,
            "the hooks still say what the agent is doing"
        );
        assert_eq!(
            first_look.verdict.rule, None,
            "the screen was read for the question, not for the state"
        );

        let asking = first_look.asking.expect("the pane is asking something");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);

        // A pane that cannot be read leaves the hooks' conclusion as it is.
        let unreadable = reading(&state(Phase::Waiting, 1_000), true, None, 1_000);
        assert_eq!(unreadable.verdict.phase, Phase::Waiting);
        assert_eq!(unreadable.verdict.evidence, Evidence::Hooks);
        assert_eq!(unreadable.asking, None);
    }

    #[test]
    fn reader_reads_the_pane_past_a_question_that_names_nothing() {
        // The vendor's dialog host sends only the dialog's title, so the record
        // holds a sentence saying a question exists. That is no more use than
        // an empty field, and the rest is on the pane.
        let mut placeheld = state(Phase::Waiting, 1_000);
        placeheld.question = Some(A_PLACEHOLDER.to_string());
        let asking = reading(&placeheld, true, Some(A_BLOCKING_SCREEN), 1_000)
            .asking
            .expect("a sentence that names nothing leaves the question unasked");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);

        // The sentence naming the tool is actionable, so a reader holding one
        // does not go to the pane.
        let mut told = state(Phase::Waiting, 1_000);
        told.question = Some(format!("{A_PLACEHOLDER} to use Bash"));
        assert_eq!(
            reading(&told, true, Some(A_BLOCKING_SCREEN), 1_000).asking,
            None,
            "the placeholder is a whole sentence, not the start of one"
        );
    }

    #[test]
    fn reader_replaces_a_question_a_screen_read_and_keeps_one_a_hook_reported() {
        // Deciding by vendor fails: pi reports through an extension, but its
        // login box, trust selector and startup gate fire no event, so those
        // questions were treated as pi's own word and stayed on the record
        // while the pane moved on. The question's own source decides, on
        // reporting vendors as on others.
        let vendor = Meta {
            parent: None,
            depth: 0,
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            ..meta()
        };
        assert!(reports(vendor_of(&vendor)), "a vendor with hooks");

        let looked_at_the_dialog = |question: &str, reported: bool| {
            let root = TempDir::new().unwrap();
            let mut state = state(Phase::Waiting, 1_000);
            state.question = Some(question.to_string());
            state.options = vec!["Skip".to_string()];
            state.reported = reported;
            a_record(root.path(), &vendor, &state);

            let agent = Agent::open(root.path(), &vendor.id).expect("a record");
            let reading = reading(&state, true, Some(A_BLOCKING_SCREEN), 1_100);
            assert_eq!(reading.verdict.evidence, Evidence::Screen, "{question}");
            assert!(
                !is_the_record(&vendor, &reading),
                "and the phase is still the vendor's to write, which this does \
                 not move: {question}"
            );
            note(&agent, rules::of("claude"), &mut state, &reading);
            state
        };

        // A reported question is the vendor's own words: the screen only fills
        // what the hooks left empty.
        let heard = looked_at_the_dialog("Claude needs your permission to use Bash", true);
        assert_eq!(
            heard.question.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        assert_eq!(heard.options, ["Skip"], "which was nothing here");
        assert!(heard.reported, "and it is still the vendor's word");

        // An earlier look at the same pane: the later reading replaces the
        // question and its choices together, so one screen's choices never
        // end up under another screen's question.
        let read = looked_at_the_dialog("Which license should the LICENSE file contain?", false);
        assert_eq!(read.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(read.options, ["Yes", "No"]);
        assert!(!read.reported, "and it is still nobody's but the screen's");
    }

    #[test]
    fn reader_lets_the_menu_on_the_screen_say_what_answers_it() {
        use crate::store::Kind;

        // A record from an older amx: the vendor asks itself permission to
        // use its question tool after the menu is drawn, leaving `permission`
        // on the record, and a caller told to answer a menu with y or n.
        let mut stale = state(Phase::Waiting, 1_000);
        stale.kind = Some(Kind::Permission);
        stale.question = Some("Claude needs your permission to use Ask User Question".to_string());

        let menu = reading(&stale, true, Some(A_MENU), 1_100);
        assert_eq!(menu.verdict.rule.as_deref(), Some("ask_menu"));
        let view = View::new(meta(), stale, menu.verdict);
        assert_eq!(
            view.kind(),
            Some(Kind::Question),
            "the one screen no other prompt can be mistaken for"
        );
        assert_eq!(view.json()["kind"], "question");

        // Only that way round. A record saying a question is asked is the
        // vendor's own account, and a rule naming another screen only fills
        // what the hooks left empty.
        let mut told = state(Phase::Waiting, 1_000);
        told.kind = Some(Kind::Question);
        let box_screen = reading(&told, true, Some(A_BLOCKING_SCREEN), 1_100);
        assert_eq!(
            box_screen.verdict.rule.as_deref(),
            Some("permission_prompt")
        );
        assert_eq!(
            View::new(meta(), told, box_screen.verdict).kind(),
            Some(Kind::Question)
        );

        // A screen no rule claimed says nothing about the kind.
        let mut held = state(Phase::Waiting, 1_000);
        held.kind = Some(Kind::Permission);
        let unclaimed = reading(&held, true, Some(A_SHELL), 1_500);
        assert_eq!(unclaimed.verdict.rule, None);
        assert_eq!(
            View::new(meta(), held, unclaimed.verdict).kind(),
            Some(Kind::Permission)
        );
    }

    #[test]
    fn reader_never_hands_a_caller_the_placeholder() {
        use crate::store::Kind;

        // None of these reaches anybody: a row carrying one reads like an
        // answer while saying no more than an empty row.
        for nothing in [A_PLACEHOLDER, "Claude is waiting for your input", "   "] {
            let held = State {
                state: Phase::Waiting,
                question: Some(nothing.to_string()),
                options: vec!["Yes".to_string()],
                kind: Some(Kind::Permission),
                ..State::default()
            };
            let waiting = View::new(meta(), held, verdict(Phase::Waiting, Evidence::Hooks, None));

            assert_eq!(waiting.line(), None, "{nothing:?} is not a question");
            assert_eq!(waiting.json()["question"], serde_json::Value::Null);
            assert!(
                waiting.state.options.is_empty(),
                "and they are nobody's choices"
            );
            assert_eq!(
                waiting.kind(),
                Some(Kind::Permission),
                "what kind of thing is being asked is still known"
            );
        }
    }

    #[test]
    fn reader_lets_the_pane_answer_what_the_placeholder_was_holding() {
        use crate::store::Kind;

        // The placeholder goes before the record is asked whether the screen
        // adds anything; otherwise it holds the field the screen would fill
        // and every later reader goes back to the pane.
        let mut state = State {
            state: Phase::Waiting,
            question: Some(A_PLACEHOLDER.to_string()),
            kind: Some(Kind::Permission),
            ..State::default()
        };
        let seen = Question {
            marked: None,
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
            walked: false,
        };

        forget_the_placeholder(rules::of("claude"), &mut state);
        assert!(state.learns_from(&seen), "there is something to learn now");
        state.learn(&seen);
        assert_eq!(state.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(state.options, ["Yes", "No"]);
        assert_eq!(
            state.kind,
            Some(Kind::Permission),
            "and it was a permission box all along"
        );
    }

    #[test]
    fn reader_has_no_question_from_a_screen_it_never_looked_at() {
        // A record that already names the question needs no capture while the
        // hooks are fresh.
        let mut told = state(Phase::Waiting, 1_000);
        told.question = Some("Do you want to proceed?".to_string());
        let fresh = reading(&told, true, Some(A_BLOCKING_SCREEN), 1_000);
        assert_eq!(fresh.verdict.evidence, Evidence::Hooks);
        assert_eq!(fresh.asking, None);

        // Nor does a working agent, whatever is on its screen.
        let mid_turn = reading(
            &state(Phase::Working, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_000,
        );
        assert_eq!(mid_turn.verdict.phase, Phase::Working);
        assert_eq!(mid_turn.asking, None);

        // A screen that is not asking anything says nothing about it.
        let quiet = reading(
            &state(Phase::Starting, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(quiet.verdict.phase, Phase::Idle);
        assert_eq!(quiet.asking, None);
    }

    #[test]
    fn reader_takes_what_a_working_agent_is_doing_off_the_spinner_line() {
        // The hooks went quiet mid-turn, so the tool the record names may have
        // finished a minute ago. The vendor's line on the pane is current.
        let mut told = state(Phase::Working, 1_000);
        told.summary = Some("Running Bash".to_string());
        let reading = reading(&told, true, Some(A_WORKING_SCREEN), 1_100);

        assert_eq!(reading.verdict.phase, Phase::Working);
        assert_eq!(reading.verdict.rule.as_deref(), Some("spinner"));
        assert_eq!(
            reading.doing.as_deref(),
            Some("Forging… (22s · ↓ 1.3k tokens)"),
            "the glyph is the vendor's pulse rather than a word about the turn"
        );

        // For the card's rule only. It is chrome (a gerund, a clock the row
        // already shows), so the row keeps what the last tool call wrote.
        let root = TempDir::new().unwrap();
        let view = seen(&an_agent(&root), meta(), told, reading);
        assert_eq!(
            view.doing.as_deref(),
            Some("Forging… (22s · ↓ 1.3k tokens)")
        );
        assert_eq!(view.line(), Some("Running Bash"));
    }

    #[test]
    fn reader_reads_the_spinner_line_while_the_hooks_are_fresh_and_name_no_tool() {
        // A second after the prompt: fresh hooks, working, and no tool called
        // yet, so the record names nothing. The spinner line is read now for
        // the card's rule.
        let told = state(Phase::Working, 1_000);
        let fresh = reading(&told, true, Some(A_WORKING_SCREEN), 1_001);
        assert_eq!(fresh.verdict.phase, Phase::Working);
        assert_eq!(
            fresh.verdict.evidence,
            Evidence::Hooks,
            "the hooks still decide the phase"
        );
        assert_eq!(
            fresh.doing.as_deref(),
            Some("Forging… (22s · ↓ 1.3k tokens)")
        );
        let root = TempDir::new().unwrap();
        let view = seen(&an_agent(&root), meta(), told.clone(), fresh);
        assert_eq!(
            view.doing.as_deref(),
            Some("Forging… (22s · ↓ 1.3k tokens)")
        );
        assert_eq!(
            view.line(),
            None,
            "and a row with nothing to say says nothing rather than saying \
             the gerund the vendor is spinning"
        );

        // Before the vendor draws its line, the row stays as empty as the
        // record.
        let blank = reading(&told, true, Some(IDLE_SCREEN), 1_001);
        assert_eq!(blank.verdict.phase, Phase::Working);
        assert_eq!(blank.doing, None);

        // Both measured vendors spin a row, claude by its fragments and pi by
        // its braille frame, so both are asked.
        assert!(wants_the_screen(
            rules::of("claude"),
            &told,
            false,
            true,
            1_001,
            None
        ));
        assert!(wants_the_screen(
            rules::of("pi"),
            &told,
            false,
            true,
            1_001,
            None
        ));
    }

    #[test]
    fn reader_reads_the_spinner_line_through_whichever_glyph_is_on_it() {
        // The glyph cycles through six shapes and may gain more; all that is
        // relied on is one character that is not a word.
        for glyph in ["✻", "✽", "✢", "✶", "·", "*"] {
            let screen = format!(
                "{glyph} Smooshing… (7s · thinking with xhigh effort)\n────────────────────────────────────────\n❯\n────────────────────────────────────────\n  ⏵⏵ auto mode on\n"
            );
            let reading = reading(&state(Phase::Working, 1_000), true, Some(&screen), 1_100);
            assert_eq!(
                reading.doing.as_deref(),
                Some("Smooshing… (7s · thinking with xhigh effort)"),
                "{glyph}"
            );
        }
    }

    #[test]
    fn reader_finds_the_spinning_line_by_the_vendors_own_fragments() {
        // claude's line always carries two punctuation fragments, read from
        // claude's document. Another vendor's line never has them.
        let second = second_vendors_screens();
        let its_own = " thinking for 12s about the file you named\n = compose =\n";
        assert_eq!(
            doing(&second, its_own).as_deref(),
            Some("thinking for 12s about the file you named")
        );
        assert_eq!(
            doing(rules::of("claude"), its_own),
            None,
            "claude spins nothing that reads like that"
        );
        assert_eq!(
            doing(&second, A_WORKING_SCREEN),
            None,
            "and its fragments are on no screen claude draws"
        );
    }

    #[test]
    fn reader_reads_pis_spinner_out_of_the_border_and_its_thinking_row_above_it() {
        // pi 0.85.1 draws its status in the composer's top border, and with
        // `hideThinkingBlock` it draws `Thinking...` until text or a tool row
        // follows.
        let pi = rules::of("pi");
        let thinking = "\
 First think about the sky.
 Thinking...

── ⠙ Working ───────────────────────────────
────────────────────────────────────────────
Muse (1M context) │ ◈ 0% │ probe (main) │ ◖ medium
";
        assert_eq!(doing(pi, thinking).as_deref(), Some("Thinking..."));

        let running = "\
 First think about the sky.
 Thinking...
 $ sleep 30; echo done (timeout 40s)
 Elapsed 6.0s

── ⠼ Working ───────────────────────────────
────────────────────────────────────────────
Muse (1M context) │ ◈ 0% │ probe (main) │ ◖ medium
";
        assert_eq!(
            doing(pi, running).as_deref(),
            Some("Working"),
            "a row under Thinking... is the thinking over, and the border says what it says"
        );

        // A status line an extension wrote, and pi's own status row, still
        // read as before.
        let compacting = " ⠼ Compacting context... (escape to cancel)\n────────\n";
        assert_eq!(
            doing(pi, compacting).as_deref(),
            Some("Compacting context... (escape to cancel)")
        );
    }

    #[test]
    fn reader_says_what_an_agent_is_doing_only_where_it_read_it() {
        // Fresh hooks and a named tool: the line is read for the card, but the
        // row keeps the record's account while the tool runs.
        let mut running = state(Phase::Working, 1_000);
        running.summary = Some("Running Bash".to_string());
        let fresh = reading(&running, true, Some(A_WORKING_SCREEN), 1_000);
        assert_eq!(fresh.verdict.evidence, Evidence::Hooks);
        assert_eq!(
            fresh.doing.as_deref(),
            Some("Forging… (22s · ↓ 1.3k tokens)")
        );
        let root = TempDir::new().unwrap();
        let view = seen(&an_agent(&root), meta(), running.clone(), fresh);
        assert_eq!(
            view.state.summary.as_deref(),
            Some("Running Bash"),
            "the row keeps the tool the fresh hooks named"
        );
        assert_eq!(
            view.doing.as_deref(),
            Some("Forging… (22s · ↓ 1.3k tokens)"),
            "and the view carries the vendor's line beside it"
        );

        // A screen with no turn running says nothing about one: a question, an
        // idle prompt, or an unknown shell.
        for screen in [A_BLOCKING_SCREEN, IDLE_SCREEN, A_SHELL] {
            let reading = reading(&state(Phase::Starting, 1_000), true, Some(screen), 1_100);
            assert_eq!(reading.doing, None, "{screen}");
        }

        // So the row says what the record says.
        let mut told = state(Phase::Working, 1_000);
        told.summary = Some("Running Bash".to_string());
        let reading = reading(&told, true, Some(A_SHELL), 1_500);
        let root = TempDir::new().unwrap();
        assert_eq!(
            seen(&an_agent(&root), meta(), told, reading).line(),
            Some("Running Bash")
        );
    }

    #[test]
    fn reader_says_what_a_working_agent_is_saying_off_its_stream() {
        // pi is writing prose with no tool running. The record's summary is the
        // finished tool (pi reports no tool end), so the stream wins.
        let mut told = state(Phase::Working, 1_000);
        told.summary = Some("Running Bash".to_string());
        let root = TempDir::new().unwrap();
        let agent = an_agent(&root);
        let streaming = agent.dir().join(crate::store::LIVE);
        std::fs::write(&streaming, "\nThe redirect drops the query.\n\nFixing it.").unwrap();
        let view = seen(
            &agent,
            meta(),
            told.clone(),
            reading(&told, true, None, 1_000),
        );
        assert_eq!(view.line(), Some("The redirect drops the query."));
        assert_eq!(view.json()["summary"], "The redirect drops the query.");

        // A running tool has no stream: the vendor takes it down when the
        // message ends. The record stands.
        std::fs::remove_file(&streaming).unwrap();
        let quiet = seen(
            &agent,
            meta(),
            told.clone(),
            reading(&told, true, None, 1_000),
        );
        assert_eq!(quiet.line(), Some("Running Bash"));

        // A stream left behind a finished turn is not its answer.
        std::fs::write(&streaming, "\nThe redirect drops the query.\n\nFixing it.").unwrap();
        let mut ended = state(Phase::Idle, 1_000);
        ended.result = Some("the redirect keeps the query now".to_string());
        let done = seen(
            &agent,
            meta(),
            ended.clone(),
            reading(&ended, true, None, 1_000),
        );
        assert_eq!(done.line(), Some("the redirect keeps the query now"));
    }

    /// A claude transcript mid-turn: a sentence, and the call after it.
    const A_SENTENCE: &str = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"The importer keeps its own clock.\"}]}}\n";
    const A_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"file_path\":\"src/importer.rs\"}}]}}\n";

    #[test]
    fn reader_says_what_a_working_agent_last_said_off_its_transcript() {
        let root = TempDir::new().unwrap();
        let agent = an_agent(&root);
        let session = root.path().join("session.jsonl");
        let keeping_one = Meta {
            parent: None,
            depth: 0,
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            transcript: Some(session.clone()),
            ..meta()
        };

        // The hooks went quiet on a call ten minutes ago. The transcript is
        // newer than the record and the spinner: the agent answered that call
        // and has said a sentence since.
        let mut told = state(Phase::Working, 1_000);
        told.summary = Some("Running Bash".to_string());
        let row = |meta: &Meta, screen, now| {
            let reading = reading(&told, true, screen, now);
            seen(&agent, meta.clone(), told.clone(), reading)
                .line()
                .map(str::to_string)
        };

        std::fs::write(&session, A_SENTENCE).unwrap();
        assert_eq!(
            row(&keeping_one, Some(A_WORKING_SCREEN), 1_100).as_deref(),
            Some("The importer keeps its own clock."),
            "the newest line of the conversation, over the line the vendor spins"
        );

        // The next call lands: the tool and its one argument worth a row.
        std::fs::write(&session, format!("{A_SENTENCE}{A_CALL}")).unwrap();
        assert_eq!(
            row(&keeping_one, None, 1_000).as_deref(),
            Some("Read src/importer.rs"),
            "fresh hooks are still the older account of the same call"
        );

        // Nothing said this turn leaves the row alone: a transcript holding
        // only the user's prompt says nothing useful, and a record with no
        // transcript is not read for one. The spinner line is for the card.
        std::fs::write(
            &session,
            "{\"type\":\"user\",\"message\":{\"content\":\"port the importer\"}}\n",
        )
        .unwrap();
        for meta in [&keeping_one, &meta()] {
            for screen in [Some(A_WORKING_SCREEN), None] {
                assert_eq!(row(meta, screen, 1_000).as_deref(), Some("Running Bash"));
            }
        }

        // The vendor's stream comes first: it is the sentence being written.
        std::fs::write(&session, format!("{A_SENTENCE}{A_CALL}")).unwrap();
        std::fs::write(
            agent.dir().join(crate::store::LIVE),
            "Reading the importer's clock.",
        )
        .unwrap();
        assert_eq!(
            row(&keeping_one, Some(A_WORKING_SCREEN), 1_100).as_deref(),
            Some("Reading the importer's clock.")
        );

        // A finished turn is answered off the record, whatever the transcript
        // holds.
        let mut ended = state(Phase::Idle, 1_000);
        ended.result = Some("the importer keeps the clock now".to_string());
        let done = seen(
            &agent,
            keeping_one.clone(),
            ended.clone(),
            reading(&ended, true, None, 1_000),
        );
        assert_eq!(done.line(), Some("the importer keeps the clock now"));
    }

    #[test]
    fn reader_keeps_a_rewrite_of_this_turn_until_the_transcript_moves() {
        // A `summary_command` rewrite of the running turn covers the whole of
        // it, so it beats the transcript's last line until the agent says
        // something the rewrite cannot have read.
        let root = TempDir::new().unwrap();
        let agent = an_agent(&root);
        let session = root.path().join("session.jsonl");
        std::fs::write(&session, format!("{A_SENTENCE}{A_CALL}")).unwrap();
        let keeping_one = Meta {
            parent: None,
            depth: 0,
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            transcript: Some(session),
            ..meta()
        };
        let written = written_at(&keeping_one).expect("the transcript's own stamp");

        let mut told = state(Phase::Working, 1_000);
        told.summary = Some("Porting the importer's clock.".to_string());
        let row = || {
            seen(
                &agent,
                keeping_one.clone(),
                told.clone(),
                reading(&told, true, None, 1_000),
            )
            .line()
            .map(str::to_string)
        };
        let ask = |asked| assert!(write_asked(agent.dir(), asked), "the ask");

        ask(Asked {
            turn: told.since,
            at: written + 1,
            over: true,
        });
        assert_eq!(row().as_deref(), Some("Porting the importer's clock."));

        // The transcript moving takes it back.
        ask(Asked {
            turn: told.since,
            at: written - 1,
            over: true,
        });
        assert_eq!(row().as_deref(), Some("Read src/importer.rs"));

        // A line about the previous turn says nothing about this one, and an
        // ask still out has written no line.
        for asked in [
            Asked {
                turn: told.since - 1,
                at: written + 1,
                over: true,
            },
            Asked {
                turn: told.since,
                at: written + 1,
                over: false,
            },
        ] {
            ask(asked);
            assert_eq!(row().as_deref(), Some("Read src/importer.rs"));
        }

        // The spinner line does not overtake it either: it says how long the
        // turn has run, the rewrite says what it is doing.
        ask(Asked {
            turn: told.since,
            at: written + 1,
            over: true,
        });
        let spinning = seen(
            &agent,
            keeping_one.clone(),
            told.clone(),
            reading(&told, true, Some(A_WORKING_SCREEN), 1_100),
        );
        assert_eq!(spinning.line(), Some("Porting the importer's clock."));
    }

    #[test]
    fn reader_reads_the_stream_beside_the_record() {
        // Throughout: a working agent whose vendor streams reads as the
        // stream's first line.
        let root = TempDir::new().unwrap();
        let server = Own(
            Server::named(format!("amx-derive-live-{}", std::process::id())).with_conf("/dev/null"),
        );
        let pane = a_pane_showing(&server.0, &meta().id, A_SHELL);
        let meta = Meta {
            parent: None,
            depth: 0,
            socket: server.0.socket().clone(),
            pane,
            ..meta()
        };
        a_record(root.path(), &meta, &state(Phase::Working, 1_000));
        let agent = Agent::open(root.path(), &meta.id).unwrap();
        std::fs::write(
            agent.dir().join(crate::store::LIVE),
            "Reading the failing test first.\nThen the fix.",
        )
        .unwrap();

        let view = view(root.path(), &meta.id, 1_002).expect("a reading");
        assert_eq!(view.phase(), Phase::Working);
        assert_eq!(view.line(), Some("Reading the failing test first."));

        let wall = views(root.path(), 1_002).expect("a reading");
        assert_eq!(wall[0].line(), Some("Reading the failing test first."));
    }

    #[test]
    fn reader_reads_what_a_finished_turn_left_on_the_pane() {
        // pi's prompt against pi's document: the agent's rows come back
        // without the box, directory or stats line under them.
        let ended = read(
            &state(Phase::Starting, 1_000),
            0,
            true,
            || Some(A_PI_PROMPT.to_string()),
            rules::of("pi"),
            true,
            1_100,
            1,
            None,
        );
        assert_eq!(ended.verdict.phase, Phase::Idle);
        assert_eq!(
            ended.said.as_deref(),
            Some(" ran the migration\n\n Took 15.2s")
        );

        // A running turn, a screen asking a question and an unclaimed screen
        // have no answer to read.
        for screen in [A_WORKING_SCREEN, A_BLOCKING_SCREEN, A_SHELL] {
            let reading = reading(&state(Phase::Starting, 1_000), true, Some(screen), 1_100);
            assert_eq!(reading.said, None, "{screen}");
        }

        // The reading does not decide whether the screen is worth keeping;
        // `answers_on_the_pane` does, at the record write.
        let claude = reading(
            &state(Phase::Starting, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(claude.verdict.phase, Phase::Idle);
        assert!(
            claude.said.is_some(),
            "the same reading of a screen, on a vendor whose own words are \
             somewhere amx can read them"
        );
    }

    #[test]
    fn reader_writes_a_pane_down_only_where_nothing_else_will_ever_say_it() {
        // Only a vendor with neither hooks nor a transcript has its answers
        // on the pane alone; the test-only second vendor is that shape. claude
        // and pi report their answers themselves. An unregistered command
        // counts as neither, as it does for `logs`.
        let ran = |agent: Option<&str>| {
            answers_on_the_pane(vendor_of(&Meta {
                parent: None,
                depth: 0,
                agent: agent.map(str::to_string),
                model: None,
                effort: None,
                ..meta()
            }))
        };

        assert!(answers_on_the_pane(Some(&SECOND)));
        assert!(reads_its_own_record(Some(&SECOND)));
        assert!(!ran(Some("pi")), "pi reports through its extension now");
        assert!(!ran(Some("claude")));
        assert!(!ran(Some("mock-claude")), "an unregistered command");
        assert!(!ran(None), "and a record naming no command at all");
    }

    /// Whether a reading of this record captured the pane, the question
    /// [`wants_the_screen`] answers without capturing.
    fn looked_at_the_pane(
        meta: &Meta,
        state: &State,
        alive: bool,
        now: u64,
        heartbeat: Option<u64>,
    ) -> bool {
        let asked = std::cell::Cell::new(false);
        conclude(
            meta,
            state,
            alive,
            || {
                asked.set(true);
                Some(A_BLOCKING_SCREEN.to_string())
            },
            rules::of("claude"),
            now,
            1,
            heartbeat,
        );
        asked.get()
    }

    #[test]
    fn reader_says_which_readings_need_a_pane_before_it_takes_one() {
        // A wall works out which screens it wants from the records before
        // capturing, and must agree with the reading: wanting too few reads
        // `unknown` off a capture never taken, too many pays for unread
        // captures.
        let mut asked = state(Phase::Waiting, 1_000);
        asked.question = Some("Do you want to proceed?".to_string());
        let mut placeheld = state(Phase::Waiting, 1_000);
        placeheld.question = Some(A_PLACEHOLDER.to_string());
        let mut running = state(Phase::Working, 1_000);
        running.summary = Some("Running Bash".to_string());
        // A turn amx cut short is read off the pane at once, however fresh
        // the record.
        let mut cut = running.clone();
        cut.interrupted_at = 1_001;

        let records = [
            state(Phase::Starting, 1_000),
            state(Phase::Working, 1_000),
            running,
            cut,
            state(Phase::Waiting, 1_000),
            asked,
            placeheld,
            state(Phase::Idle, 1_000),
            state(Phase::Done, 1_000),
            state(Phase::Failed, 1_000),
            state(Phase::Stopped, 1_000),
        ];
        // Commands too: the two must agree on records with and without a
        // vendor.
        let started_by = [
            Meta {
                parent: None,
                depth: 0,
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
                ..meta()
            },
            meta(),
        ];
        for meta in &started_by {
            for record in &records {
                for alive in [true, false] {
                    // Fresh, on the last second of freshness, and stale.
                    for now in [1_000, 1_000 + FRESH, 1_100] {
                        // And with no beat, a beat as fresh as the window
                        // allows, and one past it: a beat must move both
                        // answers or neither.
                        for beat in [None, Some(now - FRESH), Some(now - FRESH - 1)] {
                            assert_eq!(
                                wants_the_screen(
                                    own_screens(meta),
                                    record,
                                    runs_a_command(meta, record),
                                    alive,
                                    now,
                                    beat
                                ),
                                looked_at_the_pane(meta, record, alive, now, beat),
                                "{} under {:?} alive={alive} at {now} beating {beat:?}",
                                record.state,
                                meta.agent
                            );
                        }
                    }
                }
            }
        }
    }

    /// A tmux server owned by the test, killed on drop.
    struct Own(Server);

    impl Drop for Own {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    /// A pane showing `screen` over a sleep, on its own server.
    ///
    /// In a session named for agent `id`, as [`crate::spawn::place`] names
    /// them, which is what makes the pane answer for that agent. Waits until
    /// the screen's last non-blank row (vendor chrome on every screen here) is
    /// captured, so the capture holds the whole screen.
    fn a_pane_showing(server: &Server, id: &str, screen: &str) -> crate::tmux::PaneId {
        let showing = [
            "sh",
            "-c",
            "printf '%s' \"$0\"; while :; do sleep 0.05; done",
            screen,
        ];
        let session = format!("{}{id}", crate::tmux::SESSION_PREFIX);
        let (_, pane) = server
            .new_session(&crate::tmux::Spawn {
                name: Some(&session),
                command: &showing,
                ..crate::tmux::Spawn::default()
            })
            .expect("a pane to read");
        let last = screen
            .lines()
            .rev()
            .find(|row| !row.trim().is_empty())
            .expect("a screen with something on it")
            .trim();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if server
                .capture(&pane)
                .is_ok_and(|drawn| drawn.contains(last))
            {
                return pane;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the screen never reached the pane");
    }

    /// Writes an agent's record straight to disk, bypassing the writer, so a
    /// test controls when it went quiet.
    fn a_record(root: &Path, meta: &Meta, state: &State) {
        let agent = Agent::create(root, meta).expect("a record");
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_vec(state).expect("a record"),
        )
        .expect("a record");
    }

    #[test]
    fn reader_gives_every_agent_of_a_wall_the_screen_of_its_own_pane() {
        let root = TempDir::new().unwrap();
        let server =
            Own(Server::named(format!("amx-derive-{}", std::process::id())).with_conf("/dev/null"));
        let socket = server.0.socket().clone();

        // Two quiet agents on one server with different screens. The screens
        // are captured in one call; the readings differing shows each got its
        // own.
        let asking = a_pane_showing(&server.0, "asks-a1b", A_BLOCKING_SCREEN);
        let idle = a_pane_showing(&server.0, "idles-b2c", IDLE_SCREEN);
        for (id, pane, phase) in [
            ("asks-a1b", &asking, Phase::Working),
            ("idles-b2c", &idle, Phase::Starting),
        ] {
            a_record(
                root.path(),
                &Meta {
                    parent: None,
                    depth: 0,
                    id: id.to_string(),
                    // Both run claude: a record naming no vendor is a
                    // command.
                    agent: Some("claude".to_string()),
                    model: None,
                    effort: None,
                    socket: socket.clone(),
                    pane: pane.clone(),
                    ..meta()
                },
                &state(phase, 1_000),
            );
        }

        let views = views(root.path(), 1_100).expect("a reading");
        let read = |id: &str| {
            views
                .iter()
                .find(|view| view.id() == id)
                .unwrap_or_else(|| panic!("{id} was read"))
        };
        assert_eq!(read("asks-a1b").phase(), Phase::Waiting);
        assert_eq!(
            read("asks-a1b").verdict.rule.as_deref(),
            Some("permission_prompt")
        );
        assert_eq!(read("idles-b2c").phase(), Phase::Idle);
        assert_eq!(
            read("idles-b2c").verdict.rule.as_deref(),
            Some("idle_prompt")
        );
    }

    #[test]
    fn reader_gives_a_pane_to_the_one_agent_it_answers_for() {
        // Two records naming one pane, as after a reboot: the server died, the
        // records never saw their panes go, and the new server numbers from %0
        // again. The pane answers for the agent whose session it is in; the
        // other record has lost it.
        let root = TempDir::new().unwrap();
        let server = Own(
            Server::named(format!("amx-derive-owner-{}", std::process::id()))
                .with_conf("/dev/null"),
        );
        let socket = server.0.socket().clone();
        let pane = a_pane_showing(&server.0, "holds-it-a1b", IDLE_SCREEN);
        for id in ["holds-it-a1b", "lost-it-b2c"] {
            a_record(
                root.path(),
                &Meta {
                    parent: None,
                    depth: 0,
                    id: id.to_string(),
                    agent: Some("claude".to_string()),
                    model: None,
                    effort: None,
                    socket: socket.clone(),
                    pane: pane.clone(),
                    ..meta()
                },
                &state(Phase::Starting, 1_000),
            );
        }

        let read = |id: &str| view(root.path(), id, 1_100).expect("a reading");
        assert_eq!(read("holds-it-a1b").phase(), Phase::Idle);
        assert_eq!(read("holds-it-a1b").verdict.evidence, Evidence::Screen);
        assert_eq!(read("lost-it-b2c").phase(), Phase::Stopped);
        assert_eq!(
            read("lost-it-b2c").verdict.evidence,
            Evidence::Gone,
            "a pane answering for somebody else is a pane this record lost"
        );

        // A wall reads them the same way off its one listing.
        let wall = views(root.path(), 1_100).expect("a reading");
        let seen = |id: &str| {
            wall.iter()
                .find(|view| view.id() == id)
                .unwrap_or_else(|| panic!("{id} was read"))
        };
        assert_eq!(seen("holds-it-a1b").phase(), Phase::Idle);
        assert_eq!(seen("holds-it-a1b").verdict.evidence, Evidence::Screen);
        assert_eq!(seen("lost-it-b2c").phase(), Phase::Stopped);
        assert_eq!(seen("lost-it-b2c").verdict.evidence, Evidence::Gone);

        // A parked record keeps its state, as it does for a missing pane:
        // both name a pane they no longer have.
        let mut parked = state(Phase::Idle, 1_000);
        parked.parked_at = 1_050;
        let lost = Agent::open(root.path(), "lost-it-b2c").expect("the record");
        std::fs::write(
            lost.dir().join("state.json"),
            serde_json::to_vec(&parked).expect("a record"),
        )
        .expect("a record");
        assert_eq!(read("lost-it-b2c").phase(), Phase::Idle);
        assert_eq!(read("lost-it-b2c").verdict.evidence, Evidence::LetGo);
    }

    #[test]
    fn reader_leaves_the_answer_to_a_vendor_that_reports_it_itself() {
        let root = TempDir::new().unwrap();
        let server = Own(
            Server::named(format!("amx-derive-said-{}", std::process::id())).with_conf("/dev/null"),
        );
        let socket = server.0.socket().clone();

        // Two quiet agents at finished turns, each on its own vendor's prompt.
        // claude and pi both report their answers (claude in its Stop payload
        // and transcript, pi through its extension's report and session file),
        // so nothing is written from the screen. The reading still says idle.
        let pi = a_pane_showing(&server.0, "pi-a1b", A_PI_PROMPT);
        let claude = a_pane_showing(&server.0, "claude-b2c", IDLE_SCREEN);
        for (id, agent, pane) in [("pi-a1b", "pi", &pi), ("claude-b2c", "claude", &claude)] {
            a_record(
                root.path(),
                &Meta {
                    parent: None,
                    depth: 0,
                    id: id.to_string(),
                    agent: Some(agent.to_string()),
                    model: None,
                    effort: None,
                    socket: socket.clone(),
                    pane: pane.clone(),
                    ..meta()
                },
                &state(Phase::Starting, 1_000),
            );
        }

        let views = views(root.path(), 1_100).expect("a reading");
        let read = |id: &str| {
            views
                .iter()
                .find(|view| view.id() == id)
                .unwrap_or_else(|| panic!("{id} was read"))
        };
        let kept = |id: &str| {
            Agent::open(root.path(), id)
                .expect("the record")
                .state()
                .expect("the record")
        };

        assert_eq!(read("pi-a1b").phase(), Phase::Idle);
        assert_eq!(
            read("pi-a1b").verdict.rule.as_deref(),
            Some("prompt"),
            "pi's own rule, out of pi's own document"
        );
        assert_eq!(read("pi-a1b").state.result, None);
        assert_eq!(
            kept("pi-a1b").result,
            None,
            "a vendor that reports what it answered is left to report it"
        );
        assert_eq!(
            kept("pi-a1b").last_event,
            1_000,
            "and heard nothing by looking at a screen"
        );

        assert_eq!(read("claude-b2c").phase(), Phase::Idle);
        assert_eq!(read("claude-b2c").state.result, None);
        assert_eq!(
            kept("claude-b2c").result,
            None,
            "a vendor that says what it answered is left to say it"
        );
    }

    #[test]
    fn reader_answers_from_the_records_before_it_has_asked_tmux_anything() {
        let root = TempDir::new().unwrap();
        let socket = crate::tmux::Socket::Name("amx-not-a-server".to_string());
        a_record(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                socket,
                created: 1_000,
                ..meta()
            },
            &state(Phase::Working, 1_000),
        );

        // Nothing listens on that socket, so asking tmux would find no pane
        // and call it stopped. The records say working, and that is all a
        // surface has before it waits for anything.
        let recorded = recorded(root.path(), 1_100).expect("the records");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].phase(), Phase::Working);
        assert_eq!(recorded[0].verdict.evidence, Evidence::Hooks);
        assert_eq!(recorded[0].verdict.age, 100, "with the record's own clock");

        let read = views(root.path(), 1_100).expect("a reading");
        assert_eq!(
            read[0].phase(),
            Phase::Stopped,
            "which is what looking at the pane costs"
        );
        assert_eq!(read[0].verdict.evidence, Evidence::Gone);
    }

    #[test]
    fn reader_reads_the_record_as_written_where_a_tmux_that_cannot_be_asked_is() {
        // A tmux that never ran said nothing about the pane, so the record
        // stands and nothing is written: calling it stopped would end every
        // wait on it.
        let root = TempDir::new().unwrap();
        a_record(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                socket: crate::tmux::unaskable(),
                created: 900,
                ..meta()
            },
            &state(Phase::Working, 1_000),
        );
        let file = root.path().join("fix-login-a1b").join("state.json");
        let before = std::fs::read(&file).unwrap();

        let one = view(root.path(), "fix-login-a1b", 1_100).expect("a reading");
        let all = views(root.path(), 1_100).expect("a reading");
        for read in [&one, &all[0]] {
            assert_eq!(read.phase(), Phase::Working);
            assert_eq!(read.verdict.evidence, Evidence::Record);
        }
        assert_eq!(
            std::fs::read(&file).unwrap(),
            before,
            "and the record stands"
        );
    }

    #[test]
    fn reader_reads_a_record_that_has_ended_the_same_way_with_or_without_a_look() {
        // The record ended it, and readers with and without a look agree.
        let root = TempDir::new().unwrap();
        let mut done = state(Phase::Done, 1_000);
        done.ended = 1_000;
        done.exit = Some(0);
        a_record(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                created: 900,
                ..meta()
            },
            &done,
        );

        let recorded = recorded(root.path(), 1_100).expect("the records");
        assert_eq!(recorded[0].phase(), Phase::Done);
        assert_eq!(recorded[0].verdict.evidence, Evidence::Record);
        assert_eq!(recorded[0].verdict.age, 100, "and how long it worked");
    }

    #[test]
    fn reader_takes_an_ended_run_with_no_spans_but_turns_in_its_log_off_the_log() {
        // No spans on the record, and the log has a minute's turn in a
        // day-long run: it worked a minute. The whole run is only for a log
        // with no turn edges.
        let root = TempDir::new().unwrap();
        let mut done = state(Phase::Done, 90_000);
        done.ended = 90_000;
        let turned = Meta {
            parent: None,
            depth: 0,
            agent: Some("claude".to_string()),
            created: 900,
            ..meta()
        };
        a_record(root.path(), &turned, &done);
        let agent = Agent::open(root.path(), &turned.id).unwrap();
        let writer = agent.writer().unwrap();
        for (at, kind) in [
            (1_000, "SessionStart"),
            (1_010, "UserPromptSubmit"),
            (1_070, "Stop"),
        ] {
            writer
                .append(&crate::store::Event {
                    at,
                    kind: kind.to_string(),
                    payload: serde_json::json!({"hook_event_name": kind}),
                })
                .unwrap();
        }
        drop(writer);

        let first = &recorded(root.path(), 99_000).unwrap()[0];
        assert_eq!((first.verdict.worked, first.verdict.age), (60, 60));
        let read = view(root.path(), &turned.id, 99_000).unwrap();
        assert_eq!((read.verdict.worked, read.verdict.age), (60, 60));

        // A log with no turn edges still reads as the whole run.
        std::fs::write(agent.events_path(), "").unwrap();
        let spanless = &recorded(root.path(), 99_000).unwrap()[0];
        assert_eq!(spanless.verdict.worked, 89_100);
    }

    #[test]
    fn reader_parses_an_ended_runs_log_again_only_when_it_changes() {
        let root = TempDir::new().unwrap();
        let mut done = state(Phase::Done, 90_000);
        done.ended = 90_000;
        let turned = Meta {
            parent: None,
            depth: 0,
            id: "logged-l0g".to_string(),
            agent: Some("claude".to_string()),
            created: 900,
            ..meta()
        };
        a_record(root.path(), &turned, &done);
        let agent = Agent::open(root.path(), &turned.id).unwrap();
        let turn = |writer: &crate::store::Writer<'_>, opens: u64, closes: u64| {
            for (at, kind) in [(opens, "UserPromptSubmit"), (closes, "Stop")] {
                writer
                    .append(&crate::store::Event {
                        at,
                        kind: kind.to_string(),
                        payload: serde_json::json!({"hook_event_name": kind}),
                    })
                    .unwrap();
            }
        };
        turn(&agent.writer().unwrap(), 1_010, 1_070);
        let worked = || recorded(root.path(), 99_000).unwrap()[0].verdict.worked;
        assert_eq!(worked(), 60);

        // Rewritten to the same size with its mtime put back: the cached count
        // stands, so the log was not parsed again.
        let log = agent.events_path();
        let modified = std::fs::metadata(&log).unwrap().modified().unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        std::fs::write(&log, text.replace("1070", "1130")).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&log)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(worked(), 60);

        // An append changes the size, and the whole log is counted again.
        turn(&agent.writer().unwrap(), 2_000, 2_030);
        assert_eq!(worked(), 120 + 30);
    }

    #[test]
    fn records_skips_an_agent_whose_state_json_is_unreadable_but_keeps_the_rest() {
        let root = TempDir::new().unwrap();
        let broken = Agent::create(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                id: "broken-a1b".to_string(),
                ..meta()
            },
        )
        .expect("a record");
        std::fs::write(broken.dir().join("state.json"), b"not json at all").expect("garbage bytes");
        a_record(
            root.path(),
            &Meta {
                parent: None,
                depth: 0,
                id: "fine-b2c".to_string(),
                ..meta()
            },
            &state(Phase::Working, 1_000),
        );

        let records = records(root.path()).expect("the walk to finish");
        assert_eq!(
            records.iter().map(|r| r.agent.id()).collect::<Vec<_>>(),
            vec!["fine-b2c"]
        );
    }

    #[test]
    fn records_skips_an_agent_removed_after_the_listing() {
        // `amx clear` or a sweep in another amx can remove a directory between
        // `store::list` and the open. That costs the one record, not the whole
        // listing.
        let root = TempDir::new().unwrap();
        a_record(root.path(), &meta(), &state(Phase::Idle, 1_000));
        let listed = vec!["going-g0e".to_string(), meta().id];
        let records = records_of(root.path(), listed);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].meta.id, meta().id);
    }

    #[test]
    fn records_reads_a_phase_this_build_has_never_heard_of_as_unknown() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta()).expect("a record");
        std::fs::write(agent.dir().join("state.json"), br#"{"state":"reviewing"}"#)
            .expect("a state naming an unrecognized phase");

        let records = records(root.path()).expect("the walk to finish");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state.state, Phase::Unknown);
    }

    #[test]
    fn reader_takes_the_record_when_the_record_says_how_it_ended() {
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
            // Long stale and no pane, but the record says it is over.
            let verdict = decided(&state(phase, 100), false, Some(A_SHELL), 10_000);
            assert_eq!(verdict.phase, phase);
            assert_eq!(verdict.evidence, Evidence::Record);
        }
    }

    #[test]
    fn reader_calls_an_agent_with_no_pane_stopped() {
        let verdict = decided(&state(Phase::Working, 1_000), false, None, 1_001);
        assert_eq!(verdict.phase, Phase::Stopped);
        assert_eq!(
            verdict.evidence,
            Evidence::Gone,
            "fresh hooks do not outrank a pane that is not there"
        );
    }

    #[test]
    fn reader_trusts_the_hooks_while_they_are_fresh() {
        let verdict = decided(&state(Phase::Working, 1_000), true, None, 1_000 + FRESH);
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);
        assert_eq!(verdict.age, FRESH);
        assert_eq!(verdict.rule, None, "the screen was never asked for");
    }

    #[test]
    fn reader_believes_a_fresh_beat_the_way_it_believes_a_hook() {
        // Half a minute into a tool call the record is stale and the screen
        // is one no rule claims. The vendor's report beats on the record while
        // the turn runs, and a beat is heard like a hook.
        let running = state(Phase::Working, 1_000);
        let beaten = beating(&running, Some(A_SHELL), 1_030, Some(1_028));
        assert_eq!(beaten.verdict.phase, Phase::Working);
        assert_eq!(beaten.verdict.evidence, Evidence::Hooks);
        assert_eq!(beaten.verdict.age, 2, "the beat is the last thing heard");

        // No beat, or one older than the window, leaves the record unheard
        // from.
        for beat in [None, Some(1_030 - FRESH - 1)] {
            let quiet = beating(&running, Some(A_SHELL), 1_030, beat);
            assert_eq!(quiet.verdict.phase, Phase::Unknown, "{beat:?}");
            assert_eq!(quiet.verdict.evidence, Evidence::Unknown, "{beat:?}");
        }
    }

    #[test]
    fn reader_reads_the_screen_once_the_hooks_have_gone_quiet() {
        // Nothing outstanding, so the idle rule decides at once: this is the
        // parked agent that would otherwise sit at `starting`.
        let verdict = decided(
            &state(Phase::Starting, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"));
        assert_eq!(verdict.age, 100);
    }

    #[test]
    fn reader_does_not_end_a_running_turn_on_one_look() {
        // The idle screen and a mid-turn pause are the same bytes. With a turn
        // on the record, one look at a still screen decides nothing.
        let verdict = decided(
            &state(Phase::Working, 1_000),
            true,
            Some(IDLE_SCREEN),
            1_100,
        );
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"), "and says why");
        assert_eq!(verdict.age, 100);
    }

    #[test]
    fn reader_leaves_a_turn_whose_shells_are_still_running_at_work() {
        // A finished turn's prompt and one with shells still running behind it
        // look alike. The count the Stop hook wrote tells them apart, so the
        // rule names the screen without ending the turn.
        let mut running = state(Phase::Working, 1_000);
        running.background = 2;

        let verdict = settled(&running, A_SCREEN_WITH_A_SHELL, 1_100);
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"), "and says why");

        // With nothing running, the same screen after the same wait ends it.
        let verdict = settled(&state(Phase::Working, 1_000), IDLE_SCREEN, 1_100);
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::Screen);
    }

    #[test]
    fn reader_spends_a_shell_count_the_screen_has_stopped_agreeing_with() {
        // Nothing is sent when somebody stops a background shell in the pane,
        // so the count goes stale. Without this the row said `working` until
        // the agent died.
        let mut running = state(Phase::Working, 1_000);
        running.background = 2;

        let verdict = settled(&running, A_SCREEN_WITH_NO_SHELL, 1_100);
        assert_eq!(verdict.phase, Phase::Idle, "the count is spent");
        assert_eq!(verdict.evidence, Evidence::Screen);

        // The same screen with the vendor's shell count on its footer keeps
        // the turn running.
        let verdict = settled(&running, A_SCREEN_WITH_A_SHELL, 1_100);
        assert_eq!(verdict.phase, Phase::Working);

        // The quiescent wait is unchanged: one look at a still prompt ends
        // nothing.
        let verdict = decided(&running, true, Some(A_SCREEN_WITH_NO_SHELL), 1_100);
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);
    }

    #[test]
    fn reader_takes_a_vendor_that_says_nothing_about_its_shells_at_the_count() {
        // A footer with no measured shell tail says nothing either way, and
        // silence is not "no shells".
        let rules = rules::of("pi");
        assert!(
            rules
                .furniture()
                .shells_running(&["⏵⏵ bypass permissions on (shift+tab to cycle)"])
                .is_none()
        );
        assert!(still_has_a_shell(rules, A_SCREEN_WITH_NO_SHELL));
    }

    #[test]
    fn reader_ends_a_turn_somebody_cut_short_in_the_pane_on_the_first_look() {
        // claude fires no hook on esc, so only the pane says the turn ended.
        // Its interrupted row does, so neither the fresh window nor the
        // quiescent wait holds the row at `working`.
        let running = state(Phase::Working, 1_000);
        let verdict = decided(&running, true, Some(AN_INTERRUPTED_SCREEN), 1_001);
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"));

        // The row stays in the transcript for the session, so only being the
        // last row makes it news. With a turn running over it, the hooks
        // decide.
        let verdict = decided(&running, true, Some(A_SCREEN_PAST_AN_INTERRUPT), 1_001);
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Hooks);

        // A vendor with no such row waits as before.
        assert!(
            !rules::of("pi")
                .furniture()
                .cut_by_hand(&AN_INTERRUPTED_SCREEN.lines().collect::<Vec<&str>>())
        );
    }

    #[test]
    fn reader_ends_a_turn_amx_cut_short_as_soon_as_the_prompt_is_up() {
        // claude fires no hook on an interrupt, so the record says a turn amx
        // itself ended is running. The stamp is amx's word that it is over, so
        // the pane is read on the first look and the prompt rule may end it.
        let mut cut = state(Phase::Working, 1_000);
        cut.interrupted_at = 1_002;

        let verdict = decided(&cut, true, Some(IDLE_SCREEN), 1_003);
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"));
    }

    #[test]
    fn reader_ends_a_turn_cut_short_in_the_second_the_vendor_last_spoke_in() {
        // Both stamps are whole seconds and a busy turn hooks about once a
        // second, so the key often lands in the same second as the last hook.
        // Comparing them read that as no interrupt. The stamp stands until the
        // vendor speaks.
        let mut tied = state(Phase::Working, 1_000);
        tied.interrupted_at = 1_000;

        let verdict = decided(&tied, true, Some(IDLE_SCREEN), 1_001);
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.rule.as_deref(), Some("idle_prompt"));
    }

    #[test]
    fn reader_reads_a_cut_turn_that_is_still_drawing_as_working() {
        // The key is sent without waiting, so the vendor may still be spinning
        // at the next look. The stamp says to read the pane, not what to find.
        let mut cut = state(Phase::Working, 1_000);
        cut.interrupted_at = 1_002;

        let verdict = decided(&cut, true, Some(A_WORKING_SCREEN), 1_003);
        assert_eq!(verdict.phase, Phase::Working);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.rule.as_deref(), Some("spinner"));
    }

    #[test]
    fn reader_takes_a_hook_after_an_interrupt_as_the_agent_speaking_again() {
        // The vendor's next event is its own account of a later moment (the
        // next turn, a question), so the hook that carries it clears the stamp.
        // Only the hook clears it, so the hook is driven here.
        let mut spoke = state(Phase::Working, 1_010);
        spoke.interrupted_at = 1_002;
        crate::hook::apply(
            &serde_json::json!({ "hook_event_name": "UserPromptSubmit", "prompt": "carry on" }),
            &mut spoke,
            &mut meta(),
        );

        let fresh = decided(&spoke, true, Some(IDLE_SCREEN), 1_012);
        assert_eq!(fresh.phase, Phase::Working);
        assert_eq!(fresh.evidence, Evidence::Hooks);
        assert_eq!(fresh.rule, None, "and the screen decided nothing");

        // Once quiet again, the screen has to hold still to end the turn, as
        // over any running turn.
        let quiet = decided(&spoke, true, Some(IDLE_SCREEN), 1_100);
        assert_eq!(quiet.phase, Phase::Working);
        assert_eq!(quiet.evidence, Evidence::Hooks);
        assert_eq!(quiet.rule.as_deref(), Some("idle_prompt"), "unsettled");
    }

    /// claude's chrome over a transcript that never moves, with `tick`
    /// standing in for a statusline value that changes every second. Two
    /// ticks are two captures of the same still screen.
    fn a_ticking_screen(tick: u64) -> String {
        format!(
            "\
  Done building the feature.

──────────────────────────── amx-42 ─
❯
───────────────────────────────────────
  Sonnet 5 · {tick} tokens
  ⏵⏵ auto mode on (shift+tab to cycle)
"
        )
    }

    /// An agent whose record says a turn is running.
    fn a_quiet_agent(root: &Path, id: &str) -> Agent {
        a_record(
            root,
            &Meta {
                parent: None,
                depth: 0,
                id: id.to_string(),
                ..meta()
            },
            &state(Phase::Working, 1_000),
        );
        Agent::open(root, id).expect("the record just written")
    }

    #[test]
    fn held_still_measures_the_screen_and_not_the_chrome_ticking_over_it() {
        let root = TempDir::new().unwrap();
        let agent = a_quiet_agent(root.path(), "ticking-t4a");
        let rules = rules::of("claude");
        let mut state = agent.state().unwrap();

        assert_eq!(
            held_still(&agent, &mut state, Some(&a_ticking_screen(0)), rules, 1_000),
            0,
            "a screen amx has just laid eyes on has held still for no time at all"
        );
        assert_eq!(
            held_still(
                &agent,
                &mut state,
                Some(&a_ticking_screen(rules::SETTLED_LOOKS)),
                rules,
                1_000 + rules::SETTLED_LOOKS
            ),
            rules::SETTLED_LOOKS,
            "the statusline ticked the whole way, and none of it is the screen"
        );
    }

    #[test]
    fn held_still_leaves_the_stamp_on_the_record_for_whoever_looks_next() {
        let root = TempDir::new().unwrap();
        let agent = a_quiet_agent(root.path(), "settles-t4b");
        let rules = rules::of("claude");
        let screen = a_ticking_screen(1);

        let mut looking = agent.state().unwrap();
        assert_eq!(
            held_still(&agent, &mut looking, Some(&screen), rules, 1_000),
            0
        );

        let written = agent.state().unwrap();
        assert_eq!(
            written.still.map(|still| still.since),
            Some(1_000),
            "when the screen went up, not when it was last looked at"
        );
        assert_eq!(
            written.last_event, 1_000,
            "and a look is not something the agent said"
        );

        // A fresh process reads the stamp the previous look left, so the
        // screen has held still as long as the stamp says.
        let mut next = agent.state().unwrap();
        assert_eq!(
            held_still(
                &agent,
                &mut next,
                Some(&screen),
                rules,
                1_000 + rules::SETTLED_LOOKS
            ),
            rules::SETTLED_LOOKS,
            "the stillness one reader watched is the stillness the next one reads"
        );
    }

    #[test]
    fn held_still_starts_the_clock_again_the_moment_the_transcript_changes() {
        let root = TempDir::new().unwrap();
        let agent = a_quiet_agent(root.path(), "changes-t4c");
        let rules = rules::of("claude");
        let first = a_ticking_screen(1);

        let mut state = agent.state().unwrap();
        held_still(&agent, &mut state, Some(&first), rules, 1_000);
        assert_eq!(
            held_still(&agent, &mut state, Some(&first), rules, 1_020),
            20
        );

        let changed = a_ticking_screen(1).replace("Done building", "Ran the migration and");
        assert_eq!(
            held_still(&agent, &mut state, Some(&changed), rules, 1_020),
            0,
            "a mid-turn change is not the screen holding still"
        );
        assert_eq!(
            agent.state().unwrap().still.map(|still| still.since),
            Some(1_020),
            "and the screen on the pane now is the one the next look compares against"
        );
    }

    #[test]
    fn held_still_is_not_kept_for_a_command() {
        // No rule reads a command's pane, so its output changing is no reason
        // to rewrite its record on every look.
        let root = TempDir::new().unwrap();
        a_record(root.path(), &meta(), &state(Phase::Starting, 1_000));
        for (screen, now) in [(A_COMMAND, 1_010), ("running 3 tests\n", 1_011)] {
            let record = records(root.path()).unwrap().pop().expect("the record");
            let view = look(
                root.path(),
                record,
                true,
                Some(screen.to_string()),
                now,
                None,
            );
            assert_eq!(view.phase(), Phase::Working);
        }
        let agent = Agent::open(root.path(), &meta().id).unwrap();
        assert_eq!(agent.state().unwrap().still, None);
    }

    #[test]
    fn held_still_says_nothing_about_a_pane_nobody_read() {
        let root = TempDir::new().unwrap();
        let agent = a_quiet_agent(root.path(), "unread-t4d");
        let mut state = agent.state().unwrap();

        assert_eq!(
            held_still(&agent, &mut state, None, rules::of("claude"), 9_000),
            0,
            "a reading that wanted no screen leaves the record where it was"
        );
        assert_eq!(agent.state().unwrap().still, None);
    }

    #[test]
    fn reader_says_unknown_rather_than_guessing() {
        let verdict = decided(&state(Phase::Working, 1_000), true, Some(A_SHELL), 1_500);
        assert_eq!(verdict.phase, Phase::Unknown);
        assert_eq!(verdict.evidence, Evidence::Unknown);
        assert_eq!(verdict.age, 500, "and how long it has been out of touch");

        // A pane that cannot be captured gives the same answer.
        let unreadable = decided(&state(Phase::Working, 1_000), true, None, 1_500);
        assert_eq!(unreadable.phase, Phase::Unknown);
    }

    #[test]
    fn reader_reads_a_running_command_off_the_line_it_last_printed() {
        // No vendor on the record: nothing goes stale and no rules apply. A
        // live pane means the command is running, and its last printed row is
        // the summary.
        let ran = meta();
        let starting = state(Phase::Starting, 1_000);
        let printed = || Some(A_COMMAND.to_string());
        let reading = conclude(
            &ran,
            &starting,
            true,
            printed,
            own_screens(&ran),
            1_500,
            1,
            None,
        );
        assert_eq!(reading.verdict.phase, Phase::Working);
        assert_eq!(reading.verdict.evidence, Evidence::Screen);
        assert_eq!(reading.verdict.age, 500, "and how long it has been running");

        let root = TempDir::new().unwrap();
        let view = seen(&an_agent(&root), ran, starting.clone(), reading);
        assert_eq!(view.line(), Some("test reads_the_line ... ok"));
        assert_eq!(view.json()["state"], "working");
        assert_eq!(view.json()["summary"], "test reads_the_line ... ok");

        // A command whose pane is gone is not running, and an exit code is
        // not read off a screen.
        let gone = conclude(
            &meta(),
            &starting,
            false,
            printed,
            rules::of(""),
            1_500,
            1,
            None,
        );
        assert_eq!(gone.verdict.phase, Phase::Stopped);

        let mut failed = state(Phase::Failed, 1_000);
        failed.exit = Some(3);
        let ended = conclude(
            &meta(),
            &failed,
            true,
            printed,
            rules::of(""),
            1_500,
            1,
            None,
        );
        assert_eq!(ended.verdict.phase, Phase::Failed);
        assert_eq!(ended.verdict.evidence, Evidence::Record);

        // A record naming a vendor is read against that vendor's screens.
        let agent = Meta {
            parent: None,
            depth: 0,
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            ..meta()
        };
        let claude = conclude(
            &agent,
            &starting,
            true,
            printed,
            rules::of("claude"),
            1_500,
            1,
            None,
        );
        assert_eq!(claude.verdict.phase, Phase::Unknown);
    }

    #[test]
    fn reader_gives_a_running_command_the_whole_of_its_life_as_its_work() {
        // A command's phase never leaves `starting`, so it never opens a span.
        // Its work is its whole life: started five seconds ago, worked five.
        let starting = state(Phase::Starting, 1_000);
        assert_eq!(starting.worked_by(1_005), 0, "no span was ever opened");

        let running = read_a_command(&starting, 1_000, Some(A_COMMAND), 1_005, None);
        assert_eq!(running.verdict.worked, 5);
        assert_eq!(running.verdict.age, 5, "which is its age as well");
    }

    #[test]
    fn reader_takes_a_commands_line_off_the_bottom_of_the_screen() {
        assert_eq!(last_printed(A_COMMAND), Some("test reads_the_line ... ok"));
        assert_eq!(
            last_printed("  \n\n"),
            None,
            "a screen with nothing on it says nothing"
        );
    }

    #[test]
    fn reader_keeps_a_reporting_vendors_settled_word_on_a_screen_nobody_claims() {
        // pi's hooks said the turn ended a minute ago, and pi's update notice
        // over the prompt is claimed by no rule. Nothing has contradicted the
        // vendor, so the record stands, at its age.
        let idle = heard_from(
            true,
            0,
            &state(Phase::Idle, 1_000),
            true,
            Some(A_SHELL),
            1_060,
        );
        assert_eq!(idle.verdict.phase, Phase::Idle);
        assert_eq!(idle.verdict.evidence, Evidence::Hooks);
        assert_eq!(idle.verdict.age, 60);

        // A question the vendor named stays for the same reason.
        let mut asked = state(Phase::Waiting, 1_000);
        asked.question = Some("Run echo hi?".to_string());
        let waiting = heard_from(true, 0, &asked, true, Some(A_SHELL), 1_060);
        assert_eq!(waiting.verdict.phase, Phase::Waiting);
        assert_eq!(waiting.verdict.evidence, Evidence::Hooks);

        // A turn recorded as running is still unknown: nothing said it ended,
        // and an unmeasured screen may be a missed question. So is a record
        // with nothing reported yet.
        for phase in [Phase::Working, Phase::Starting] {
            let mid = heard_from(true, 0, &state(phase, 1_000), true, Some(A_SHELL), 1_060);
            assert_eq!(mid.verdict.phase, Phase::Unknown, "{phase}");
        }

        // A vendor that reports nothing has no word to keep.
        let silent = heard_from(
            false,
            0,
            &state(Phase::Idle, 1_000),
            true,
            Some(A_SHELL),
            1_060,
        );
        assert_eq!(silent.verdict.phase, Phase::Unknown);
    }

    #[test]
    fn reader_freezes_the_clock_on_a_run_that_has_ended() {
        // Started at 1_000, worked ten seconds and waited at a question for
        // the hour between. It worked ten seconds, whenever anybody asks.
        let mut done = state(Phase::Done, 4_610);
        done.ended = 4_610;
        done.worked = 10;

        assert_eq!(started(1_000, &done, true, None, 4_620).verdict.age, 10);
        assert_eq!(
            started(1_000, &done, true, None, 90_000).verdict.age,
            10,
            "a day later it is still the run it was"
        );

        let view = View::new(
            Meta {
                parent: None,
                depth: 0,
                created: 1_000,
                ..meta()
            },
            done.clone(),
            started(1_000, &done, true, None, 90_000).verdict,
        );
        assert_eq!(view.json()["age"], 10);
        assert_eq!(view.json()["worked"], 10, "and the spans it was added from");
        assert_eq!(view.json()["ended"], 4_610, "and when it ended, whole");
    }

    #[test]
    fn reader_ticks_the_work_column_only_while_the_agent_works() {
        // Working: the added-up spans plus the open one, moving with the
        // clock.
        let mut working = state(Phase::Working, 1_000);
        working.worked = 120;
        assert_eq!(
            reading(&working, true, None, 1_000 + FRESH).verdict.worked,
            120 + FRESH
        );

        // Waiting: frozen where the work stopped. The wait is still the age,
        // which the card reads.
        let mut waiting = state(Phase::Waiting, 2_000);
        waiting.worked = 120;
        let read = reading(&waiting, true, None, 2_000 + FRESH).verdict;
        assert_eq!(read.worked, 120);
        assert_eq!(read.age, FRESH, "and the wait stays on the age");

        // Idle: frozen too, until the next turn opens a span.
        let mut idle = state(Phase::Idle, 2_000);
        idle.worked = 120;
        assert_eq!(
            reading(&idle, true, None, 2_000 + FRESH).verdict.worked,
            120
        );

        // Ended: what it worked, for good, or the whole run where no spans
        // were added up; the same as the age.
        let mut done = state(Phase::Done, 4_610);
        done.ended = 4_610;
        done.worked = 10;
        assert_eq!(started(1_000, &done, true, None, 90_000).verdict.worked, 10);
        done.worked = 0;
        assert_eq!(
            started(1_000, &done, true, None, 90_000).verdict.worked,
            3_610
        );
    }

    #[test]
    fn reader_says_its_number_in_one_set_of_units() {
        // Every surface prints the number through `in_words`, so the units
        // cannot differ between them.
        assert_eq!(in_words(0), "0s");
        assert_eq!(in_words(45), "45s");
        assert_eq!(in_words(59), "59s");
        assert_eq!(in_words(60), "1m");
        assert_eq!(in_words(3_599), "59m");
        assert_eq!(in_words(3_600), "1h");
        assert_eq!(in_words(86_399), "23h");
        assert_eq!(in_words(86_400), "1d");

        // A reading's number goes through it too, a day later as well.
        let mut done = state(Phase::Done, 4_610);
        done.ended = 4_610;
        done.worked = 10;
        let verdict = started(1_000, &done, true, None, 90_000).verdict;
        assert_eq!(in_words(verdict.age), "10s");
    }

    #[test]
    fn reader_counts_a_span_of_work_nothing_ever_closed() {
        // The pane went mid-turn, so the last span is still open on the
        // record. It closes at the last thing heard.
        let mut killed = state(Phase::Working, 1_300);
        killed.since = 1_200;
        killed.worked = 20;

        let verdict = started(1_000, &killed, false, None, 5_000).verdict;
        assert_eq!(verdict.phase, Phase::Stopped);
        assert_eq!(
            verdict.age, 120,
            "twenty seconds, and the hundred it was in"
        );
    }

    #[test]
    fn reader_reads_a_run_with_no_spans_on_it_as_the_whole_of_the_run() {
        // A record from before spans were added up, and one of an agent that
        // never worked, both show how long the run was alive.
        let mut older = state(Phase::Done, 1_300);
        older.ended = 1_300;
        assert_eq!(older.worked, 0);
        assert_eq!(started(1_000, &older, true, None, 9_000).verdict.age, 300);
    }

    #[test]
    fn reader_dates_an_ending_nobody_stamped_from_the_last_thing_it_said() {
        // A record written by an older amx has no end stamp.
        let unstamped = state(Phase::Done, 1_300);
        assert_eq!(unstamped.ended, 0);
        assert_eq!(
            started(1_000, &unstamped, true, None, 5_000).verdict.age,
            300
        );

        // The pane went without recording an exit: the run lasts until the
        // last thing heard.
        let killed = state(Phase::Working, 1_300);
        let verdict = started(1_000, &killed, false, None, 5_000).verdict;
        assert_eq!(verdict.phase, Phase::Stopped);
        assert_eq!(verdict.evidence, Evidence::Gone);
        assert_eq!(verdict.age, 300, "and it does not tick after it");

        // An end before the start is a hand-edited record; a run of no length
        // is the only honest answer.
        assert_eq!(started(9_000, &unstamped, true, None, 9_100).verdict.age, 0);
    }

    #[test]
    fn reader_tells_a_pane_amx_let_go_from_one_that_died() {
        // amx closed the pane of an agent idle for an hour. It is still idle,
        // and the next enter, attach or resume gives it a pane.
        let mut parked = state(Phase::Idle, 1_000);
        parked.parked_at = 4_600;
        let verdict = started(900, &parked, false, None, 4_700).verdict;
        assert_eq!(verdict.phase, Phase::Idle);
        assert_eq!(verdict.evidence, Evidence::LetGo);

        // Without the stamp, the pane went by itself (killed, or its server
        // died), so the run is stopped.
        let died = state(Phase::Idle, 1_000);
        let verdict = started(900, &died, false, None, 4_700).verdict;
        assert_eq!(verdict.phase, Phase::Stopped);
        assert_eq!(verdict.evidence, Evidence::Gone);

        // And what `--json` calls it, which callers branch on.
        assert_eq!(
            serde_json::to_value(Evidence::LetGo).unwrap(),
            "parked",
            "the word every surface prints"
        );
    }

    #[test]
    fn reader_says_how_long_a_waiting_agent_has_waited() {
        // It stopped on a question at 1_000. The vendor's notification about
        // the box lands six seconds later and must not restart the wait.
        let mut asked = state(Phase::Waiting, 1_000);
        asked.last_event = 1_006;
        let verdict = started(900, &asked, true, Some(A_BLOCKING_SCREEN), 1_300).verdict;
        assert_eq!(verdict.phase, Phase::Waiting);
        assert_eq!(verdict.age, 300, "since it stopped, not since it spoke");

        // A question read off the screen does not say when it went up, and
        // the record is still mid-turn, so the age is time since last heard.
        let mut mid_turn = state(Phase::Working, 1_000);
        mid_turn.last_event = 1_100;
        let verdict = started(900, &mid_turn, true, Some(A_BLOCKING_SCREEN), 1_300).verdict;
        assert_eq!(verdict.phase, Phase::Waiting);
        assert_eq!(verdict.evidence, Evidence::Screen);
        assert_eq!(verdict.age, 200);
    }

    #[test]
    fn reader_ages_from_whichever_is_later() {
        // A record written before its first event has a `since` and no
        // `last_event`; the agent is not an hour stale.
        let mut fresh = state(Phase::Starting, 0);
        fresh.since = 1_000;
        assert_eq!(decided(&fresh, true, None, 1_002).age, 2);
    }

    #[test]
    fn reader_has_a_kind_for_every_screen_that_blocks() {
        use crate::store::Kind;

        // Every blocking rule, in every document, needs a kind: what may be
        // sent back depends on it.
        let second = second_vendors_screens();
        for screens in [rules::of("claude"), &second] {
            for rule in screens.rules() {
                let kind = asked_kind(screens, Some(&rule.name));
                assert_eq!(
                    kind.is_some(),
                    rule.state == Phase::Waiting,
                    "{} claims a {} screen",
                    rule.name,
                    rule.state
                );
            }
        }

        let claude = rules::of("claude");
        assert_eq!(asked_kind(claude, Some("folder_trust")), Some(Kind::Trust));
        assert_eq!(asked_kind(claude, Some("ask_menu")), Some(Kind::Question));
        assert_eq!(asked_kind(claude, None), None);
        assert_eq!(
            asked_kind(claude, Some("a rule from a ruleset amx has not met")),
            None
        );
    }

    #[test]
    fn reader_takes_what_a_screen_wants_back_from_the_document_that_named_it() {
        use crate::store::Kind;

        // The rule that names the screen also says what it wants back. A Rust
        // match on claude's rule names would give every other vendor's screens
        // no kind at all.
        let second = second_vendors_screens();
        assert_eq!(asked_kind(&second, Some("choice")), Some(Kind::Question));
        assert_eq!(
            asked_kind(&second, Some("permission_prompt")),
            None,
            "that screen is not on this vendor's pane"
        );
    }

    /// The screens of the second vendor, which shares nothing with claude.
    fn second_vendors_screens() -> Ruleset {
        let screens = crate::vendor::second::SECOND
            .screens
            .expect("the second vendor draws screens of its own");
        Ruleset::parse(screens).expect("and they parse")
    }

    #[test]
    fn reader_says_the_kind_the_record_holds_over_the_kind_it_read() {
        use crate::store::Kind;

        let claimed = |rule: &str| verdict(Phase::Waiting, Evidence::Screen, Some(rule));

        // Nothing on the record, so the screen decides. No hook can report the
        // folder-trust screen: it stands in front of the session.
        let read = View {
            meta: meta(),
            state: State::default(),
            verdict: claimed("folder_trust"),
            doing: None,
        };
        assert_eq!(read.kind(), Some(Kind::Trust));
        assert_eq!(read.json()["kind"], "trust");

        // A hook said so, and a hook is the vendor's own account.
        let told = View {
            meta: meta(),
            state: State {
                kind: Some(Kind::Question),
                ..State::default()
            },
            verdict: claimed("permission_prompt"),
            doing: None,
        };
        assert_eq!(told.kind(), Some(Kind::Question));
    }

    /// A call of two questions, the second multi-select, as a hook recorded
    /// it.
    fn a_call_of_two() -> State {
        use crate::store::{Ask, Choice, Kind};

        let choice = |label: &str, description: &str| Choice {
            label: label.to_string(),
            description: Some(description.to_string()),
            preview: None,
        };
        let mut state = State {
            state: Phase::Waiting,
            kind: Some(Kind::Question),
            ..State::default()
        };
        state.asks_all(vec![
            Ask {
                header: Some("Runtime".to_string()),
                text: "Which runtime should the service target?".to_string(),
                options: vec![
                    choice("Node", "Widest library support"),
                    choice("Deno", "Batteries included"),
                ],
                multi: false,
                answer: None,
            },
            Ask {
                header: Some("Rollout".to_string()),
                text: "Which rollout steps should run?".to_string(),
                options: vec![
                    choice("Canary", "Five percent first"),
                    choice("Announce", "Post to the channel"),
                ],
                multi: true,
                answer: None,
            },
        ]);
        state
    }

    #[test]
    fn reader_hands_a_caller_every_question_of_the_call() {
        let waiting = View::new(
            meta(),
            a_call_of_two(),
            verdict(Phase::Waiting, Evidence::Hooks, None),
        );
        let json = waiting.json();

        // The existing fields mean what they did: the showing question and its
        // choices.
        assert_eq!(json["question"], "Which runtime should the service target?");
        assert_eq!(json["options"][0], "Node");
        assert_eq!(json["options"][1], "Deno");
        assert_eq!(json["kind"], "question");

        // Plus what no screen carries.
        assert_eq!(json["multi"], false, "the one showing takes one choice");
        assert_eq!(json["questions"].as_array().unwrap().len(), 2);
        assert_eq!(json["questions"][0]["header"], "Runtime");
        assert_eq!(
            json["questions"][0]["options"][0]["description"],
            "Widest library support"
        );
        assert_eq!(json["questions"][1]["multi"], true);
        assert_eq!(json["questions"][0]["answer"], serde_json::Value::Null);
    }

    #[test]
    fn reader_says_which_question_of_a_call_is_the_one_showing() {
        // A caller answering a call of several finds the next question in the
        // same fields.
        let mut state = a_call_of_two();
        state.answered("Node");

        let waiting = View::new(
            meta(),
            state,
            verdict(Phase::Waiting, Evidence::Hooks, None),
        );
        let json = waiting.json();
        assert_eq!(json["question"], "Which rollout steps should run?");
        assert_eq!(json["options"][0], "Canary");
        assert_eq!(json["multi"], true, "and this one takes more than one");
        assert_eq!(json["questions"][0]["answer"], "Node");
    }

    #[test]
    fn reader_hands_a_program_what_the_branch_has_open() {
        use crate::pr::{Pr, Standing};

        // `--json` readers cannot see colour, so the word the colour comes
        // from goes out with the number.
        let view = View::new(
            meta(),
            state(Phase::Done, 1_300),
            verdict(Phase::Done, Evidence::Record, None),
        );
        let open = [
            Pr {
                number: 12,
                standing: Standing::Failing,
            },
            Pr {
                number: 9,
                standing: Standing::Merged,
            },
        ];
        assert_eq!(
            view.json_beside(&open)["pr"],
            serde_json::json!([
                {"number": 12, "standing": "failing"},
                {"number": 9, "standing": "merged"},
            ])
        );

        assert_eq!(
            view.json()["pr"],
            serde_json::json!([]),
            "and an agent amx cut no branch for has nothing to say here, \
             which is an empty list rather than a missing field"
        );
    }

    #[test]
    fn reader_hands_a_program_the_title_the_session_goes_under() {
        // The title the wall shows in place of the id.
        let seen = |title: Option<&str>| {
            View::new(
                meta(),
                State {
                    session_title: title.map(str::to_string),
                    ..state(Phase::Idle, 1_300)
                },
                verdict(Phase::Idle, Evidence::Hooks, None),
            )
        };
        assert_eq!(
            seen(Some("Importer clock drift")).json()["session_title"],
            "Importer clock drift"
        );
        assert_eq!(
            seen(None).json()["session_title"],
            serde_json::Value::Null,
            "and a session that has never been titled says so rather than \
             leaving the field out"
        );
    }

    #[test]
    fn reader_hands_a_program_the_name_somebody_gave_the_agent() {
        // The name a rename gave the row. A program drawing its own rows
        // wants the same name; the id is what both address.
        let seen = |name: Option<&str>| {
            View::new(
                meta(),
                State {
                    name: name.map(str::to_string),
                    ..state(Phase::Idle, 1_300)
                },
                verdict(Phase::Idle, Evidence::Hooks, None),
            )
        };
        assert_eq!(seen(Some("auth")).json()["name"], "auth");
        assert_eq!(
            seen(None).json()["name"],
            serde_json::Value::Null,
            "and an agent nobody has renamed says so rather than leaving the \
             field out"
        );
    }

    #[test]
    fn summary_is_wanted_once_a_turn_has_ended_with_something_to_boil_down() {
        let ended = |phase, result: Option<&str>, summary: Option<&str>| State {
            state: phase,
            result: result.map(str::to_string),
            summary: summary.map(str::to_string),
            ..State::default()
        };

        let answered = "Fixed the redirect and ran the suite.\n\nThe test was …";
        assert!(wants_a_line(&ended(Phase::Idle, Some(answered), None)));
        assert!(
            wants_a_line(&ended(Phase::Done, Some(answered), None)),
            "a run that has ended still said something worth a line"
        );

        // Mid-turn the row shows what the agent is doing, so nothing is
        // summarised.
        assert!(!wants_a_line(&ended(Phase::Working, Some(answered), None)));
        assert!(!wants_a_line(&ended(Phase::Waiting, Some(answered), None)));
        // A turn that ended without an answer, and a line already written.
        assert!(!wants_a_line(&ended(Phase::Idle, None, None)));
        assert!(!wants_a_line(&ended(Phase::Idle, Some("  \n "), None)));
        assert!(!wants_a_line(&ended(
            Phase::Idle,
            Some(answered),
            Some("Fixed the redirect")
        )));
    }

    #[test]
    fn summary_command_is_handed_the_answer_and_read_for_one_line() {
        let at = TempDir::new().unwrap();
        let said = "fixed the redirect\nand ran the suite\n";

        // The answer goes in on stdin whole; the first non-blank line comes
        // back.
        assert_eq!(
            ask_for_a_line("tr a-z A-Z", at.path(), "fix-login-a1b", said).as_deref(),
            Some("FIXED THE REDIRECT")
        );
        assert_eq!(
            ask_for_a_line("printf '\\n   \\nsecond thoughts\\n'", at.path(), "x", said).as_deref(),
            Some("second thoughts")
        );

        // It runs where the agent ran and is told which agent it is about.
        assert_eq!(
            ask_for_a_line("pwd", at.path(), "fix-login-a1b", said).as_deref(),
            Some(std::fs::canonicalize(at.path()).unwrap().to_str().unwrap())
        );
        assert_eq!(
            ask_for_a_line(
                "printf '%s\\n' \"$AMX_ID\"",
                at.path(),
                "fix-login-a1b",
                said
            )
            .as_deref(),
            Some("fix-login-a1b")
        );

        // And told it is not the agent: the usual command is `claude -p`,
        // which would otherwise report through the agent's hooks under its id.
        assert_eq!(
            ask_for_a_line(
                "printf '%s\\n' \"$AMX_NESTED\"",
                at.path(),
                "fix-login-a1b",
                said
            )
            .as_deref(),
            Some("1")
        );

        // A command that fails or prints nothing gives no line; the row keeps
        // its answer.
        assert_eq!(ask_for_a_line("exit 3", at.path(), "x", said), None);
        assert_eq!(ask_for_a_line("true", at.path(), "x", said), None);
        assert_eq!(
            ask_for_a_line("no-such-command-here", at.path(), "x", said),
            None
        );
    }

    #[test]
    fn summary_command_takes_an_answer_longer_than_a_pipe_holds() {
        // A megabyte is an ordinary long turn and far more than a pipe holds.
        // With the write and the read on one thread, a command that echoes its
        // input (`tr a-z A-Z`) fills its stdout pipe and stops reading while
        // amx is still writing: a deadlock.
        let at = TempDir::new().unwrap();
        let answer = format!("fixed the redirect\n{}\n", "y".repeat(1024 * 1024));

        // On its own thread, so a deadlock fails the test instead of hanging
        // the suite.
        let (said, heard) = std::sync::mpsc::channel();
        let ran_in = at.path().to_path_buf();
        std::thread::spawn(move || {
            let _ = said.send(ask_for_a_line("cat", &ran_in, "fix-login-a1b", &answer));
        });
        let line = heard
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the command to come back");
        assert_eq!(line.as_deref(), Some("fixed the redirect"));
    }

    /// Serialises the tests that use the process-wide ask queue, which would
    /// otherwise see each other's asks. Taken through a poisoned lock, since a
    /// test that panicked holding it has already failed.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn the_queue_to_itself() -> std::sync::MutexGuard<'static, ()> {
        ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|held| held.into_inner())
    }

    #[test]
    fn summary_asks_about_one_turn_at_a_time() {
        let _queue = the_queue_to_itself();

        // The command is often a model call, so a view over many finished
        // agents queues them instead of starting one per row.
        assert!(may_ask());
        assert!(!may_ask(), "the next turn waits until this one is answered");

        done_asking();
        assert!(may_ask());
        done_asking();
    }

    #[test]
    fn summary_asks_again_only_about_a_turn_whose_ask_went_with_its_process() {
        let out = |turn, at| {
            Some(Asked {
                turn,
                at,
                over: false,
            })
        };

        assert!(
            worth_asking(None, 100, 1_000, About::TheAnswer),
            "nobody has asked yet"
        );
        assert!(
            !worth_asking(out(100, 1_000), 100, 1_100, About::TheAnswer),
            "an ask that went out a minute ago is still out"
        );
        assert!(
            worth_asking(out(100, 1_000), 100, 1_000 + AGAIN, About::TheAnswer),
            "and one that never came back went with the verb that made it"
        );
        assert!(
            !worth_asking(
                Some(Asked {
                    turn: 100,
                    at: 1_000,
                    over: true
                }),
                100,
                90_000,
                About::TheAnswer
            ),
            "a command that answered nothing has answered"
        );
        assert!(
            worth_asking(out(100, 1_000), 200, 1_100, About::TheAnswer),
            "the turn after it is a question of its own"
        );
    }

    #[test]
    fn summary_claims_a_turn_so_one_amx_asks_about_it_and_not_five() {
        let _queue = the_queue_to_itself();

        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta()).unwrap();
        let writer = agent.writer().unwrap();
        let ended = writer
            .update_state(|state| {
                state.state = Phase::Idle;
                state.result = Some("fixed the redirect".to_string());
            })
            .unwrap();
        drop(writer);

        assert!(claim_the_turn(&agent, ended.since, 1_000, About::TheAnswer));
        assert!(
            !claim_the_turn(&agent, ended.since, 1_001, About::TheAnswer),
            "a caller's next ls is a new process, and the record is the only \
             thing either of them shares"
        );

        // A turn that cannot be asked about again does not hold the queue for
        // the rows after it.
        have_a_line_written(root.path(), &agent, &meta(), &ended, "true", 1_001);
        assert!(may_ask(), "the queue is where it was");
        done_asking();

        // Settled, with a line or without: the question has been put.
        settle_the_ask(&agent, ended.since, 1_002);
        assert!(!claim_the_turn(
            &agent,
            ended.since,
            90_000,
            About::TheAnswer
        ));
        assert!(agent.state().unwrap().summary.is_none());
    }

    #[test]
    fn summary_a_reader_that_is_not_staying_never_claims_a_turn() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta()).unwrap();
        let writer = agent.writer().unwrap();
        let ended = writer
            .update_state(|state| {
                state.state = Phase::Idle;
                state.result = Some("fixed the redirect".to_string());
            })
            .unwrap();
        drop(writer);

        // This thread has not declared it stays, like every verb but the view.
        have_a_line_written(root.path(), &agent, &meta(), &ended, "true", 1_000);

        assert!(
            asked(agent.dir()).is_none(),
            "a reader that will not be here to settle it never claims the turn"
        );
        assert!(agent.state().unwrap().summary.is_none());
    }

    #[test]
    fn summary_a_reader_that_is_staying_claims_a_turn_as_before() {
        let _queue = the_queue_to_itself();
        will_stay_for_the_answer();

        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta()).unwrap();
        let writer = agent.writer().unwrap();
        let ended = writer
            .update_state(|state| {
                state.state = Phase::Idle;
                state.result = Some("fixed the redirect".to_string());
            })
            .unwrap();
        drop(writer);

        have_a_line_written(root.path(), &agent, &meta(), &ended, "true", 1_000);
        // The ask thread settles the claim as soon as the command is done, so
        // wait for it rather than race it.
        the_command_comes_back();

        let asked = asked(agent.dir()).expect("a reader staying for the answer claims the turn");
        assert_eq!(asked.turn, ended.since);
        assert!(asked.over, "and settles it once the command is done");
    }

    /// Waits for the ask thread to settle and free the queue, so the test
    /// reads what the command wrote and the next test finds the queue free.
    fn the_command_comes_back() {
        let waited = std::time::Instant::now();
        loop {
            if may_ask() {
                done_asking();
                return;
            }
            assert!(
                waited.elapsed() < std::time::Duration::from_secs(5),
                "the command never came back"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn summary_line_goes_on_the_record_of_the_turn_it_was_asked_about() {
        let root = TempDir::new().unwrap();
        let at = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta()).unwrap();
        let writer = agent.writer().unwrap();
        let ended = writer
            .update_state(|state| {
                state.state = Phase::Idle;
                state.result = Some("fixed the redirect\nand ran the suite".to_string());
            })
            .unwrap();
        drop(writer);

        let answer = ended.result.clone().unwrap();
        write_the_line(
            root.path(),
            "fix-login-a1b",
            ended.since,
            at.path(),
            "cat",
            &answer,
            About::TheAnswer,
        );

        let written = agent.state().unwrap();
        assert_eq!(written.summary.as_deref(), Some("fixed the redirect"));
        assert_eq!(
            written.last_event, ended.last_event,
            "amx heard nothing from the agent by asking somebody else"
        );
        assert_eq!(written.since, ended.since, "and the turn is the same turn");

        // The record moved on while the command ran: the line is about a turn
        // that is over.
        let writer = agent.writer().unwrap();
        writer
            .update_state(|state| {
                state.state = Phase::Working;
                state.summary = None;
                state.result = None;
            })
            .unwrap();
        drop(writer);
        write_the_line(
            root.path(),
            "fix-login-a1b",
            ended.since,
            at.path(),
            "cat",
            &answer,
            About::TheAnswer,
        );
        assert_eq!(agent.state().unwrap().summary, None);
    }

    #[test]
    fn summary_asks_again_about_a_turn_still_running_every_three_minutes() {
        let answered = |at| {
            Some(Asked {
                turn: 100,
                at,
                over: true,
            })
        };

        assert_eq!(REWRITE, 180, "three minutes");
        // A finished answer is summarised once; a running turn is summarised
        // again as it moves.
        assert!(!worth_asking(
            answered(1_000),
            100,
            90_000,
            About::TheAnswer
        ));
        assert!(!worth_asking(
            answered(1_000),
            100,
            1_000 + REWRITE - 1,
            About::TheTurnSoFar
        ));
        assert!(worth_asking(
            answered(1_000),
            100,
            1_000 + REWRITE,
            About::TheTurnSoFar
        ));

        // A permission box moves `since` while the same work goes on, so the
        // clock paces the rewrite, whatever turn the last ask was about. A
        // finished turn's own line is asked for straight away.
        assert!(!worth_asking(
            answered(1_000),
            200,
            1_000 + REWRITE - 1,
            About::TheTurnSoFar
        ));
        assert!(worth_asking(
            answered(1_000),
            200,
            1_000 + REWRITE,
            About::TheTurnSoFar
        ));
        assert!(worth_asking(answered(1_000), 200, 1_001, About::TheAnswer));

        // An ask still out is still out, whichever question it put.
        let out = Some(Asked {
            turn: 100,
            at: 1_000,
            over: false,
        });
        assert!(!worth_asking(
            out,
            100,
            1_000 + REWRITE,
            About::TheTurnSoFar
        ));
        assert!(worth_asking(out, 100, 1_000 + AGAIN, About::TheTurnSoFar));
    }

    /// A running turn over a transcript holding a sentence and the call after
    /// it.
    fn a_running_turn(root: &TempDir) -> (Meta, Agent, State) {
        let session = root.path().join("session.jsonl");
        std::fs::write(&session, format!("{A_SENTENCE}{A_CALL}")).expect("a transcript");
        let meta = Meta {
            parent: None,
            depth: 0,
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            transcript: Some(session),
            // `where_it_ran` is the agent's directory, which must exist for
            // the command to start in.
            dir: root.path().to_path_buf(),
            ..meta()
        };
        let agent = Agent::create(root.path(), &meta).expect("a record");
        let writer = agent.writer().expect("the lock");
        let running = writer
            .update_state(|state| state.state = Phase::Working)
            .expect("a record");
        drop(writer);
        (meta, agent, running)
    }

    #[test]
    fn summary_rewrites_a_running_turn_out_of_the_whole_transcript() {
        let _queue = the_queue_to_itself();
        will_stay_for_the_answer();

        let root = TempDir::new().unwrap();
        let (meta, agent, running) = a_running_turn(&root);

        // The command gets the turn as `amx logs` prints it, not just the last
        // row, and its line goes on the record while the turn runs.
        have_a_line_rewritten(root.path(), &agent, &meta, &running, "tr '\\n' ' '", 1_000);
        the_command_comes_back();
        assert_eq!(
            agent.state().unwrap().summary.as_deref(),
            Some("The importer keeps its own clock.  › Read src/importer.rs")
        );

        let asked = asked(agent.dir()).expect("the ask");
        assert_eq!(asked.turn, running.since, "about the turn under way");
        assert!(asked.over, "and the ask is over");

        // Asked again once `REWRITE` has passed, and not before.
        let again = "printf 'porting the importer\\n'";
        have_a_line_rewritten(
            root.path(),
            &agent,
            &meta,
            &running,
            again,
            asked.at + REWRITE - 1,
        );
        assert!(
            agent.state().unwrap().summary.as_deref() != Some("porting the importer"),
            "a row is drawn every second and the command is not run every second"
        );

        have_a_line_rewritten(
            root.path(),
            &agent,
            &meta,
            &running,
            again,
            asked.at + REWRITE,
        );
        the_command_comes_back();
        assert_eq!(
            agent.state().unwrap().summary.as_deref(),
            Some("porting the importer")
        );
    }

    #[test]
    fn summary_a_reader_that_is_not_staying_never_rewrites_a_turn() {
        let root = TempDir::new().unwrap();
        let (meta, agent, running) = a_running_turn(&root);

        // This thread has not declared it stays, like every verb but the view.
        have_a_line_rewritten(
            root.path(),
            &agent,
            &meta,
            &running,
            "printf 'porting the importer\\n'",
            1_000,
        );

        assert!(
            asked(agent.dir()).is_none(),
            "a reader that will not be here to settle it never claims the turn"
        );
        assert!(agent.state().unwrap().summary.is_none());
    }

    #[test]
    fn summary_a_project_that_names_no_command_is_asked_nothing_at_all() {
        let _queue = the_queue_to_itself();
        will_stay_for_the_answer();

        let root = TempDir::new().unwrap();
        let (meta, agent, running) = a_running_turn(&root);
        assert!(
            summary_command(&meta).is_none(),
            "this record's project names no command"
        );

        have_a_line_where_one_is_wanted(root.path(), &agent, &meta, &running, 1_000);
        assert!(asked(agent.dir()).is_none(), "so nothing was asked");
        assert!(agent.state().unwrap().summary.is_none());
    }

    #[test]
    fn summary_a_rewrite_lands_only_on_the_turn_it_was_asked_about() {
        let root = TempDir::new().unwrap();
        let (meta, agent, running) = a_running_turn(&root);
        let so_far = the_turn_so_far(&meta).expect("the turn so far");

        write_the_line(
            root.path(),
            &meta.id,
            running.since,
            root.path(),
            "printf 'porting the importer\\n'",
            &so_far,
            About::TheTurnSoFar,
        );
        assert_eq!(
            agent.state().unwrap().summary.as_deref(),
            Some("porting the importer")
        );

        // The turn ended while the command ran. `since` moved with the phase,
        // so the line is about a turn that is over and must not land.
        let writer = agent.writer().unwrap();
        let ended = writer
            .observe(|state| {
                state.state = Phase::Idle;
                // Set by hand so the two turns are one second apart.
                state.since = running.since + 1;
                state.summary = None;
                state.result = Some("the importer keeps the clock now".to_string());
            })
            .unwrap();
        drop(writer);
        assert_ne!(ended.since, running.since);

        write_the_line(
            root.path(),
            &meta.id,
            running.since,
            root.path(),
            "printf 'porting the importer\\n'",
            &so_far,
            About::TheTurnSoFar,
        );
        assert_eq!(agent.state().unwrap().summary, None);
    }

    #[test]
    fn reader_drops_the_line_about_a_turn_it_watched_end() {
        // A rewrite of the running turn is stale once the turn ends, and a
        // finished turn shows its answer. Hooks clear it on vendors that have
        // them; this is the vendor without.
        let root = TempDir::new().unwrap();
        let agent = an_agent(&root);
        let writer = agent.writer().unwrap();
        let mut running = writer
            .update_state(|state| {
                state.state = Phase::Working;
                state.summary = Some("Porting the importer's clock.".to_string());
            })
            .unwrap();
        drop(writer);

        write_the_reading(
            &agent,
            &mut running,
            &verdict(Phase::Idle, Evidence::Screen, Some("prompt")),
            Some("the importer keeps the clock now"),
        );

        assert_eq!(running.summary, None);
        assert_eq!(agent.state().unwrap().summary, None);
        assert!(
            wants_a_line(&running),
            "so the finished turn is asked about, as a turn with no rewrite on it is"
        );
    }

    /// A claude agent's record on disk, at `state`.
    fn a_claude_record(root: &TempDir, state: &State) -> (Agent, Meta) {
        let meta = Meta {
            agent: Some("claude".to_string()),
            ..meta()
        };
        let agent = Agent::create(root.path(), &meta).expect("a record");
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_vec(state).expect("a record"),
        )
        .expect("a record");
        (agent, meta)
    }

    fn nothing_to_run() -> crate::config::Config {
        crate::config::Config {
            park_after: 0,
            ..crate::config::Config::default()
        }
    }

    #[test]
    fn a_hooked_turn_cut_by_hand_is_written_idle() {
        // claude sends nothing when esc ends a turn in its pane, so the record
        // says working until the reader that sees the prompt writes otherwise.
        let root = TempDir::new().unwrap();
        let mut running = state(Phase::Working, 1_000);
        running.summary = Some("Running Bash".to_string());
        let (agent, meta) = a_claude_record(&root, &running);

        let reading = read(
            &running,
            0,
            true,
            || Some(AN_INTERRUPTED_SCREEN.to_string()),
            rules::of("claude"),
            true,
            1_001,
            1,
            None,
        );
        assert!(reading.settled, "a prompt cut by hand with no shell on it");
        hear_what_went_unsaid(
            root.path(),
            &agent,
            &meta,
            &mut running,
            &reading,
            &nothing_to_run(),
        );

        let written = agent.state().unwrap();
        assert_eq!(written.state, Phase::Idle);
        assert_eq!(written.summary, None);
        assert_eq!(written.question, None);
        assert_eq!(running, written, "the reading goes on with what it wrote");
        let kinds: Vec<String> = agent
            .events()
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, [READ_TURN_END]);

        // Written once: the next look finds the phase already moved.
        let again = read(
            &written,
            0,
            true,
            || Some(AN_INTERRUPTED_SCREEN.to_string()),
            rules::of("claude"),
            true,
            written.last_event + 60,
            1,
            None,
        );
        let mut written = written;
        hear_what_went_unsaid(
            root.path(),
            &agent,
            &meta,
            &mut written,
            &again,
            &nothing_to_run(),
        );
        assert_eq!(agent.events().unwrap().len(), 1);

        // A prompt with a shell still running behind it is not the turn's end.
        let running = state(Phase::Working, 1_000);
        let shells = read(
            &running,
            0,
            true,
            || Some(A_SCREEN_WITH_A_SHELL.to_string()),
            rules::of("claude"),
            true,
            1_100,
            SETTLED_LOOKS,
            None,
        );
        assert!(!shells.settled);
    }

    #[test]
    fn a_turn_cut_mid_tool_books_work_up_to_the_last_working_look() {
        // The last hook fires as the tool starts and esc cuts the turn half a
        // minute later. The turn's work runs to the last look that saw it
        // working.
        let cut = |looks: &[u64]| {
            let root = TempDir::new().unwrap();
            let mut running = state(Phase::Working, 1_003);
            running.since = 1_000;
            let (agent, meta) = a_claude_record(&root, &running);
            for &now in looks {
                let look = read(
                    &running,
                    0,
                    true,
                    || Some(A_WORKING_SCREEN.to_string()),
                    rules::of("claude"),
                    true,
                    now,
                    1,
                    None,
                );
                saw_it_working(&agent, &meta, &look, now);
                hear_what_went_unsaid(
                    root.path(),
                    &agent,
                    &meta,
                    &mut running,
                    &look,
                    &nothing_to_run(),
                );
            }
            let prompt = read(
                &running,
                0,
                true,
                || Some(AN_INTERRUPTED_SCREEN.to_string()),
                rules::of("claude"),
                true,
                1_045,
                1,
                None,
            );
            saw_it_working(&agent, &meta, &prompt, 1_045);
            hear_what_went_unsaid(
                root.path(),
                &agent,
                &meta,
                &mut running,
                &prompt,
                &nothing_to_run(),
            );
            (root, agent)
        };

        let (_root, agent) = cut(&(1_010..=1_044).collect::<Vec<_>>());
        let written = agent.state().unwrap();
        assert_eq!(written.state, Phase::Idle);
        assert_eq!(written.worked, 44, "up to the last look that saw work");
        assert_eq!(agent.seen(), Some(1_044), "the prompt is no sign of work");

        // No look saw work since the hook: the span closes at the hook.
        let (_root, agent) = cut(&[]);
        let written = agent.state().unwrap();
        assert_eq!(written.state, Phase::Idle);
        assert_eq!(written.worked, 3);
        assert_eq!(agent.seen(), None);

        // A look that saw work does not delay the next one: the reader weighs
        // the vendor's beat, not its own stamp.
        let root = TempDir::new().unwrap();
        let (agent, _) = a_claude_record(&root, &state(Phase::Working, 1_003));
        agent.saw_working(1_044).unwrap();
        assert!(wants_the_screen(
            rules::of("claude"),
            &state(Phase::Working, 1_003),
            false,
            true,
            1_045,
            agent.heartbeat(),
        ));
    }

    #[test]
    fn a_box_answered_by_hand_is_written_working() {
        // A permission box answered in the pane sends nothing either: the tool
        // runs and the record still says waiting.
        let root = TempDir::new().unwrap();
        let mut waiting = state(Phase::Waiting, 1_000);
        waiting.asks(Some("Do you want to proceed?".to_string()));
        let (agent, meta) = a_claude_record(&root, &waiting);

        let reading = read(
            &waiting,
            0,
            true,
            || Some(A_WORKING_SCREEN.to_string()),
            rules::of("claude"),
            true,
            1_100,
            1,
            None,
        );
        assert_eq!(reading.verdict.phase, Phase::Working);
        assert_eq!(reading.verdict.evidence, Evidence::Screen);
        hear_what_went_unsaid(
            root.path(),
            &agent,
            &meta,
            &mut waiting,
            &reading,
            &nothing_to_run(),
        );

        let written = agent.state().unwrap();
        assert_eq!(written.state, Phase::Working);
        assert_eq!(written.question, None);
        assert!(
            agent.events().unwrap().is_empty(),
            "back on the turn it was on, which is no edge"
        );
    }

    #[test]
    fn reader_coherence_a_call_that_is_over_goes_with_its_question() {
        // The questions behind the one showing are answered too, and a working
        // agent is not being asked any of them.
        let working = View::new(
            meta(),
            a_call_of_two(),
            verdict(Phase::Working, Evidence::Screen, Some("thinking")),
        );
        assert_eq!(working.json()["question"], serde_json::Value::Null);
        assert_eq!(working.json()["questions"], serde_json::json!([]));
        assert_eq!(working.json()["multi"], false);
        assert!(working.state.asking.is_empty());
    }

    #[test]
    fn reader_coherence_gives_one_account_of_an_agent_that_has_finished() {
        use crate::store::Kind;

        // A record from an older amx: done, with its last answer, and the
        // vendor's idle nudge still on it as the question. The reader must
        // still give one consistent answer.
        let answered = State {
            state: Phase::Done,
            exit: Some(0),
            question: Some("Claude is waiting for your input".to_string()),
            options: vec!["Yes".to_string()],
            kind: Some(Kind::Permission),
            result: Some("Three that made me stop and re-read:".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };

        let view = View::new(
            meta(),
            answered,
            verdict(Phase::Done, Evidence::Record, None),
        );
        assert_eq!(view.phase(), Phase::Done);
        assert_eq!(view.line(), Some("Three that made me stop and re-read:"));
        assert_eq!(view.state.question, None);
        assert!(view.state.options.is_empty());
        assert_eq!(view.kind(), None);
        assert_eq!(view.json()["question"], serde_json::Value::Null);
        assert_eq!(
            view.json()["result"],
            "Three that made me stop and re-read:"
        );
    }

    #[test]
    fn reader_a_row_takes_the_last_thing_a_screen_said_and_not_the_first() {
        // A result read off a pane is the whole cut screen: this turn's tool
        // calls and leftovers of the turn before, with the answer at the
        // bottom. Columns showing one line must get the answer, not an old
        // shell command above it.
        let read_off_a_screen = |screen: &str| State {
            state: Phase::Idle,
            result: Some(screen.to_string()),
            source: Some(Source::Screen),
            ..State::default()
        };

        let view = View::new(
            meta(),
            read_off_a_screen(A_PI_TURN),
            verdict(Phase::Idle, Evidence::Screen, Some("prompt")),
        );
        assert_eq!(
            view.line(),
            Some(" The file describes recent changes to caching and timeout configuration."),
        );
        assert_eq!(
            view.state.result.as_deref(),
            Some(A_PI_TURN),
            "the record still holds the screen, which is what result, logs and \
             the card print"
        );

        // At 40 columns pi wraps the answer over two rows; the line starts at
        // the opening of the answer.
        let wrapped = View::new(
            meta(),
            read_off_a_screen(A_PI_TURN_AT_40),
            verdict(Phase::Idle, Evidence::Screen, Some("prompt")),
        );
        assert_eq!(
            wrapped.line(),
            Some(" The file describes recent changes to\n caching and timeout configuration."),
        );

        // An answer the vendor reported in its own words is handed over whole.
        let reported = State {
            state: Phase::Done,
            result: Some("Three that made me stop and re-read:\n\nThe cache.".to_string()),
            source: Some(Source::Payload),
            ..State::default()
        };
        let reported = View::new(
            meta(),
            reported,
            verdict(Phase::Done, Evidence::Record, None),
        );
        assert_eq!(
            reported.line(),
            Some("Three that made me stop and re-read:\n\nThe cache.")
        );
    }

    #[test]
    fn reader_coherence_keeps_the_question_that_is_still_somebody_to_answer() {
        use crate::store::Kind;

        let asked = State {
            state: Phase::Waiting,
            question: Some("Do you want to proceed?".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            ..State::default()
        };

        let waiting = View::new(
            meta(),
            asked.clone(),
            verdict(Phase::Waiting, Evidence::Hooks, None),
        );
        assert_eq!(waiting.line(), Some("Do you want to proceed?"));
        assert_eq!(waiting.state.options, ["Yes", "No"]);
        assert_eq!(waiting.kind(), Some(Kind::Permission));

        // A reader that cannot tell what the agent is doing cannot tell the
        // question was answered either, so the question stays.
        let unreadable = View::new(
            meta(),
            asked.clone(),
            verdict(Phase::Unknown, Evidence::Unknown, None),
        );
        assert_eq!(unreadable.line(), Some("Do you want to proceed?"));
        assert_eq!(unreadable.kind(), Some(Kind::Permission));

        // Back at work: the question was answered on the pane.
        let working = State {
            summary: Some("Running Bash".to_string()),
            ..asked
        };
        let working = View::new(
            meta(),
            working,
            verdict(Phase::Working, Evidence::Screen, Some("thinking")),
        );
        assert_eq!(working.line(), Some("Running Bash"));
        assert_eq!(working.state.question, None);
    }
}
