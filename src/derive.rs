//! What an agent is doing, worked out at the moment somebody asks.
//!
//! Nothing amx runs keeps this up to date, because nothing amx runs stays
//! resident. A reader weighs what it has, in this order:
//!
//! 1. **The record ended it.** An exit code was written, or `stop` was. That
//!    is not a guess and nothing overrules it.
//! 2. **The pane is gone.** No pane, no agent: it is stopped, whatever the
//!    last hook said.
//! 3. **The hooks are fresh.** Inside [`FRESH`] seconds the vendor's own
//!    events are the best account there is — of what the agent is doing. They
//!    can say it has stopped on a question without saying which, and that part
//!    is on the pane and nowhere else, so it is read from there at once rather
//!    than waited for.
//! 4. **The screen, against the rules.** Older than that, the pane is captured
//!    and matched. A rule that claims it decides.
//! 5. **Neither.** The screen is claimed by nothing, so the answer is
//!    `unknown` — with how long it has been since anything was heard, because
//!    "I can't tell" is only useful with that beside it.
//!
//! A reader concludes and forgets, with one exception. When the screen it read
//! was a screen asking a question, the question and the choices under it go on
//! the record: they are the one thing on a pane that somebody has to act on
//! rather than merely read, and the pane is the only place the choices are
//! ever written. Nothing a hook reported is corrected by them.
//!
//! A screen with a turn running on it carries one more thing worth handing on
//! and nothing worth recording: the vendor's own spinner line, which says what
//! it is doing at the moment it was looked at. It goes on the reading and
//! never on the record — see [`doing`].
//!
//! And one thing a reader asks for rather than reads. What a turn leaves
//! behind is an answer, not a line about one, so a row of an agent that has
//! finished shows the answer's first sentence. Where somebody has configured a
//! `summary_command`, the first reader to see the turn end sets it going, and
//! the line it writes is on the record for every reader after — see
//! [`wants_a_line`]. Nothing configured is nothing run.
//!
//! Whatever it concludes, it concludes once. Every reader of an agent — `ls`,
//! `status`, the view, `--json` — is handed one [`View`], and what is on that
//! view agrees with the phase on it. A record can disagree with itself; the
//! answer a reader gives from it may not.
//!
//! Beside the phase goes one number, and it answers whichever question of the
//! clock the phase makes worth asking: how long a finished run took, how long
//! a waiting agent has waited, how long since anything was heard from one
//! still going — see [`clock`].

use anyhow::Result;
use serde::Serialize;
use std::io::Write;
use std::path::Path;

use crate::rules::{Claim, Ruleset};
use crate::store::{Agent, Meta, Phase, Question, Source, State};
use crate::tmux::Server;

/// How long the vendor's own events are taken at their word.
///
/// Long enough to cover the quiet inside a turn between two tool calls, short
/// enough that an agent somebody interrupted stops reading as working while
/// they watch it.
pub const FRESH: u64 = 8;

/// Which signal the answer came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Evidence {
    /// The record says how it ended.
    Record,
    /// There is no pane any more.
    Gone,
    /// The vendor's own events, recently enough to trust.
    Hooks,
    /// The screen, and the rule that claimed it.
    Screen,
    /// Nothing accounts for the screen.
    Unknown,
}

/// What a reader concluded, and what it concluded it from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub phase: Phase,
    pub evidence: Evidence,
    /// The rule that claimed the screen, when one did.
    pub rule: Option<String>,
    /// The seconds a surface puts beside this agent, which is not one question
    /// but three — see [`clock`]. A run that has ended says how long it took, an
    /// agent stopped on a question says how long it has waited, and anything
    /// still going says how long since it was last heard from.
    pub age: u64,
}

/// An agent as a reader sees it: the record, and what amx makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub meta: Meta,
    pub state: State,
    pub verdict: Verdict,
}

impl View {
    /// An agent as one answer rather than two.
    ///
    /// A question belongs to the turn that is waiting for it to be answered.
    /// Once a reader has concluded that the agent is not waiting — it is back
    /// at work, the turn is over, the command has gone — a question still on
    /// the record is about a moment that has passed, and handing it out beside
    /// that conclusion is amx saying two things at once. The record is what
    /// lags: an older amx wrote it, or a nudge arrived after the answer did.
    ///
    /// The exception is `unknown`, which is a reader saying it cannot tell. It
    /// cannot tell that the question was answered either, and the one thing on
    /// a pane that somebody has to act on is the last thing to hide from them.
    ///
    /// The other half of one answer is that a question handed out is one
    /// somebody can act on. A sentence the vendor sent in place of a question
    /// is not, so it goes no further than here — see [`forget_the_placeholder`].
    pub fn new(meta: Meta, mut state: State, verdict: Verdict) -> View {
        if !matches!(verdict.phase, Phase::Waiting | Phase::Unknown) {
            state.asks(None);
        }
        forget_the_placeholder(&mut state);
        View {
            meta,
            state,
            verdict,
        }
    }

    pub fn id(&self) -> &str {
        &self.meta.id
    }

    pub fn phase(&self) -> Phase {
        self.verdict.phase
    }

    /// The one line that says what this agent is up to: what it is waiting to
    /// be told, else what it is doing, else what it answered.
    pub fn line(&self) -> Option<&str> {
        self.state
            .question
            .as_deref()
            .or(self.state.summary.as_deref())
            .or(self.state.result.as_deref())
    }

    /// What kind of thing this agent is being asked, if anything.
    ///
    /// The record's own word for it, else what the rule that claimed the
    /// screen says. The order is the one amx keeps everywhere: a hook is the
    /// vendor's account of its own state and a rule is amx's reading of a
    /// picture of it, so the screen fills what the hooks left empty and
    /// corrects nothing.
    pub fn kind(&self) -> Option<crate::store::Kind> {
        self.state
            .kind
            .or_else(|| asked_kind(self.verdict.rule.as_deref()))
    }

    /// The stable shape `--json` prints. Fields are added, never renamed or
    /// removed: callers branch on these.
    ///
    /// The pull requests come from the same reading the row is labelled from,
    /// which is what the last look wrote down beside the record. A verb that
    /// prints once and exits does not wait for a forge, so a caller that has
    /// never had the view open reads an empty list until something has asked.
    pub fn json(&self) -> serde_json::Value {
        self.json_beside(&crate::pr::of(&self.meta))
    }

    /// The same, over requests already read.
    fn json_beside(&self, prs: &[crate::pr::Pr]) -> serde_json::Value {
        serde_json::json!({
            "id": self.meta.id,
            "state": self.verdict.phase.as_str(),
            "evidence": self.verdict.evidence,
            "rule": self.verdict.rule,
            // The seconds a row shows: how long a finished run took, how long
            // a waiting agent has waited, and how long since anything was
            // heard from one still going. The stamps it is worked out from are
            // all here too, so a caller that wants a different question of the
            // clock has what it needs to ask it.
            "age": self.verdict.age,
            "since": self.state.since,
            "last_event": self.state.last_event,
            "ended": self.state.ended,
            "seq": self.state.seq,
            "summary": self.state.summary,
            "question": self.state.question,
            "options": self.state.options,
            // The question showing above, and the whole of the call it came
            // from here: every question in it with its own choices, the
            // sentences under them and the flag saying how many may be taken.
            // `multi` is the showing one's, because that is the question a
            // caller is about to answer.
            "questions": self.state.asking,
            "multi": self.state.multi(),
            "result": self.state.result,
            "source": self.state.source.map(source_name),
            "exit": self.state.exit,
            "kind": self.kind(),
            // What the agent's branch has open, newest of whatever is still
            // live first. `standing` is the word the row's colour came from,
            // because a program has no colour to read it off.
            "pr": prs,
            "task": self.meta.task,
            "dir": self.meta.dir,
            "worktree": self.meta.worktree,
            "branch": self.meta.branch,
            "base": self.meta.base,
            "pane": self.meta.pane.as_str(),
            "socket": self.meta.socket,
            "session": self.meta.session,
            "created": self.meta.created,
        })
    }
}

/// What kind of thing the screen a rule claimed is asking for.
///
/// The rules say which screen is on the pane; this says what that screen wants
/// back, which is the part anything answering an agent needs. It is by name
/// because the screens are told apart by name everywhere else in amx, and a
/// name amx does not know asks for nothing it can describe.
///
/// The folder-trust screen is here and nowhere else: it stands in front of the
/// session that every hook comes from, so no hook can ever report it, and the
/// pane is the only place it is ever seen.
fn asked_kind(rule: Option<&str>) -> Option<crate::store::Kind> {
    use crate::store::Kind;

    match rule? {
        "permission_prompt" => Some(Kind::Permission),
        // An approval, answered yes or no about something the agent is about
        // to do. That the vendor draws it as a menu does not make it a
        // question with an answer of your own.
        "plan_approval" => Some(Kind::Permission),
        "ask_menu" => Some(Kind::Question),
        "folder_trust" => Some(Kind::Trust),
        _ => None,
    }
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Payload => "payload",
        Source::Transcript => "transcript",
        Source::Screen => "screen",
        Source::Error => "error",
    }
}

/// What a reader made of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub verdict: Verdict,
    /// What the screen says the agent is asking, when the screen was read —
    /// because it answered, or because the record said waiting and could not
    /// say what for. Only ever the screen's: a question a hook reported is on
    /// the record already, and the screen is where the choices under it are.
    pub asking: Option<Question>,
    /// What the screen said the agent was doing, off the line the vendor spins
    /// while a turn runs. Only where a rule read the screen as a turn running,
    /// and never written down: it is about the second it was read in.
    pub doing: Option<String>,
}

/// The sentences the vendor sends about a dialog it will not describe.
///
/// Measured against claude 2.1.240 on 2026-08-24, read out of the binary's own
/// dialog host: six seconds after a dialog goes up it fires a
/// `permission_prompt` notification whose whole message is that dialog's
/// title, and the title it gives every tool dialog is `Claude needs your
/// permission` — no tool, no command, nothing anybody could weigh. A tool
/// permission box has a notifier of its own on the same six-second timer, and
/// that one sends `Claude needs your permission to use <tool>`. Which of the
/// two lands last is the vendor's business, so what is recognised here is a
/// whole sentence and never the start of one: the sentence that names the tool
/// is one a caller can act on.
///
/// The idle nudge is the other, and it is not about a question at all: the
/// vendor sends it about a session with nothing open on it. One that says so
/// in its own payload is turned away where hooks are folded, but an older
/// vendor sends it with no type on it, and records outlive the amx that wrote
/// them.
const PLACEHOLDERS: [&str; 2] = [
    "Claude needs your permission",
    "Claude is waiting for your input",
];

/// Whether a question on the record is one of those, or has no words in it at
/// all. Either way there is nothing in it about what is being asked.
fn placeholder(question: &str) -> bool {
    let question = question.trim();
    question.is_empty() || PLACEHOLDERS.contains(&question)
}

/// Forget a question that says nothing about what is being asked.
///
/// A row carrying one says an agent is waiting, which is what a row carrying
/// nothing says, and it says it in words that read like an answer — so a
/// caller stops looking and a person reads the pane themselves. The choices go
/// with it, the way choices always go where their question goes.
///
/// What kind of thing is being asked stays. The vendor said that much, it is
/// not in the words, and it is what decides what may be sent back.
fn forget_the_placeholder(state: &mut State) {
    if state.question.as_deref().is_some_and(placeholder) {
        state.question = None;
        state.options.clear();
    }
}

/// What the vendor's own spinner line says the agent is doing.
///
/// The record's account of a turn is whatever the last tool call wrote, and a
/// reader is at the screen precisely because that was some time ago. The
/// vendor spins one line above its composer for as long as a turn runs, and
/// what is on it is about the second the pane was captured:
///
///   ✽ Nesting… (15s · still thinking with xhigh effort)
///   · Infusing… (2m 2s · ↓ 6.9k tokens)
///
/// The row is found by what the spinner rule anchors on
/// (`assets/screen-rules.toml`): the ellipsis before the parenthesis and the
/// elapsed seconds before the `·`, both on the one row, because a row carrying
/// half of it is not the line the rule was measured against. The lowest such
/// row, and only inside the floor the rules themselves read, so an agent's own
/// output further up the transcript is not mistaken for the vendor's chrome.
///
/// Read but not recorded. A line that says an agent has been at something for
/// 22 seconds is true for a second, and a record carrying it would have every
/// later reader repeat it as news.
fn doing(capture: &str) -> Option<String> {
    let rows: Vec<&str> = capture.lines().collect();
    let floor = rows.len().saturating_sub(crate::rules::FLOOR_LINES);
    let line = rows[floor..]
        .iter()
        .rev()
        .find(|row| row.contains("… (") && row.contains("s · "))?;
    Some(unglyphed(line))
}

/// The spinner line without the glyph the vendor pulses in front of it.
///
/// The glyph cycles through six shapes — `✻ ✽ ✢ ✶ · *` — and a vendor bump may
/// bring a seventh, so what is dropped is described rather than listed: one
/// character that is not a word, in front of the words. A row that opens with
/// a word is left whole.
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

/// Whether the pane is worth reading for a question the record has not got.
///
/// The vendor's own events say an agent has stopped on a question earlier and
/// surer than any screen does, and they can say it without saying what the
/// question is: `PermissionRequest` fires as the box goes up carrying the tool
/// it is about, so a box with no tool named leaves nothing written down at
/// all. The notification that describes it is six seconds behind and the
/// freshness window is eight, so a reader that waited for the record to go
/// stale would hand back a waiting agent with nothing to answer — which is
/// what `status` and `answer` did on the first ask, every time.
fn wants_the_question(state: &State) -> bool {
    state.state == Phase::Waiting && state.question.as_deref().is_none_or(placeholder)
}

/// When anything was last heard from the agent, as the record has it.
///
/// Whichever of the two stamps is later: a record written before its first
/// event has a `since` and no `last_event`, and the agent is not therefore an
/// hour out of touch.
fn heard(state: &State) -> u64 {
    state.last_event.max(state.since)
}

/// The seconds a surface puts beside an agent.
///
/// One column, three questions, because the answer worth reading changes with
/// what the agent is doing.
///
/// **A run that has ended** is asked how long it took, counting from the
/// moment the agent was started, and that number never moves again: an agent
/// that finished in four minutes finished in four minutes, and a column
/// counting up from there is timing how long the record has sat on a disk. The
/// stamp the ending wrote says when it ended; a record that has none — an
/// older amx wrote it, or the pane went and nothing got to record an exit — is
/// dated from the last thing the agent said, which is the last moment amx can
/// vouch for it running.
///
/// **An agent stopped on a question** is asked how long it has waited, which
/// is how long since it stopped and not how long since the last hook: the
/// vendor sends its own notification about a dialog six seconds after putting
/// it up, and a wait that reset to zero on that would be timing amx's news.
/// Only the record can say when the wait began, so a wait amx concluded off a
/// screen — the record says mid-turn, and the screen has a question on it —
/// falls back to how long since anything was heard. A picture of a box says a
/// question is up and not when it went up.
///
/// **Anything still going** is asked how long since it was last heard from,
/// which is what says whether the rest of the row is worth believing, and it
/// is what the column has always said.
fn clock(phase: Phase, state: &State, created: u64, now: u64) -> u64 {
    let heard = heard(state);
    if phase.is_terminal() {
        let ended = match state.ended {
            0 => heard,
            at => at,
        };
        return ended.saturating_sub(created);
    }
    if phase == Phase::Waiting && state.state == Phase::Waiting && state.since > 0 {
        return now.saturating_sub(state.since);
    }
    now.saturating_sub(heard)
}

/// Work out what an agent is doing.
///
/// `alive` is whether its pane is still there, and `capture` is asked for the
/// screen only when it is going to be read: a fresh record needs no tmux call
/// at all unless it is a record of an agent waiting on a question it cannot
/// name, which is what keeps `ls` cheap with a wall full of agents.
///
/// `created` is when the agent was started, which is where a finished run's
/// length is measured from. It is the one thing here that is not on the state
/// document: how long a run took is a fact about the whole agent.
pub fn read(
    state: &State,
    created: u64,
    alive: bool,
    capture: impl FnOnce() -> Option<String>,
    rules: &Ruleset,
    now: u64,
    looks: usize,
) -> Reading {
    // How stale the record is, which is what decides whether a reader believes
    // it over the pane. What a row shows beside the agent is a different
    // question of the same clock, and `clock` is where that one is answered.
    let quiet = now.saturating_sub(heard(state));
    let told = |phase, evidence, rule: Option<&str>| Reading {
        verdict: Verdict {
            phase,
            evidence,
            rule: rule.map(str::to_string),
            age: clock(phase, state, created, now),
        },
        asking: None,
        doing: None,
    };

    if state.state.is_terminal() {
        return told(state.state, Evidence::Record, None);
    }

    if !alive {
        // The pane went without recording an exit: killed, or its server died.
        return told(Phase::Stopped, Evidence::Gone, None);
    }

    if quiet <= FRESH {
        // A record that says waiting and cannot say what for is half an
        // answer, and the half it is missing is the half somebody has to act
        // on. The hooks still decide the phase — this is the same conclusion
        // with the question beside it, on the first look rather than on the
        // one after the freshness runs out.
        let asking = wants_the_question(state)
            .then(capture)
            .flatten()
            .and_then(|screen| rules.asking(&screen));
        return Reading {
            asking,
            ..told(state.state, Evidence::Hooks, None)
        };
    }

    let Some(screen) = capture() else {
        return told(Phase::Unknown, Evidence::Unknown, None);
    };

    match rules.claim(&screen, state.state, looks) {
        Claim::Ruled(rule) => Reading {
            verdict: Verdict {
                phase: rule.state,
                evidence: Evidence::Screen,
                rule: Some(rule.name.clone()),
                age: clock(rule.state, state, created, now),
            },
            asking: rule.question(&screen),
            // A screen a rule read as a turn running is a screen with the
            // vendor's spinner line on it, and that line is fresher than
            // anything the record can say about the same turn.
            doing: (rule.state == Phase::Working)
                .then(|| doing(&screen))
                .flatten(),
        },
        // A rule claims the screen but may not end a turn that is on the
        // record as running. The record stands, with its age beside it.
        Claim::Unsettled(rule) => told(state.state, Evidence::Hooks, Some(&rule.name)),
        Claim::Unclaimed => told(Phase::Unknown, Evidence::Unknown, None),
    }
}

/// Write down what a screen is asking, when the record has not got it.
///
/// The one thing a reader records rather than works out and forgets. A
/// question is not a conclusion about an agent: it is something a person or a
/// caller has to answer, and the pane is the only place the choices under it
/// are ever written down. `ls` and the view are looking at the screen anyway,
/// and throwing away what they read there would leave every caller to capture
/// the pane and parse it again for itself.
///
/// A placeholder is dropped first, and dropped from the document as well as
/// from the reading. It holds the one field the screen was going to fill, so a
/// record still carrying one has nothing to learn, and every reader after this
/// would find it there and go back to the pane for what this look already had.
///
/// The writer's lock is taken only when there is something new to write, so
/// the promise that readers never wait on writers holds for every look but the
/// one that finds the question.
fn note(agent: &Agent, state: &mut State, asking: &Question) {
    forget_the_placeholder(state);
    if !state.learns_from(asking) {
        return;
    }

    let heard = state.last_event;
    let noted = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            // A hook that arrived while the pane was being read is the
            // vendor's own account of a moment this picture is already behind.
            if current.last_event == heard {
                forget_the_placeholder(current);
                current.learn(asking);
            }
        })
    });

    match noted {
        Ok(current) => *state = current,
        // A record that cannot be written is still a question that can be
        // reported, and the next look will try again.
        Err(_) => state.learn(asking),
    }
}

/// One agent as one answer, with the screen where the screen is fresher.
///
/// What a reader read off the pane about a turn in progress stands in front of
/// what the record says about the same turn, and goes no further than the
/// answer this reader hands back. The record is the vendor's own account and
/// this is a picture of it; the picture wins here because the record is stale
/// by the time anything looks at a pane at all.
fn seen(meta: Meta, mut state: State, reading: Reading) -> View {
    if let Some(doing) = reading.doing {
        state.summary = Some(doing);
    }
    View::new(meta, state, reading.verdict)
}

/// Whether this record is a turn that has ended with something to boil down.
///
/// What a row says about an agent that has finished is the first line of what
/// the agent said, and an answer does not open with a summary of itself: it
/// opens with `Done.` or with the first of five paragraphs. A line about the
/// whole answer is a job for something that can read the whole answer, which
/// is what [`ask_for_a_line`] is for.
///
/// Only where the turn is over, and only once. Mid-turn the row is saying what
/// the agent is doing, which is worth more than a line about a turn that has
/// not happened yet, and a line already written is one nobody pays for twice.
fn wants_a_line(state: &State) -> bool {
    (state.state == Phase::Idle || state.state.is_terminal())
        && state.summary.is_none()
        && state
            .result
            .as_deref()
            .is_some_and(|answer| !answer.trim().is_empty())
}

/// The line the configured command makes of what an agent said.
///
/// Through `sh`, because the key holds a command line and a shell is what a
/// command line is written for. The answer goes in on stdin whole — an answer
/// is arbitrary text and an argv is the one place it could be read as syntax —
/// and the command runs where the agent ran, with `AMX_ID` naming which agent
/// it is about, so a command that wants more than the answer knows where to
/// look for it.
///
/// What comes back is the first line with anything on it. A command that fails,
/// that is not there, or that says nothing leaves the row exactly as it was:
/// this is a line about the answer, and the answer is on the record either way.
fn ask_for_a_line(command: &str, at: &Path, id: &str, answer: &str) -> Option<String> {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(at)
        .env("AMX_ID", id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        // A command that reads a line and leaves is answering the question
        // asked, so a pipe it stopped reading is not a failure.
        let _ = stdin.write_all(answer.as_bytes());
    }
    let said = child.wait_with_output().ok()?;
    if !said.status.success() {
        return None;
    }
    String::from_utf8_lossy(&said.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Ask for a turn's line and write it down.
///
/// Written with the observing hand rather than the recording one: amx heard
/// nothing from the agent by asking somebody else about it, and moving the
/// record's freshness would have the next reader believe this document over
/// the pane it is meant to be checking.
///
/// Against the answer it was asked about, and no other. Whatever wrote the
/// line took its time about it, and a record that has moved on to another turn
/// is not the record this sentence is about. The answer is what says so rather
/// than the clock: two turns of one second are told apart by what they said
/// and not by when they said it.
fn write_the_line(root: &Path, id: &str, at: &Path, command: &str, answer: &str) {
    let Some(line) = ask_for_a_line(command, at, id, answer) else {
        return;
    };
    let Ok(agent) = Agent::open(root, id) else {
        return;
    };
    let _ = agent.writer().and_then(|writer| {
        writer.observe(|current| {
            if current.result.as_deref() == Some(answer) && current.summary.is_none() {
                current.summary = Some(line);
            }
        })
    });
}

/// The turns a line has already been asked about.
///
/// A reading is taken every second and the answer takes as long as it takes,
/// so without this every look would put another command behind the same turn.
/// A turn that was asked about and got nothing back stays in here too: a
/// command that failed once fails the same way a second later, and a wall of
/// finished agents would be a wall of subprocesses for as long as the view is
/// open.
static ASKED: std::sync::Mutex<std::collections::BTreeSet<(String, u64)>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Set that going, with nobody waiting for it.
///
/// The thread is never joined, the way a look at a forge is not
/// (`crate::pr`): a view is open for hours and has the line on its next
/// reading, and a verb that exits first leaves the turn unsummarised, which
/// costs the line and nothing else. A command that never returns costs the
/// thread it is on.
fn have_a_line_written(root: &Path, meta: &Meta, state: &State, command: &str) {
    {
        let Ok(mut asked) = ASKED.lock() else {
            return;
        };
        if !asked.insert((meta.id.clone(), state.since)) {
            return;
        }
    }

    let (root, id) = (root.to_path_buf(), meta.id.clone());
    let at = where_it_ran(meta);
    let command = command.to_string();
    let answer = state.result.clone().unwrap_or_default();
    let _ = std::thread::Builder::new()
        .name("amx-summary".to_string())
        .spawn(move || write_the_line(&root, &id, &at, &command, &answer));
}

/// Where the command runs: the agent's own tree while it is there, else where
/// the agent was started. A tree that has been removed leaves the directory
/// the run was asked for, which is the repository it was cut from.
fn where_it_ran(meta: &Meta) -> std::path::PathBuf {
    match &meta.worktree {
        Some(tree) if tree.is_dir() => tree.clone(),
        _ => meta.dir.clone(),
    }
}

/// Read one agent.
pub fn view(root: &Path, id: &str, rules: &Ruleset, now: u64) -> Result<View> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let mut state = agent.state()?;
    let server = Server::from_socket(meta.socket.clone());

    let alive = state.state.is_terminal() || server.pane_alive(&meta.pane);
    let reading = read(
        &state,
        meta.created,
        alive,
        || server.capture(&meta.pane).ok(),
        rules,
        now,
        1,
    );
    if let Some(asking) = &reading.asking {
        note(&agent, &mut state, asking);
    }
    if let Some(command) = crate::config::current().summary_command.as_deref()
        && wants_a_line(&state)
    {
        have_a_line_written(root, &meta, &state, command);
    }

    Ok(seen(meta, state, reading))
}

/// Read every agent, oldest first.
///
/// One pane list per server rather than one per agent: a wall of ten agents is
/// one tmux call, not ten.
pub fn views(root: &Path, rules: &Ruleset, now: u64) -> Result<Vec<View>> {
    let mut views = Vec::new();
    let mut panes: Vec<(crate::tmux::Socket, Vec<crate::tmux::PaneId>)> = Vec::new();

    for id in crate::store::list(root)? {
        let agent = Agent::open(root, &id)?;
        let Ok(meta) = agent.meta() else { continue };
        let mut state = agent.state()?;
        let server = Server::from_socket(meta.socket.clone());

        let alive = if state.state.is_terminal() {
            true
        } else {
            let listed = match panes.iter().find(|(socket, _)| socket == &meta.socket) {
                Some((_, listed)) => listed,
                None => {
                    let listed = server.panes().unwrap_or_default();
                    panes.push((meta.socket.clone(), listed));
                    &panes.last().expect("just pushed").1
                }
            };
            listed.contains(&meta.pane)
        };

        let reading = read(
            &state,
            meta.created,
            alive,
            || server.capture(&meta.pane).ok(),
            rules,
            now,
            1,
        );
        if let Some(asking) = &reading.asking {
            note(&agent, &mut state, asking);
        }
        if let Some(command) = crate::config::current().summary_command.as_deref()
            && wants_a_line(&state)
        {
            have_a_line_written(root, &meta, &state, command);
        }

        views.push(seen(meta, state, reading));
    }

    views.sort_by_key(|view| (view.meta.created, view.meta.id.clone()));
    Ok(views)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules;
    use tempfile::TempDir;

    const IDLE_SCREEN: &str = "\
✻ Worked for 2m 26s
❯
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    const A_SHELL: &str = "$ ls\nCargo.toml  src\n$\n";

    /// A turn running, as claude 2.1.240 draws it: the agent's own output, the
    /// vendor's spinner line over the composer, and the mode footer that is on
    /// every screen this vendor draws.
    const A_WORKING_SCREEN: &str = "\
● Read(src/main.rs)
  ⎿  Read 210 lines

✢ Forging… (22s · ↓ 1.3k tokens)
❯
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
";

    /// The vendor's own notification about a dialog it will not describe,
    /// whole. It says a question exists and not one word about what it wants.
    const A_PLACEHOLDER: &str = "Claude needs your permission";

    /// A permission box, which is a screen with a question on it.
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
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
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

    fn verdict(phase: Phase, evidence: Evidence, rule: Option<&str>) -> Verdict {
        Verdict {
            phase,
            evidence,
            rule: rule.map(str::to_string),
            age: 30,
        }
    }

    fn reading(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Reading {
        started(0, state, alive, screen, now)
    }

    /// The same reading of an agent started at a stated moment, which is where
    /// a finished run's length is measured from.
    fn started(
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
            rules::bundled(),
            now,
            1,
        )
    }

    fn decided(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Verdict {
        reading(state, alive, screen, now).verdict
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
        // The record says waiting and cannot say what for: the event that put
        // the box up carried no tool name, and the notification that would
        // have described it is six seconds behind. The hooks are as fresh as
        // they ever get, and the pane is still the only place the question and
        // the choices under it are written.
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

        // A pane that cannot be read leaves the conclusion exactly where the
        // hooks left it.
        let unreadable = reading(&state(Phase::Waiting, 1_000), true, None, 1_000);
        assert_eq!(unreadable.verdict.phase, Phase::Waiting);
        assert_eq!(unreadable.verdict.evidence, Evidence::Hooks);
        assert_eq!(unreadable.asking, None);
    }

    #[test]
    fn reader_reads_the_pane_past_a_question_that_names_nothing() {
        // The vendor's dialog host sends the dialog's title and nothing else,
        // so the record ends up holding a sentence that says a question
        // exists. A caller can do as much with that as with an empty field,
        // and the pane is where the rest of it is.
        let mut placeheld = state(Phase::Waiting, 1_000);
        placeheld.question = Some(A_PLACEHOLDER.to_string());
        let asking = reading(&placeheld, true, Some(A_BLOCKING_SCREEN), 1_000)
            .asking
            .expect("a sentence that names nothing leaves the question unasked");
        assert_eq!(asking.text, "Do you want to proceed?");
        assert_eq!(asking.options, ["Yes", "No"]);

        // The sentence that does name the tool is the vendor telling a caller
        // something, and a reader holding one does not go to the pane at all.
        let mut told = state(Phase::Waiting, 1_000);
        told.question = Some(format!("{A_PLACEHOLDER} to use Bash"));
        assert_eq!(
            reading(&told, true, Some(A_BLOCKING_SCREEN), 1_000).asking,
            None,
            "the placeholder is a whole sentence, not the start of one"
        );
    }

    #[test]
    fn reader_never_hands_a_caller_the_placeholder() {
        use crate::store::Kind;

        // Whatever the pane said or failed to say, none of these reaches
        // anybody: a row carrying one says an agent is waiting, which is what
        // a row carrying nothing says, and it says it in words that read like
        // an answer.
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

        // The forgetting comes before the record is asked whether the screen
        // tells it anything. Otherwise the placeholder sits in the one field
        // the screen was going to fill, the record learns nothing, and the
        // next reader finds it there and asks the pane all over again.
        let mut state = State {
            state: Phase::Waiting,
            question: Some(A_PLACEHOLDER.to_string()),
            kind: Some(Kind::Permission),
            ..State::default()
        };
        let seen = Question {
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
        };

        forget_the_placeholder(&mut state);
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
        // A record that already says what is being asked has nothing to learn
        // from the pane, so fresh hooks answer without a capture.
        let mut told = state(Phase::Waiting, 1_000);
        told.question = Some("Do you want to proceed?".to_string());
        let fresh = reading(&told, true, Some(A_BLOCKING_SCREEN), 1_000);
        assert_eq!(fresh.verdict.evidence, Evidence::Hooks);
        assert_eq!(fresh.asking, None);

        // Neither has an agent that is working, whatever is on its screen.
        let mid_turn = reading(
            &state(Phase::Working, 1_000),
            true,
            Some(A_BLOCKING_SCREEN),
            1_000,
        );
        assert_eq!(mid_turn.verdict.phase, Phase::Working);
        assert_eq!(mid_turn.asking, None);

        // And a screen that is not asking anything says nothing about it.
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
        // The hooks have gone quiet mid-turn, so the record's account of what
        // this agent is doing is as old as the silence: the tool it names may
        // have finished a minute ago. The vendor's own line is on the pane and
        // it is about now.
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

        let view = seen(meta(), told, reading);
        assert_eq!(view.line(), Some("Forging… (22s · ↓ 1.3k tokens)"));
        assert_eq!(view.json()["summary"], "Forging… (22s · ↓ 1.3k tokens)");
    }

    #[test]
    fn reader_reads_the_spinner_line_through_whichever_glyph_is_on_it() {
        // The glyph cycles through six shapes and a vendor bump may bring a
        // seventh. What they have in common is that they are one character and
        // not a word, which is the whole of what this leans on.
        for glyph in ["✻", "✽", "✢", "✶", "·", "*"] {
            let screen = format!("{glyph} Smooshing… (7s · thinking with xhigh effort)\n");
            let reading = reading(&state(Phase::Working, 1_000), true, Some(&screen), 1_100);
            assert_eq!(
                reading.doing.as_deref(),
                Some("Smooshing… (7s · thinking with xhigh effort)"),
                "{glyph}"
            );
        }
    }

    #[test]
    fn reader_says_what_an_agent_is_doing_only_where_it_read_it() {
        // Fresh hooks, so no screen is captured at all and the record's own
        // account of the turn stands.
        let fresh = reading(
            &state(Phase::Working, 1_000),
            true,
            Some(A_WORKING_SCREEN),
            1_000,
        );
        assert_eq!(fresh.verdict.evidence, Evidence::Hooks);
        assert_eq!(fresh.doing, None);

        // And a screen with no turn running on it says nothing about one: a
        // question, a prompt nobody is at, and a shell amx cannot account for.
        for screen in [A_BLOCKING_SCREEN, IDLE_SCREEN, A_SHELL] {
            let reading = reading(&state(Phase::Starting, 1_000), true, Some(screen), 1_100);
            assert_eq!(reading.doing, None, "{screen}");
        }

        // What the record says is what the row says, then.
        let mut told = state(Phase::Working, 1_000);
        told.summary = Some("Running Bash".to_string());
        let reading = reading(&told, true, Some(A_SHELL), 1_500);
        assert_eq!(seen(meta(), told, reading).line(), Some("Running Bash"));
    }

    #[test]
    fn reader_takes_the_record_when_the_record_says_how_it_ended() {
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
            // Long stale, no pane, and it does not matter: this is over.
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
    fn reader_reads_the_screen_once_the_hooks_have_gone_quiet() {
        // Nothing outstanding on the record, so the idle rule may decide at
        // once — this is the parked agent that would otherwise sit at
        // `starting` for ever.
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
        // on the record, one look at a still screen decides nothing, and the
        // record stands with its age beside it.
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
    fn reader_says_unknown_rather_than_guessing() {
        let verdict = decided(&state(Phase::Working, 1_000), true, Some(A_SHELL), 1_500);
        assert_eq!(verdict.phase, Phase::Unknown);
        assert_eq!(verdict.evidence, Evidence::Unknown);
        assert_eq!(verdict.age, 500, "and how long it has been out of touch");

        // A pane that cannot be captured is the same answer.
        let unreadable = decided(&state(Phase::Working, 1_000), true, None, 1_500);
        assert_eq!(unreadable.phase, Phase::Unknown);
    }

    #[test]
    fn reader_freezes_the_clock_on_a_run_that_has_ended() {
        // Started at 1_000 and ended at 1_300: a five-minute run, and a run
        // that took five minutes took five minutes whenever anybody asks.
        let mut done = state(Phase::Done, 1_300);
        done.ended = 1_300;

        assert_eq!(started(1_000, &done, true, None, 1_310).verdict.age, 300);
        assert_eq!(
            started(1_000, &done, true, None, 90_000).verdict.age,
            300,
            "a day later it is still the run it was"
        );

        let view = View::new(
            Meta {
                created: 1_000,
                ..meta()
            },
            done.clone(),
            started(1_000, &done, true, None, 90_000).verdict,
        );
        assert_eq!(view.json()["age"], 300);
        assert_eq!(view.json()["ended"], 1_300, "and when it ended, whole");
    }

    #[test]
    fn reader_dates_an_ending_nobody_stamped_from_the_last_thing_it_said() {
        // A record written by an older amx has no stamp on it, and records
        // outlive the amx that wrote them.
        let unstamped = state(Phase::Done, 1_300);
        assert_eq!(unstamped.ended, 0);
        assert_eq!(
            started(1_000, &unstamped, true, None, 5_000).verdict.age,
            300
        );

        // The pane went without recording an exit: the reader ends the run,
        // and the run is as long as the last thing anybody heard.
        let killed = state(Phase::Working, 1_300);
        let verdict = started(1_000, &killed, false, None, 5_000).verdict;
        assert_eq!(verdict.phase, Phase::Stopped);
        assert_eq!(verdict.evidence, Evidence::Gone);
        assert_eq!(verdict.age, 300, "and it does not tick after it");

        // An ending before the agent was started is a record somebody edited,
        // and a run of no length is the only honest answer to it.
        assert_eq!(started(9_000, &unstamped, true, None, 9_100).verdict.age, 0);
    }

    #[test]
    fn reader_says_how_long_a_waiting_agent_has_waited() {
        // It stopped on a question at 1_000. The vendor's own notification
        // about the box lands six seconds later, and a row that started
        // counting there would be timing amx's news rather than the wait.
        let mut asked = state(Phase::Waiting, 1_000);
        asked.last_event = 1_006;
        let verdict = started(900, &asked, true, Some(A_BLOCKING_SCREEN), 1_300).verdict;
        assert_eq!(verdict.phase, Phase::Waiting);
        assert_eq!(verdict.age, 300, "since it stopped, not since it spoke");

        // A screen amx read a question off says a question is up and not when
        // it went up: the record is still mid-turn, so how long since anything
        // was heard is the whole of what amx can say.
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
        // `last_event`; the agent is not therefore an hour stale.
        let mut fresh = state(Phase::Starting, 0);
        fresh.since = 1_000;
        assert_eq!(decided(&fresh, true, None, 1_002).age, 2);
    }

    #[test]
    fn reader_has_a_kind_for_every_screen_that_blocks() {
        use crate::store::Kind;

        // Every rule that stops an agent stops it on something somebody has to
        // answer, and what may be sent back depends on which. A blocking rule
        // added without a kind here would leave a caller guessing again.
        for rule in rules::bundled().rules() {
            let kind = asked_kind(Some(&rule.name));
            assert_eq!(
                kind.is_some(),
                rule.state == Phase::Waiting,
                "{} claims a {} screen",
                rule.name,
                rule.state
            );
        }
        assert_eq!(asked_kind(Some("folder_trust")), Some(Kind::Trust));
        assert_eq!(asked_kind(Some("ask_menu")), Some(Kind::Question));
        assert_eq!(asked_kind(None), None);
        assert_eq!(
            asked_kind(Some("a rule from a ruleset amx has not met")),
            None
        );
    }

    #[test]
    fn reader_says_the_kind_the_record_holds_over_the_kind_it_read() {
        use crate::store::Kind;

        let claimed = |rule: &str| verdict(Phase::Waiting, Evidence::Screen, Some(rule));

        // Nothing on the record: the screen is all there is, and the
        // folder-trust screen is the one kind no hook can ever report, because
        // it stands in front of the session every hook comes from.
        let read = View {
            meta: meta(),
            state: State::default(),
            verdict: claimed("folder_trust"),
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
        };
        assert_eq!(told.kind(), Some(Kind::Question));
    }

    /// A call of two questions, the second taking more than one choice, as a
    /// hook folded it onto the record.
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

        // What was there before means what it always meant: the question on
        // the screen and the choices under it.
        assert_eq!(json["question"], "Which runtime should the service target?");
        assert_eq!(json["options"][0], "Node");
        assert_eq!(json["options"][1], "Deno");
        assert_eq!(json["kind"], "question");

        // And beside them, the part no screen carries.
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
        // A caller answering a call of several is told what to answer next by
        // the same fields it read the first time.
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

        // A program reading `--json` is the one caller that cannot see a
        // colour, so the number goes out beside the word the colour came from
        // rather than beside the colour.
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

        // Mid-turn there is nothing to boil down: what is on the row then is
        // what the agent is doing, and it is different by the next reading.
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

        // The answer arrives on stdin, whole, and what comes back is the first
        // line with anything on it.
        assert_eq!(
            ask_for_a_line("tr a-z A-Z", at.path(), "fix-login-a1b", said).as_deref(),
            Some("FIXED THE REDIRECT")
        );
        assert_eq!(
            ask_for_a_line("printf '\\n   \\nsecond thoughts\\n'", at.path(), "x", said).as_deref(),
            Some("second thoughts")
        );

        // It runs where the agent ran, and is told which agent it is about.
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

        // A command that fails, and one that says nothing, say nothing. The
        // row keeps the answer it had.
        assert_eq!(ask_for_a_line("exit 3", at.path(), "x", said), None);
        assert_eq!(ask_for_a_line("true", at.path(), "x", said), None);
        assert_eq!(
            ask_for_a_line("no-such-command-here", at.path(), "x", said),
            None
        );
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
        write_the_line(root.path(), "fix-login-a1b", at.path(), "cat", &answer);

        let written = agent.state().unwrap();
        assert_eq!(written.summary.as_deref(), Some("fixed the redirect"));
        assert_eq!(
            written.last_event, ended.last_event,
            "amx heard nothing from the agent by asking somebody else"
        );
        assert_eq!(written.since, ended.since, "and the turn is the same turn");

        // The record moved on while the command was still running: this line
        // is about a turn that is over, and the row is about the one it is on.
        let writer = agent.writer().unwrap();
        writer
            .update_state(|state| {
                state.state = Phase::Working;
                state.summary = None;
                state.result = None;
            })
            .unwrap();
        drop(writer);
        write_the_line(root.path(), "fix-login-a1b", at.path(), "cat", &answer);
        assert_eq!(agent.state().unwrap().summary, None);
    }

    #[test]
    fn reader_coherence_a_call_that_is_over_goes_with_its_question() {
        // The questions behind the one showing are as answered as it is, and
        // an agent that is back at work is not being asked any of them.
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

        // The record the agent read-readme-md-and-799 was left with on
        // 2026-08-20: done, with the answer of its last turn, and with the
        // vendor's idle nudge still on it as the question. The hooks no longer
        // write a record like this one, and records outlive the amx that wrote
        // them, so a reader gives one answer whatever it is handed.
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

        // A reader that cannot say what the agent is doing cannot say the
        // question is answered either, and the one thing somebody has to act
        // on is the last thing to hide from them.
        let unreadable = View::new(
            meta(),
            asked.clone(),
            verdict(Phase::Unknown, Evidence::Unknown, None),
        );
        assert_eq!(unreadable.line(), Some("Do you want to proceed?"));
        assert_eq!(unreadable.kind(), Some(Kind::Permission));

        // Back at work: whatever that question was, it was answered on the
        // pane, and it is not what this agent is doing now.
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
