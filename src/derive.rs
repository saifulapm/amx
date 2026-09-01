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
//! `summary_command`, the first reader that is staying long enough to hear it
//! back sets it going, and the line it writes is on the record for every
//! reader after — see [`wants_a_line`] and [`staying`]. Nothing configured is
//! nothing run, and nothing staying is nothing asked.
//!
//! Whatever it concludes, it concludes once. Every reader of an agent — `ls`,
//! `status`, the view, `--json` — is handed one [`View`], and what is on that
//! view agrees with the phase on it. A record can disagree with itself; the
//! answer a reader gives from it may not.
//!
//! Beside the phase goes one number, and it answers whichever question of the
//! clock the phase makes worth asking: how long a finished run worked, how
//! long a waiting agent has waited, how long since anything was heard from one
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
    /// but three — see [`clock`]. A run that has ended says how long it worked,
    /// an agent stopped on a question says how long it has waited, and anything
    /// still going says how long since it was last heard from.
    pub age: u64,
    /// The seconds this agent has worked, live — see [`worked`]. It ticks
    /// while the agent works, stands still while it waits or sits idle, and
    /// stops for good at the end. The rows and the table print this one; the
    /// age above keeps its three questions for the card and for `--json`.
    pub worked: u64,
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
    ///
    /// The vendor's own menu is the exception, and it earns it twice over. It
    /// is the one screen amx can name with certainty — claude's anchors on
    /// `Enter to select`, which no other screen it draws carries — so a rule
    /// claiming it is not a guess about what is on the pane. And a rule only
    /// gets to speak once the hooks have gone quiet, which is to say once the
    /// record is old news: an amx written before the vendor was found asking
    /// itself for permission to draw a menu wrote `permission` over every menu
    /// it saw, and records outlive the amx that wrote them. A kind is what
    /// decides what may be sent back, and a permission box's one key at a menu
    /// is how a caller answers a question nobody chose.
    ///
    /// The exception is the kind rather than the screen, which is what lets it
    /// hold for whichever vendor is being read: the name on the verdict is
    /// looked up in the document that named it.
    pub fn kind(&self) -> Option<crate::store::Kind> {
        match asked_kind(crate::rules::bundled(), self.verdict.rule.as_deref()) {
            seen @ Some(crate::store::Kind::Question) => seen,
            seen => self.state.kind.or(seen),
        }
    }

    /// The stable shape `--json` prints. Fields are added, never renamed or
    /// removed: callers branch on these.
    ///
    /// The pull requests come from the same reading the row is labelled from,
    /// which is what the last look wrote down beside the record — read here and
    /// no more than read. A verb that prints once and exits does not wait for a
    /// forge, and one that will not wait has no business starting a look
    /// nobody will be here to collect: see [`crate::pr::written`]. A caller
    /// that has never had the view open reads an empty list until something
    /// that waits has asked.
    pub fn json(&self) -> serde_json::Value {
        self.json_beside(&crate::pr::written(&self.meta))
    }

    /// The same, over requests already read.
    fn json_beside(&self, prs: &[crate::pr::Pr]) -> serde_json::Value {
        serde_json::json!({
            "id": self.meta.id,
            "state": self.verdict.phase.as_str(),
            "evidence": self.verdict.evidence,
            "rule": self.verdict.rule,
            // The seconds a row shows: how long a finished run worked, how
            // long a waiting agent has waited, and how long since anything was
            // heard from one still going. The stamps it is worked out from are
            // all here too, so a caller that wants a different question of the
            // clock has what it needs to ask it.
            "age": self.verdict.age,
            "since": self.state.since,
            "last_event": self.state.last_event,
            "ended": self.state.ended,
            // The spans of work the record has added up, as it has them: a run
            // still going has everything but the span it is in.
            "worked": self.state.worked,
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
/// The rules say which screen is on the pane; the same rule says what that
/// screen wants back, which is the part anything answering an agent needs. By
/// name, because the name is all a verdict carries — the rule that spoke is
/// looked up again in the document it came out of, and a name that document
/// has never heard of asks for nothing amx can describe.
///
/// The vendor's own menu is the one screen a reader lets stand in front of the
/// record — see [`View::kind`] for why that one and no other.
fn asked_kind(screens: &Ruleset, rule: Option<&str>) -> Option<crate::store::Kind> {
    let name = rule?;
    screens.rules().iter().find(|rule| rule.name == name)?.kind
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

/// Whether a question on the record says nothing about what is being asked:
/// one of the sentences the vendor sends in place of one, or no words at all.
///
/// Which sentences those are is the vendor's own wording, so they are written
/// down where the rest of its screens are — see the `placeholders` key of
/// `assets/screen-rules.toml`. An empty question is nobody's wording and is
/// recognised here.
fn placeholder(question: &str) -> bool {
    let question = question.trim();
    question.is_empty() || crate::rules::bundled().placeholder(question)
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
/// The row is found by the fragments the vendor's own document says its
/// spinner always carries, all of them on the one row, because a row carrying
/// half of it is not the line those were measured against. The lowest such
/// row, and only inside the floor the rules themselves read, so an agent's own
/// output further up the transcript is not mistaken for the vendor's chrome.
///
/// Read but not recorded. A line that says an agent has been at something for
/// 22 seconds is true for a second, and a record carrying it would have every
/// later reader repeat it as news.
fn doing(screens: &Ruleset, capture: &str) -> Option<String> {
    let rows: Vec<&str> = capture.lines().collect();
    let floor = rows.len().saturating_sub(crate::rules::FLOOR_LINES);
    let line = rows[floor..]
        .iter()
        .rev()
        .find(|row| screens.furniture().spinning(row))?;
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

/// Whether concluding about this agent means looking at its pane.
///
/// [`read`] asks for the screen rather than being handed one, so that a
/// reading which needs no screen pays for none. A wall is the other half of
/// that: the screens it does need are worth taking in one call rather than
/// one at a time (see [`Server::captures`]), and a call made before any of the
/// records has been read is a call that cannot know which panes to name. So
/// the same three questions [`read`] asks — is this over, is the pane there,
/// are the hooks fresh enough — are asked here, off the record alone.
///
/// The two have to agree, and a test says so rather than a comment: a reading
/// wanting a screen nobody asked for concludes `unknown` off a capture that
/// was never taken.
fn wants_the_screen(state: &State, alive: bool, now: u64) -> bool {
    if state.state.is_terminal() || !alive {
        return false;
    }
    if now.saturating_sub(heard(state)) <= FRESH {
        return wants_the_question(state);
    }
    true
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
/// **A run that has ended** is asked how long it worked, which is the spans
/// the record added up as the phase moved in and out of working. That number
/// never moves again: an agent that worked four minutes worked four minutes,
/// and a column counting up from there is timing how long the record has sat
/// on a disk. Nor is it the wall clock over the run, which would count the
/// afternoon an agent spent standing at a question nobody answered as an
/// afternoon's work.
///
/// A span nothing closed is closed here. The record adds up at the write that
/// moves the phase, so an agent the pane went out from under is still on the
/// record as working, and its last span runs to the last moment amx can vouch
/// for it running. That is the stamp the ending wrote, or where there is none
/// — an older amx wrote the record, or the pane went and nothing got to record
/// an exit — the last thing the agent said.
///
/// A record with no spans on it at all is one amx cannot answer that question
/// about: written before any of this, or of an agent that never worked. It is
/// asked the old one instead, and says how long the run was alive.
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
    if phase.is_terminal() {
        return worked(phase, state, created, now);
    }
    if phase == Phase::Waiting && state.state == Phase::Waiting && state.since > 0 {
        return now.saturating_sub(state.since);
    }
    now.saturating_sub(heard(state))
}

/// The seconds of work a row puts beside an agent.
///
/// The spans the record has added up, and the one still open while the record
/// says the agent is working — so the number ticks while the agent works and
/// stands still while it waits or sits idle. An idle agent's clock climbing
/// was timing the silence, not the agent.
///
/// At the end it is [`clock`]'s own frozen answer, fallback included: a run
/// that worked four minutes worked four minutes, and a record with no spans on
/// it says how long the run was alive instead.
fn worked(phase: Phase, state: &State, created: u64, now: u64) -> u64 {
    if phase.is_terminal() {
        let ended = match state.ended {
            0 => heard(state),
            at => at,
        };
        return match state.worked_by(ended) {
            0 => ended.saturating_sub(created),
            worked => worked,
        };
    }
    state.worked_by(now)
}

/// The reading's number in words, in the shortest form that says it.
///
/// A surface prints the number [`clock`] worked out, and the units belong with
/// the working out rather than with the printing. A table and a screen that
/// each decide for themselves what `120` means are two surfaces that agree
/// today and disagree after the next hand touches one of them, and a person
/// with both open in front of them is the one who finds out.
pub fn in_words(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Work out what an agent is doing.
///
/// `alive` is whether its pane is still there, and `capture` is asked for the
/// screen only when it is going to be read: a fresh record needs no tmux call
/// at all unless it is a record of an agent waiting on a question it cannot
/// name, which is what keeps `ls` cheap with a wall full of agents.
///
/// `created` is when the agent was started, which is what a finished run with
/// no spans of work on it is measured from. It is the one thing here that is
/// not on the state document: how long a run was alive is a fact about the
/// whole agent.
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
            worked: worked(phase, state, created, now),
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
                worked: worked(rule.state, state, created, now),
            },
            asking: rule.question(&screen),
            // A screen a rule read as a turn running is a screen with the
            // vendor's spinner line on it, and that line is fresher than
            // anything the record can say about the same turn.
            doing: (rule.state == Phase::Working)
                .then(|| doing(rules, &screen))
                .flatten(),
        },
        // A rule claims the screen but may not end a turn that is on the
        // record as running. The record stands, with its age beside it.
        Claim::Unsettled(rule) => told(state.state, Evidence::Hooks, Some(&rule.name)),
        Claim::Unclaimed => told(Phase::Unknown, Evidence::Unknown, None),
    }
}

/// One agent's last look, kept only long enough to compare it to the next.
#[derive(Clone, Copy)]
struct Settled {
    /// The screen the look found, cut to the rows above the vendor's chrome
    /// and hashed: [`still_looks`] only ever asks whether it changed, never
    /// what it said.
    hash: u64,
    /// The phase the record held on that look.
    recorded: Phase,
    /// How many consecutive looks, this one included, found the same hash
    /// under the same recorded phase.
    looks: usize,
}

/// One agent's last look, for every agent this process has read, so the next
/// look at the same one has something to compare against.
static SETTLED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Settled>>> =
    std::sync::OnceLock::new();

/// How many consecutive looks at `id`, this one included, have found the same
/// screen above the vendor's chrome with `recorded` the phase on file.
///
/// The chrome is cut off before anything is compared — see
/// [`Furniture::cut`] — because a statusline that ticks off elapsed time or a
/// spinner's own clock changes every second, and neither is the screen a
/// quiescent rule is waiting to see hold still. What survives the cut is
/// hashed rather than kept whole, so a fleet of long-lived agents costs this
/// process one small number apiece and not their transcripts.
///
/// The count starts over at one — the answer every look gave before anything
/// was counted — the moment either half of what it is a count of stops being
/// true: the screen reads different from the look before it, or the record
/// has moved to a different phase since, which is a hook's news arriving
/// between two looks at a pane that has not visibly changed. Either way, a
/// streak counted before that moment is not a streak about the screen this
/// look is asking a quiescent rule to trust.
fn still_looks(id: &str, screen: Option<&str>, recorded: Phase, rules: &Ruleset) -> usize {
    let Some(screen) = screen else { return 1 };
    let rows: Vec<&str> = screen.lines().collect();
    let hash = hashed(rules.furniture().cut(&rows));

    let map = SETTLED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let Ok(mut settled) = map.lock() else {
        return 1;
    };
    let looks = match settled.get(id) {
        Some(prior) if prior.hash == hash && prior.recorded == recorded => prior.looks + 1,
        _ => 1,
    };
    settled.insert(
        id.to_string(),
        Settled {
            hash,
            recorded,
            looks,
        },
    );
    looks
}

/// A cheap stand-in for a slice of rows too large to keep one of for every
/// agent a long-lived process has read.
fn hashed(rows: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut hasher);
    hasher.finish()
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
/// and the command runs where the agent ran, with [`crate::hook::ID_ENV`]
/// naming which agent it is about, so a command that wants more than the
/// answer knows where to look for it. The same variable a pane is handed, so
/// a command written for one is written for the other.
///
/// What comes back is the first line with anything on it. A command that fails,
/// that is not there, or that says nothing leaves the row exactly as it was:
/// this is a line about the answer, and the answer is on the record either way.
///
/// The answer goes in on a thread of its own. An answer is as long as the turn
/// was and a pipe holds a page or sixteen of it, so whoever writes the answer
/// cannot also be whoever reads what comes back: a command that echoes what it
/// reads fills its own pipe, stops reading, and the two of them wait on each
/// other for good.
fn ask_for_a_line(command: &str, at: &Path, id: &str, answer: &str) -> Option<String> {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(at)
        .env(crate::hook::ID_ENV, id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let feeding = child.stdin.take().map(|mut stdin| {
        let answer = answer.to_string();
        std::thread::spawn(move || {
            // A command that reads a line and leaves is answering the question
            // asked, so a pipe it stopped reading is not a failure. The handle
            // goes at the end of this, which is what tells the command the
            // answer is all of the answer.
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
fn write_the_line(root: &Path, id: &str, turn: u64, at: &Path, command: &str, answer: &str) {
    let line = ask_for_a_line(command, at, id, answer);
    let Ok(agent) = Agent::open(root, id) else {
        return;
    };
    // The ask is over either way, and saying so is what stops the next reader
    // asking the same question of the same command.
    settle_the_ask(&agent, turn, crate::store::now());

    let Some(line) = line else {
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

thread_local! {
    /// Whether this thread is one that stays for whatever it asks, rather
    /// than printing a line or drawing a frame and exiting.
    ///
    /// A thread local and not a flag for the whole process: everything
    /// downstream of one verb's dispatch runs on a single thread in the amx
    /// that is actually running, so the distinction costs a real process
    /// nothing, but a test binary is one process running its whole suite at
    /// once, on a thread of its own per test (`tui::rows` reasons the same
    /// way about its own thread local). A process-wide flag would have the
    /// first test that opens a view leave every test after it believing an
    /// `ls` was staying too.
    static STAYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Declare that this process is staying for whatever it asks, not exiting
/// once it has drawn a frame or printed a line.
///
/// Called once, where the view's own loop starts — the one caller with
/// somewhere to put an answer that comes back after the read that asked for
/// it has finished. A one-shot verb, `ls` and `statusline` among them, never
/// calls this, so [`have_a_line_written`] never claims a turn on their
/// account: nothing would be left to hear the command back, or to pay for
/// running it.
pub(crate) fn will_stay_for_the_answer() {
    STAYING.with(|staying| staying.set(true));
}

/// Whether this thread declared [`will_stay_for_the_answer`].
fn staying() -> bool {
    STAYING.with(std::cell::Cell::get)
}

/// Whether an ask is out already.
///
/// The command is whatever somebody configured, routinely a model call, and a
/// view opened on a week of finished agents would otherwise start one for
/// every row at once — all of them at the same moment, for rows nobody is
/// waiting on. One at a time turns that into a queue draining at the rate the
/// command answers, and nobody is waiting for any of it.
static ASKING: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// Whether this is the moment to ask. A turn refused because something else is
/// being asked about is claimed by nothing, so the next reading offers it
/// again.
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

/// One question over, whatever it answered.
fn done_asking() {
    if let Ok(mut asking) = ASKING.lock() {
        *asking = false;
    }
}

/// What the last ask was, beside the record it was about.
const ASKED: &str = "summary.asked";

/// The last ask about one agent: which turn it was about, when it went out,
/// and whether it came back.
///
/// Beside the record because a queue inside one process cannot answer this:
/// `amx ls --json` in a caller's loop is a new process every time, and each
/// would find a finished turn with no line on it and start the command again
/// for as long as the loop runs. The record is what those processes share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
struct Asked {
    turn: u64,
    at: u64,
    /// Whether whoever asked heard back. A command that answered nothing has
    /// answered: it will answer nothing again in five minutes, and asking a
    /// model the same question every five minutes for as long as a view is
    /// open is the one way this key could quietly cost somebody money.
    over: bool,
}

/// How long an ask that has not come back is taken to be still out.
///
/// The ask is made on a thread nobody joins, so a verb that prints and exits
/// takes its unfinished ask with it — and a claim left behind by one of those
/// would be a turn nothing ever asks about again. Long enough that a command
/// still thinking is not asked the same question twice, short enough that the
/// row gets its line from the next reader rather than from the next turn.
const AGAIN: u64 = 300;

/// Whether this turn is worth asking about, given what the last ask was.
fn worth_asking(asked: Option<Asked>, turn: u64, now: u64) -> bool {
    match asked {
        Some(asked) if asked.turn == turn => !asked.over && now.saturating_sub(asked.at) >= AGAIN,
        // Nothing asked yet, or asked about the turn before this one.
        _ => true,
    }
}

/// Claim a turn, so one amx asks about it rather than every amx that happens
/// to read the agent.
///
/// Written under the writer's lock, which is the one thing exclusive across
/// every amx on the machine, and read once without it first: a claim already
/// made costs a small file read, and a turn with a line on it never gets this
/// far.
fn claim_the_turn(agent: &Agent, turn: u64, now: u64) -> bool {
    if !worth_asking(asked(agent.dir()), turn, now) {
        return false;
    }
    let Ok(_writer) = agent.writer() else {
        return false;
    };
    // Under the lock, where two amx that read the same absence become one.
    if !worth_asking(asked(agent.dir()), turn, now) {
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

/// Say the ask came back, whatever it came back with.
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

/// The last ask about this agent, where there was one.
fn asked(dir: &Path) -> Option<Asked> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(ASKED)).ok()?).ok()
}

/// Through [`crate::store::write_atomic`], so a write torn by a crash or a
/// second amx cannot read back as no claim at all: the file is either the one
/// there before or the whole of this one, and never half of either.
fn write_asked(dir: &Path, asked: Asked) -> bool {
    let Ok(said) = serde_json::to_string(&asked) else {
        return false;
    };
    crate::store::write_atomic(&dir.join(ASKED), said.as_bytes()).is_ok()
}

/// Set that going, with nobody waiting for it — except a caller that has said
/// it will be.
///
/// The thread is never joined, the way a look at a forge is not
/// (`crate::pr`): a view is open for hours and has the line on its next
/// reading. A verb that exits first is never in this function at all — see
/// [`staying`] — so exiting first truly costs the line and nothing else,
/// rather than a claim it never comes back to settle. A command that never
/// returns costs the thread it is on, and the queue behind it.
fn have_a_line_written(
    root: &Path,
    agent: &Agent,
    meta: &Meta,
    state: &State,
    command: &str,
    now: u64,
) {
    // Only a reader that can settle the ask may claim a turn: nothing else is
    // here to hear the command back, or ought to be paying to run it.
    if !staying() {
        return;
    }
    // Before the queue rather than after it. A turn somebody has already asked
    // about is one this reading will never ask about, and a row that stood in
    // the queue holding a place it cannot use would keep every row under it
    // from ever being asked about at all.
    if !worth_asking(asked(agent.dir()), state.since, now) {
        return;
    }
    if !may_ask() {
        return;
    }
    if !claim_the_turn(agent, state.since, now) {
        done_asking();
        return;
    }

    let (root, id, turn) = (root.to_path_buf(), meta.id.clone(), state.since);
    let at = where_it_ran(meta);
    let command = command.to_string();
    let answer = state.result.clone().unwrap_or_default();
    let asking = std::thread::Builder::new()
        .name("amx-summary".to_string())
        .spawn(move || {
            write_the_line(&root, &id, turn, &at, &command, &answer);
            done_asking();
        });
    if asking.is_err() {
        done_asking();
    }
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
    // Taken here rather than left to the closure below, so there is a screen
    // in hand to weigh against the last one this process saw of this id
    // before `read` is asked to trust a count of anything.
    let screen = wants_the_screen(&state, alive, now)
        .then(|| server.capture(&meta.pane).ok())
        .flatten();
    let looks = still_looks(id, screen.as_deref(), state.state, rules);
    let reading = read(&state, meta.created, alive, || screen, rules, now, looks);
    if let Some(asking) = &reading.asking {
        note(&agent, &mut state, asking);
    }
    if let Some(command) = crate::config::current().summary_command.as_deref()
        && wants_a_line(&state)
    {
        have_a_line_written(root, &agent, &meta, &state, command, now);
    }

    Ok(seen(meta, state, reading))
}

/// One agent's record, read off the disk.
///
/// A listing does two things with the same records: it forgets the ones that
/// have outlived their use, and it concludes about the ones that are left. Both
/// are answered out of the state document, so the document is parsed once and
/// handed on rather than opened again by whoever asks next.
pub struct Record {
    pub agent: Agent,
    pub meta: Meta,
    pub state: State,
}

/// Every agent's record on the machine, in whatever order the directory is in.
///
/// A record amx cannot read the meta or the state of is skipped: how an agent
/// was started and what it is doing are each read from their own file, and a
/// walk that broke on one unreadable document would cost every agent listed
/// after it, not just the one whose file is bad.
pub fn records(root: &Path) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for id in crate::store::list(root)? {
        let agent = Agent::open(root, &id)?;
        let Ok(meta) = agent.meta() else { continue };
        let Ok(state) = agent.state() else { continue };
        records.push(Record { agent, meta, state });
    }
    Ok(records)
}

/// One agent's record, and whether its pane is still there: everything a
/// reading of a wall has in hand before it asks for a screen.
struct Pending {
    record: Record,
    alive: bool,
}

/// Read every agent, oldest first.
pub fn views(root: &Path, rules: &Ruleset, now: u64) -> Result<Vec<View>> {
    Ok(views_of(root, records(root)?, rules, now))
}

/// The same, over records that have already been read.
///
/// Two passes over the records with one round of tmux between them, because
/// what tmux is asked is worked out from the records and the answer comes back
/// for all of them at once: one pane list per server, and one call for every
/// screen the reading needs. A wall of ten agents is two tmux calls, not
/// twenty.
pub fn views_of(root: &Path, records: Vec<Record>, rules: &Ruleset, now: u64) -> Vec<View> {
    let mut pending: Vec<Pending> = Vec::new();
    let mut panes: Vec<(crate::tmux::Socket, Vec<crate::tmux::PaneId>)> = Vec::new();

    for record in records {
        let meta = &record.meta;
        let alive = if record.state.state.is_terminal() {
            true
        } else {
            let listed = match panes.iter().find(|(socket, _)| socket == &meta.socket) {
                Some((_, listed)) => listed,
                None => {
                    let listed = Server::from_socket(meta.socket.clone())
                        .panes()
                        .unwrap_or_default();
                    panes.push((meta.socket.clone(), listed));
                    &panes.last().expect("just pushed").1
                }
            };
            listed.contains(&meta.pane)
        };

        pending.push(Pending { record, alive });
    }

    let mut screens = screens_of(&pending, now);
    let mut views = Vec::new();
    for (at, item) in pending.into_iter().enumerate() {
        let Pending {
            record:
                Record {
                    agent,
                    meta,
                    mut state,
                },
            alive,
        } = item;
        // Taken rather than borrowed: the reading is handed the screen, and
        // there is one reading it belongs to.
        let screen = screens[at].take();
        let looks = still_looks(&meta.id, screen.as_deref(), state.state, rules);

        let reading = read(&state, meta.created, alive, || screen, rules, now, looks);
        if let Some(asking) = &reading.asking {
            note(&agent, &mut state, asking);
        }
        if let Some(command) = crate::config::current().summary_command.as_deref()
            && wants_a_line(&state)
        {
            have_a_line_written(root, &agent, &meta, &state, command, now);
        }

        views.push(seen(meta, state, reading));
    }

    views.sort_by_key(|view| (view.meta.created, view.meta.id.clone()));
    views
}

/// The screens this reading needs, taken a server at a time, in the order the
/// agents were read in.
///
/// Which of them is wanted is worked out from the records first — see
/// [`wants_the_screen`] — so a wall where every record is fresh asks tmux for
/// nothing at all, and one where none of them is asks once. An agent whose
/// screen was not wanted, and one whose pane went between the listing and the
/// call, both come back with nothing: a reading that wanted neither reads no
/// screen either way.
fn screens_of(pending: &[Pending], now: u64) -> Vec<Option<String>> {
    let mut screens: Vec<Option<String>> = vec![None; pending.len()];
    let mut wanted: Vec<(crate::tmux::Socket, Vec<usize>)> = Vec::new();

    for (at, item) in pending.iter().enumerate() {
        if !wants_the_screen(&item.record.state, item.alive, now) {
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

/// Every agent as the records alone have them, oldest first, with nothing
/// asked of tmux.
///
/// What a surface draws before it has waited for anything. A reading costs a
/// pane list per server and a capture for every agent that has gone quiet, and
/// somebody who has just opened the view is looking at an empty terminal for
/// as long as that takes. The records are on disk and are the vendor's own
/// account of what each agent was last doing, which is enough to draw a wall
/// with.
///
/// Every conclusion here is the record's, so an agent whose pane has gone
/// still reads as whatever it was doing when it went, and a question the
/// record cannot name stays unnamed. The reading is what corrects that, and
/// nothing about this is a substitute for one: it is the frame before it.
pub fn recorded(root: &Path, now: u64) -> Result<Vec<View>> {
    let mut views: Vec<View> = records(root)?
        .into_iter()
        .map(|record| {
            let verdict = from_the_record(&record.state, record.meta.created, now);
            View::new(record.meta, record.state, verdict)
        })
        .collect();

    views.sort_by_key(|view| (view.meta.created, view.meta.id.clone()));
    Ok(views)
}

/// What the record on its own says about one agent.
///
/// The phase it holds, which is where every phase comes from until a screen
/// disagrees with it, and the same clock beside it that a reading would put
/// there. The evidence is the record's for a run that has ended and the hooks'
/// for one that has not, which is what [`read`] calls those two: this says no
/// more about where it came from than a reading would.
fn from_the_record(state: &State, created: u64, now: u64) -> Verdict {
    let phase = state.state;
    Verdict {
        phase,
        evidence: match phase.is_terminal() {
            true => Evidence::Record,
            false => Evidence::Hooks,
        },
        rule: None,
        age: clock(phase, state, created, now),
        worked: worked(phase, state, created, now),
    }
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

    /// The vendor's own menu, captured off a live claude 2.1.240 at 220
    /// columns on 2026-08-25 and cut to the rows a floor of 24 would reach.
    /// The two rows under the choices are the vendor's own furniture: neither
    /// is in the payload the tool call carried.
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
            worked: 30,
        }
    }

    fn reading(state: &State, alive: bool, screen: Option<&str>, now: u64) -> Reading {
        started(0, state, alive, screen, now)
    }

    /// The same reading of an agent started at a stated moment, which is what
    /// a finished run with no spans of work on it is measured from.
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
    fn reader_lets_the_menu_on_the_screen_say_what_answers_it() {
        use crate::store::Kind;

        // A record an older amx wrote: the vendor asks itself for permission
        // to use its own question tool, that event arrived after the tool call
        // that drew the menu, and `permission` is what was left on the record.
        // The menu is on the pane, and a caller reading `permission` is told
        // to answer a menu with y or n.
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

        // It goes the one way only. A record that says a question is being
        // asked is the vendor's own account of its own state, and a rule that
        // named some other screen is amx's reading of a picture: the screen
        // fills what the hooks left empty and corrects nothing.
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

        // And a screen no rule claimed says nothing about the kind either way.
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
    fn reader_finds_the_spinning_line_by_the_vendors_own_fragments() {
        // Two punctuation fragments are what claude's line always carries, and
        // they are claude's: another vendor spins a line of its own, on which
        // they never appear. Both are read off the document that says so.
        let second = second_vendors_screens();
        let its_own = " thinking for 12s about the file you named\n = compose =\n";
        assert_eq!(
            doing(&second, its_own).as_deref(),
            Some("thinking for 12s about the file you named")
        );
        assert_eq!(
            doing(rules::bundled(), its_own),
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

    /// Whether a reading of this record went to the pane at all, which is the
    /// question [`wants_the_screen`] has to answer without going there.
    fn looked_at_the_pane(state: &State, alive: bool, now: u64) -> bool {
        let asked = std::cell::Cell::new(false);
        read(
            state,
            0,
            alive,
            || {
                asked.set(true);
                Some(A_BLOCKING_SCREEN.to_string())
            },
            rules::bundled(),
            now,
            1,
        );
        asked.get()
    }

    #[test]
    fn reader_says_which_readings_need_a_pane_before_it_takes_one() {
        // A wall's screens are asked for in one call, so which of them are
        // wanted is worked out from the records before the first is taken.
        // That answer has to be the one the reading itself reaches: a reading
        // that wanted a screen nobody asked for would conclude `unknown` off a
        // capture that was never taken, and one that did not would have amx
        // pay for a screen it reads nothing off.
        let mut asked = state(Phase::Waiting, 1_000);
        asked.question = Some("Do you want to proceed?".to_string());
        let mut placeheld = state(Phase::Waiting, 1_000);
        placeheld.question = Some(A_PLACEHOLDER.to_string());

        let records = [
            state(Phase::Starting, 1_000),
            state(Phase::Working, 1_000),
            state(Phase::Waiting, 1_000),
            asked,
            placeheld,
            state(Phase::Idle, 1_000),
            state(Phase::Done, 1_000),
            state(Phase::Failed, 1_000),
            state(Phase::Stopped, 1_000),
        ];
        for record in records {
            for alive in [true, false] {
                // Fresh, on the last second of freshness, and stale.
                for now in [1_000, 1_000 + FRESH, 1_100] {
                    assert_eq!(
                        wants_the_screen(&record, alive, now),
                        looked_at_the_pane(&record, alive, now),
                        "{} alive={alive} at {now}",
                        record.state
                    );
                }
            }
        }
    }

    /// A tmux server of this test's own, gone when the test is.
    struct Own(Server);

    impl Drop for Own {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    /// A pane with a screen on it and nothing running but a sleep, on a
    /// server nothing else is using.
    fn a_pane_showing(server: &Server, screen: &str) -> crate::tmux::PaneId {
        let showing = [
            "sh",
            "-c",
            "printf '%s' \"$0\"; while :; do sleep 0.05; done",
            screen,
        ];
        let (_, pane) = server
            .new_session(&crate::tmux::Spawn {
                command: &showing,
                ..crate::tmux::Spawn::default()
            })
            .expect("a pane to read");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if server.capture(&pane).is_ok_and(|drawn| drawn.contains('❯')) {
                return pane;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the screen never reached the pane");
    }

    /// An agent's record on disk: how it was started, and what it was last
    /// heard doing. Written rather than recorded through a writer, because a
    /// reading is about a record that has gone quiet and a test says when.
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

        // Two agents on one server, both gone quiet, each with a different
        // screen on its pane. The screens are taken in one call, and what
        // says they were handed back to the right readings is that the two
        // readings differ.
        let asking = a_pane_showing(&server.0, A_BLOCKING_SCREEN);
        let idle = a_pane_showing(&server.0, IDLE_SCREEN);
        for (id, pane, phase) in [
            ("asks-a1b", &asking, Phase::Working),
            ("idles-b2c", &idle, Phase::Starting),
        ] {
            a_record(
                root.path(),
                &Meta {
                    id: id.to_string(),
                    socket: socket.clone(),
                    pane: pane.clone(),
                    ..meta()
                },
                &state(phase, 1_000),
            );
        }

        let views = views(root.path(), rules::bundled(), 1_100).expect("a reading");
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
    fn reader_answers_from_the_records_before_it_has_asked_tmux_anything() {
        let root = TempDir::new().unwrap();
        let socket = crate::tmux::Socket::Name("amx-not-a-server".to_string());
        a_record(
            root.path(),
            &Meta {
                socket,
                created: 1_000,
                ..meta()
            },
            &state(Phase::Working, 1_000),
        );

        // Nothing is listening on that socket, so anything that asked tmux
        // about this agent finds no pane and calls it stopped. What the
        // records say is that it is working, and that is the whole of what a
        // surface has before it has waited for anything.
        let recorded = recorded(root.path(), 1_100).expect("the records");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].phase(), Phase::Working);
        assert_eq!(recorded[0].verdict.evidence, Evidence::Hooks);
        assert_eq!(recorded[0].verdict.age, 100, "with the record's own clock");

        let read = views(root.path(), rules::bundled(), 1_100).expect("a reading");
        assert_eq!(
            read[0].phase(),
            Phase::Stopped,
            "which is what looking at the pane costs"
        );
        assert_eq!(read[0].verdict.evidence, Evidence::Gone);
    }

    #[test]
    fn reader_reads_a_record_that_has_ended_the_same_way_with_or_without_a_look() {
        // The record ended it, and a reader that has looked at nothing says
        // so in the same words as one that has: this is not a guess either
        // reader is making.
        let root = TempDir::new().unwrap();
        let mut done = state(Phase::Done, 1_000);
        done.ended = 1_000;
        done.exit = Some(0);
        a_record(
            root.path(),
            &Meta {
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
    fn records_skips_an_agent_whose_state_json_is_unreadable_but_keeps_the_rest() {
        let root = TempDir::new().unwrap();
        let broken = Agent::create(
            root.path(),
            &Meta {
                id: "broken-a1b".to_string(),
                ..meta()
            },
        )
        .expect("a record");
        std::fs::write(broken.dir().join("state.json"), b"not json at all").expect("garbage bytes");
        a_record(
            root.path(),
            &Meta {
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

    /// claude's chrome, whole, with `tick` standing in for whatever changes
    /// every second — a statusline's elapsed timer or its token count — under
    /// a transcript that never moves. Two calls with different ticks are two
    /// different captures of the same still screen.
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

    #[test]
    fn still_looks_settles_after_settled_looks_even_while_the_chrome_ticks() {
        let rules = rules::bundled();
        let id = "ticking-t4a";
        let mut looks = 0;
        for tick in 0..rules::SETTLED_LOOKS as u64 {
            looks = still_looks(id, Some(&a_ticking_screen(tick)), Phase::Working, rules);
        }
        assert_eq!(
            looks,
            rules::SETTLED_LOOKS,
            "the statusline ticked on every look, and none of it is the screen"
        );
    }

    #[test]
    fn still_looks_resets_the_moment_the_transcript_itself_changes() {
        let rules = rules::bundled();
        let id = "resets-t4b";
        let first = a_ticking_screen(1);
        assert_eq!(still_looks(id, Some(&first), Phase::Working, rules), 1);
        assert_eq!(still_looks(id, Some(&first), Phase::Working, rules), 2);
        assert_eq!(still_looks(id, Some(&first), Phase::Working, rules), 3);

        let changed = a_ticking_screen(1).replace("Done building", "Ran the migration and");
        assert_eq!(
            still_looks(id, Some(&changed), Phase::Working, rules),
            1,
            "a mid-turn change is not the screen holding still"
        );
    }

    #[test]
    fn still_looks_resets_when_the_record_moves_to_another_phase() {
        let rules = rules::bundled();
        let id = "moves-t4c";
        let screen = a_ticking_screen(1);
        assert_eq!(still_looks(id, Some(&screen), Phase::Working, rules), 1);
        assert_eq!(still_looks(id, Some(&screen), Phase::Working, rules), 2);

        assert_eq!(
            still_looks(id, Some(&screen), Phase::Waiting, rules),
            1,
            "a hook's own news between two looks is not the screen holding still either"
        );
    }

    #[test]
    fn still_looks_reads_a_first_look_the_way_a_one_shot_verb_always_has() {
        let rules = rules::bundled();
        assert_eq!(
            still_looks(
                "fresh-t4d",
                Some(&a_ticking_screen(1)),
                Phase::Working,
                rules
            ),
            1,
            "status, send and the rest read one look at a time, same as before this counted anything"
        );
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
        // Started at 1_000, worked ten seconds and stood at a question for the
        // hour in between. Ten seconds is what it worked, and a run that
        // worked ten seconds worked ten seconds whenever anybody asks.
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
        // Working: the spans already added up and the open one, moving with
        // the clock.
        let mut working = state(Phase::Working, 1_000);
        working.worked = 120;
        assert_eq!(
            reading(&working, true, None, 1_000 + FRESH).verdict.worked,
            120 + FRESH
        );

        // Waiting: frozen where the work stopped — an hour at a question
        // nobody answered is nobody's work. The wait is still the age, which
        // is what the card reads it off.
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

        // Ended: what it worked, for good, with the whole run standing in
        // where no spans were ever added up — the same answers the age gives.
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
        // The units are decided here, where the number is worked out, and
        // every surface that prints it says them by asking. Two surfaces with
        // a set of units each is how one of them comes to print a bare number
        // while the other prints 4s.
        assert_eq!(in_words(0), "0s");
        assert_eq!(in_words(45), "45s");
        assert_eq!(in_words(59), "59s");
        assert_eq!(in_words(60), "1m");
        assert_eq!(in_words(3_599), "59m");
        assert_eq!(in_words(3_600), "1h");
        assert_eq!(in_words(86_399), "23h");
        assert_eq!(in_words(86_400), "1d");

        // And the reading a surface has in its hand goes through it: the run
        // that worked ten seconds says ten seconds, in words, a day later.
        let mut done = state(Phase::Done, 4_610);
        done.ended = 4_610;
        done.worked = 10;
        let verdict = started(1_000, &done, true, None, 90_000).verdict;
        assert_eq!(in_words(verdict.age), "10s");
    }

    #[test]
    fn reader_counts_a_span_of_work_nothing_ever_closed() {
        // The pane went out from under a turn, so nothing wrote the phase out
        // of working and the last span is open on the record. It closes where
        // every ending amx did not see closes: at the last thing it heard.
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
        // A record written before amx added spans up, and one of an agent that
        // never worked at all, are the same record to a reader: with nothing
        // added up, the row says how long the run was alive, which is what it
        // has always said.
        let mut older = state(Phase::Done, 1_300);
        older.ended = 1_300;
        assert_eq!(older.worked, 0);
        assert_eq!(started(1_000, &older, true, None, 9_000).verdict.age, 300);
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
        // added without a kind would leave a caller guessing again — in any
        // document, because the law is about screens that block and not about
        // whose they are.
        let second = second_vendors_screens();
        for screens in [rules::bundled(), &second] {
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

        let claude = rules::bundled();
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

        // The rules say which screen is on the pane and the same rule says
        // what that screen wants back. Written in Rust as a match on rule
        // names, this read every other vendor's document with claude's names
        // in hand: its own screens would each have answered nothing at all.
        let second = second_vendors_screens();
        assert_eq!(asked_kind(&second, Some("choice")), Some(Kind::Question));
        assert_eq!(
            asked_kind(&second, Some("permission_prompt")),
            None,
            "that screen is not on this vendor's pane"
        );
    }

    /// The screens of the vendor amx keeps to prove that none of this is
    /// claude's shape.
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
    fn summary_command_takes_an_answer_longer_than_a_pipe_holds() {
        // A megabyte is an ordinary long turn and several times what a pipe
        // holds on any of these machines. Handing one to a command that writes
        // back what it reads stops both of them where the answer goes in and
        // what comes back is read on the one thread: the command's own pipe
        // fills, it stops reading, and amx is still writing into a pipe
        // nothing is draining. `tr a-z A-Z` is the shape of it, and it is the
        // shape the key's own documentation offers.
        let at = TempDir::new().unwrap();
        let answer = format!("fixed the redirect\n{}\n", "y".repeat(1024 * 1024));

        // On a thread of its own so the deadlock is a failure rather than a
        // suite that never ends.
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

    /// The queue is one place for the whole process, and every test about it
    /// is a thread of one test binary sharing that place. Two of them running
    /// at once would each see the other's turn in the queue and read it as its
    /// own, so they take turns here instead.
    ///
    /// Taken through a poisoned lock rather than around one: a test that
    /// panicked holding this has failed already, and the next one failing for
    /// having asked about it is a second failure about the first.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn the_queue_to_itself() -> std::sync::MutexGuard<'static, ()> {
        ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|held| held.into_inner())
    }

    #[test]
    fn summary_asks_about_one_turn_at_a_time() {
        let _queue = the_queue_to_itself();

        // What answers is whatever somebody configured, routinely a model
        // call, so a view opened on a week of finished agents queues them
        // rather than starting one for every row at once.
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

        assert!(worth_asking(None, 100, 1_000), "nobody has asked yet");
        assert!(
            !worth_asking(out(100, 1_000), 100, 1_100),
            "an ask that went out a minute ago is still out"
        );
        assert!(
            worth_asking(out(100, 1_000), 100, 1_000 + AGAIN),
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
                90_000
            ),
            "a command that answered nothing has answered"
        );
        assert!(
            worth_asking(out(100, 1_000), 200, 1_100),
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

        assert!(claim_the_turn(&agent, ended.since, 1_000));
        assert!(
            !claim_the_turn(&agent, ended.since, 1_001),
            "a caller's next ls is a new process, and the record is the only \
             thing either of them shares"
        );

        // A turn nobody may ask about again does not stand in the queue for
        // the rows under it, which is the whole of what those rows would get:
        // this one is the first row of every reading.
        have_a_line_written(root.path(), &agent, &meta(), &ended, "true", 1_001);
        assert!(may_ask(), "the queue is where it was");
        done_asking();

        // The command answered, with a line or with nothing, and either way
        // the question has been put.
        settle_the_ask(&agent, ended.since, 1_002);
        assert!(!claim_the_turn(&agent, ended.since, 90_000));
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

        // Nothing on this thread has declared it is staying for the answer,
        // which is every verb but the view.
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

        assert_eq!(
            asked(agent.dir()),
            Some(Asked {
                turn: ended.since,
                at: 1_000,
                over: false,
            }),
            "a reader staying for the answer claims a turn exactly as it always has"
        );

        // The command runs on a thread of its own; wait for it to settle and
        // free the queue before the next test takes it.
        let waited = std::time::Instant::now();
        loop {
            if may_ask() {
                done_asking();
                break;
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
        );

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
        write_the_line(
            root.path(),
            "fix-login-a1b",
            ended.since,
            at.path(),
            "cat",
            &answer,
        );
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
