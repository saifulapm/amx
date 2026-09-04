//! What amx knows about an agent, on disk.
//!
//! One directory per agent, `<state root>/<id>/`, holding three files:
//!
//! * `meta.json` — how the agent was started and where to find it again.
//! * `state.json` — what it is doing, as the last event left it.
//! * `events.jsonl` — one line per event, in the order they arrived.
//!
//! **One writer at a time.** Every mutation goes through a [`Writer`], which
//! holds an exclusive `flock` for as long as it lives, so two hooks firing at
//! once cannot lose each other's work or split a line in half.
//!
//! **Readers never lock to read.** `ls`, `status` and the view read these
//! files while agents are running, and waiting on a lock to draw a list would
//! be a poor trade. They see whole documents anyway: a document is written to
//! a neighbouring temporary file and renamed over its target, and a rename is
//! atomic. A reader gets the old document or the new one, never a half of
//! either. A reader that finds a question on a pane does take the lock, to put
//! it where the next reader will find it — see [`Writer::observe`] — but only
//! on the look that finds one.
//!
//! A crash can therefore lose the last write, since the rename is not followed
//! by an fsync. That is the trade the hook path is worth: state on disk is a
//! fast path, and a reader that finds it stale falls back to the pane.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;
use crate::tmux::{PaneId, Socket};

const META: &str = "meta.json";
const STATE: &str = "state.json";
const EVENTS: &str = "events.jsonl";
const LOCK: &str = "lock";

/// Where an agent is, as far as amx has been told.
///
/// This is what the hooks and the exit record say. It is not the whole answer:
/// a reader weighs it against how old it is and against the pane itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Spawned, with nothing heard from it yet.
    #[default]
    Starting,
    /// Working on the turn.
    Working,
    /// Stopped on a question and waiting to be answered.
    Waiting,
    /// The turn ended; the agent is sitting at its prompt.
    Idle,
    /// The command exited successfully.
    Done,
    /// The command exited with a failure.
    Failed,
    /// Ended by `amx stop`.
    Stopped,
    /// Nothing amx knows accounts for what is on the screen.
    Unknown,
}

impl Phase {
    /// Whether nothing more is coming from this agent.
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::Stopped)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Working => "working",
            Phase::Waiting => "waiting",
            Phase::Idle => "idle",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Stopped => "stopped",
            Phase::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a result came from, recorded beside the result itself: the transcript
/// is written asynchronously, so which source answered says how much to trust
/// what it said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The Stop hook's own payload — the freshest source there is.
    Payload,
    /// The tail of the session transcript.
    Transcript,
    /// The last screenful of the pane.
    Screen,
    /// No source had an answer, and this says why.
    Error,
}

/// What an agent has stopped to ask, and the answers it offers.
///
/// The text is the vendor's own words where a hook carried them, and amx's
/// reading of the pane where none did. The options are always the pane's: no
/// hook this vendor fires has ever named them, and the screen is the only
/// place they are written down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Question {
    /// What is being asked.
    pub text: String,
    /// The numbered choices, in the order the screen lists them. Empty until
    /// something has read the screen, and empty on a question that takes words
    /// rather than a key.
    pub options: Vec<String>,
}

/// What kind of thing an agent has stopped to ask.
///
/// Three screens block a claude agent, and none of them takes the answer the
/// others take: a permission box wants one tool call allowed or refused, an
/// AskUserQuestion menu wants a choice or words of your own, and the
/// folder-trust screen wants a decision about the directory before the vendor
/// will start a session in it at all. Which one it is decides what may be sent
/// back, so it belongs on the record beside the question rather than left for
/// whoever reads it to guess from the wording.
///
/// It is only ever written from something that said so: a hook event that is
/// about a permission prompt, the vendor's own name for a notification, the
/// tool a question arrives as, or the rule that claimed the screen. Nothing
/// here is inferred from the words of the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A tool call the agent may not make until somebody allows it.
    Permission,
    /// The vendor asking its own question, which takes a choice or words.
    Question,
    /// The folder-trust screen, which stands between the vendor and a session.
    Trust,
}

/// One choice under a question, as the vendor wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Choice {
    /// What an answer names it by, and what comes back when it is chosen.
    pub label: String,
    /// The sentence the screen draws under the label. The label is the answer;
    /// this is what explains it to whoever is reading.
    pub description: Option<String>,
    /// The block the vendor draws beside the choice instead of the
    /// descriptions. It is also what turns the notes field on — a question
    /// whose choices carry no preview takes no note — and the chosen one rides
    /// back beside the note in the vendor's own answer.
    pub preview: Option<String>,
}

/// One question of a call that asks more than one.
///
/// `AskUserQuestion` takes up to four questions and draws them as tabs on a
/// single screen, and the payload is the only place their number, their names,
/// the sentences under their choices and the flag saying how many may be taken
/// are ever written down. Measured against 2.1.240 on 2026-08-24 and recorded
/// in `docs/question-shapes.md`: the tab strip elides its headers as the pane
/// narrows, and at 24 columns the showing tab's own name is drawn as nothing
/// but an ellipsis. So this is the payload's version of the question, and
/// nothing here is ever read off a screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Ask {
    /// The short word the tab strip draws for it.
    pub header: Option<String>,
    /// What is being asked.
    pub text: String,
    /// The choices under it, in the order the screen lists them.
    pub options: Vec<Choice>,
    /// Whether it takes more than one choice. The flag is per question and not
    /// per call: a checkbox tab and two plain ones sit in the same prompt, and
    /// the keys that drive one are not the keys that drive the next.
    pub multi: bool,
    /// What was sent back for it, once somebody has answered it. A call with
    /// more than one question does not end when one is answered — the vendor
    /// moves to the next tab and the prompt is still up — so the answer goes
    /// on the question it belongs to.
    pub answer: Option<String>,
}

impl Ask {
    /// The choices as an answer names them.
    pub fn labels(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| option.label.clone())
            .collect()
    }

    /// Whether the vendor draws a notes field on this question. It draws one
    /// when a choice carries a preview and only then: `n` on a menu without
    /// one does nothing at all.
    pub fn takes_notes(&self) -> bool {
        self.options.iter().any(|option| option.preview.is_some())
    }
}

/// How the agent was started, and how to reach it again.
///
/// Fields are added, never renamed or removed: a record outlives the version
/// of amx that wrote it, so every field that may be absent carries a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub task: String,
    /// What runs the agent: the command `new` resolved for it, the word the
    /// agent it was forked from was launched with, or the vendor an adoption
    /// found in the pane. `None` from a shell command, which runs no vendor,
    /// and from a record written before amx kept this.
    #[serde(default)]
    pub agent: Option<String>,
    /// Where the agent runs — its worktree, or the directory it was asked for.
    pub dir: PathBuf,
    /// The worktree amx made for it, if it made one.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    #[serde(default)]
    pub branch: Option<String>,
    /// The commit the worktree started from, so a diff has something to say.
    #[serde(default)]
    pub base: Option<String>,
    /// The tmux server the pane lives on. Recorded at spawn, because every
    /// tmux-touching verb has to target this server and not whichever one the
    /// caller happens to be inside.
    pub socket: Socket,
    pub pane: PaneId,
    /// Started out of sight rather than on the wall — which is where a
    /// resume puts it back.
    #[serde(default)]
    pub bg: bool,
    /// The vendor's own session id, learned from a hook — what `resume` hands
    /// back to the vendor.
    #[serde(default)]
    pub session: Option<String>,
    /// The transcript that session writes.
    #[serde(default)]
    pub transcript: Option<PathBuf>,
    /// Epoch seconds.
    pub created: u64,
}

/// What the agent is doing, as the last event left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, from = "Wire", into = "Wire")]
pub struct State {
    pub state: Phase,
    /// What a person calls this agent, where somebody has renamed it. The id
    /// is what the record is filed under and what every surface addresses, and
    /// it never moves; this is only what the wall says.
    ///
    /// It lives here rather than beside the task because it is written the way
    /// everything else here is written — by whoever is looking, while the
    /// agent runs — and how the agent was started is not something a rename
    /// changes.
    pub name: Option<String>,
    /// Bumped by each `send`, so `result` can tell this turn's end from the
    /// end of the turn before it.
    pub seq: u64,
    /// Epoch seconds when the agent entered this phase.
    pub since: u64,
    /// One line about what it is doing.
    pub summary: Option<String>,
    /// The question it is waiting on.
    pub question: Option<String>,
    /// The choices that question offers, in the order the screen lists them.
    /// They belong to the question above them and go wherever it goes; the
    /// record has no place for options with no question over them.
    pub options: Vec<String>,
    /// The whole of the call the question came from, where the call is the
    /// vendor asking its own: every question in it, the sentences under their
    /// choices, and which of them have been answered. One question of it is
    /// on the screen at a time, and that one is `question` and `options`
    /// above — which is where everything that wants the question on the screen
    /// has always looked.
    pub asking: Vec<Ask>,
    /// What kind of thing is being asked, where something has said so. It can
    /// be known when the words are not: a menu whose payload amx could not
    /// read is still a menu somebody has to answer.
    pub kind: Option<Kind>,
    /// The answer from the last turn that ended.
    pub result: Option<String>,
    /// Where that answer came from.
    pub source: Option<Source>,
    /// The exit code of the agent's command, once it has one.
    pub exit: Option<i32>,
    /// Epoch seconds of the last event recorded. How fresh this document is,
    /// which decides whether a reader trusts it over the pane.
    pub last_event: u64,
    /// Epoch seconds when the run ended, and zero while it is still going.
    ///
    /// A finished row says how long the run took rather than how long ago it
    /// ended, so the moment it ended is something the record keeps. `since`
    /// holds the same number for as long as the ending is the last thing that
    /// changed, but `since` is about the phase an agent is in, and nothing
    /// asking when a run ended should have to know that the two coincide.
    pub ended: u64,
    /// Epoch seconds when somebody last looked at this agent, and zero for one
    /// nobody has opened. Read against `last_event`, it says whether what the
    /// agent has to say came before or after the last look at it.
    pub seen: u64,
    /// The seconds this agent has spent working, summed over every span of it.
    ///
    /// A run that took an afternoon because nobody answered its question until
    /// four did not work for an afternoon, and the wall clock over a finished
    /// run cannot tell the two apart. The spans are added up at the writes that
    /// move the phase in and out of `working`, because those are the moments
    /// amx is told about; what a span is worth is settled once, at the write
    /// that ends it, and never worked out again from stamps that have moved on.
    pub worked: u64,
}

impl State {
    /// Set what the agent is being asked.
    ///
    /// The options go with it. They are the choices drawn under one particular
    /// question, and under the next question they are somebody else's answers.
    ///
    /// The kind goes only when the question does. Words arriving for a
    /// question already outstanding are just words: the hook that carries them
    /// is describing the same screen something else already named, and it
    /// names nothing itself.
    pub fn asks(&mut self, question: Option<String>) {
        self.question = question;
        self.options.clear();
        self.asking.clear();
        if self.question.is_none() {
            self.kind = None;
        }
    }

    /// Set the whole of what a call is asking.
    ///
    /// The question on the screen is the first with no answer on it, and it
    /// goes where every reader has always found the question. The rest is the
    /// part no screen carries: how many questions there are, what they are
    /// called, what the sentences under their choices say, and which of them
    /// take more than one choice.
    pub fn asks_all(&mut self, asking: Vec<Ask>) {
        self.asking = asking;
        self.shows_the_pending_one();
    }

    /// Record what was sent back for the question on the screen, and put up
    /// the one after it.
    ///
    /// Answering does not end a call that asks more than one question: the
    /// vendor moves to the next tab and the prompt is still up. So the answer
    /// goes on the question it belongs to, and the question is the next one
    /// with nothing on it. When every question has an answer there is nothing
    /// left to ask and the answers stand on the record, which is what the
    /// vendor's own Submit tab is showing.
    pub fn answered(&mut self, answer: impl Into<String>) {
        if let Some(pending) = self.asking.iter_mut().find(|ask| ask.answer.is_none()) {
            pending.answer = Some(answer.into());
        }
        self.shows_the_pending_one();
    }

    /// The question of the call that is on the screen.
    pub fn pending(&self) -> Option<&Ask> {
        self.asking.iter().find(|ask| ask.answer.is_none())
    }

    /// Whether the question on the screen takes more than one choice.
    pub fn multi(&self) -> bool {
        self.pending().is_some_and(|ask| ask.multi)
    }

    /// Put the question the call is showing where the question goes.
    fn shows_the_pending_one(&mut self) {
        let (text, options) = self
            .pending()
            .map(|ask| (ask.text.clone(), ask.labels()))
            .unzip();
        self.question = text;
        self.options = options.unwrap_or_default();
    }

    /// The seconds worked as of a stated moment, counting a span still open.
    ///
    /// The total is added up at the write that ends a span, so a record left
    /// mid-turn — the pane went, and nothing got to write the phase out of
    /// working — is carrying one nothing closed. `at` is how far to count it,
    /// which is whoever is asking's business: the last moment amx can say the
    /// agent was running, or the moment it is being asked in.
    ///
    /// A record with no moment to date the open span from is one amx cannot say
    /// worked at all, so it says nothing rather than counting from the epoch.
    pub fn worked_by(&self, at: u64) -> u64 {
        let open = match (self.state, self.since) {
            (Phase::Working, since) if since > 0 => at.saturating_sub(since),
            _ => 0,
        };
        self.worked.saturating_add(open)
    }

    /// Whether a screen's reading would tell the record anything it has not
    /// got. Asked before the writer's lock is taken, because a reader that has
    /// learned nothing has no business holding it.
    pub fn learns_from(&self, seen: &Question) -> bool {
        (self.question.is_none() && !seen.text.is_empty())
            || (self.options.is_empty() && !seen.options.is_empty())
    }

    /// Take what a screen said, and overwrite nothing.
    ///
    /// A hook is the vendor's own words about its own state; a screen is amx's
    /// reading of a picture of it. So the screen fills what the hooks left
    /// empty — the options, which no hook has ever carried, and the text when
    /// no hook reported one — and never corrects them.
    pub fn learn(&mut self, seen: &Question) {
        if self.question.is_none() && !seen.text.is_empty() {
            self.question = Some(seen.text.clone());
        }
        if self.options.is_empty() {
            self.options.clone_from(&seen.options);
        }
    }
}

/// A state document as it is written down.
///
/// It differs from [`State`] in one place, and this is the only place the two
/// shapes meet: the record keeps a question, the answers it offers and the
/// call it came from under a single `question` key, because they are one thing
/// to everything that reads them, while in memory the text is a field of its
/// own, because the text alone is what most of amx asks for. Options and
/// questions with no question over them are not written at all, which is what
/// keeps an answered question from leaving its choices behind.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Wire {
    #[serde(deserialize_with = "phase_or_unknown")]
    state: Phase,
    name: Option<String>,
    seq: u64,
    since: u64,
    summary: Option<String>,
    question: Option<Asked>,
    result: Option<String>,
    source: Option<Source>,
    exit: Option<i32>,
    last_event: u64,
    ended: u64,
    seen: u64,
    worked: u64,
}

/// A phase this build knows, or [`Phase::Unknown`] for one it does not.
///
/// `state.json` is written by whatever build of amx last touched an agent, and
/// a phase this reader has never heard of is not a document to give up on: the
/// rest of it — the question, the summary, everything else a surface draws —
/// still reads. Only the shared [`Phase`] type stays strict about naming a
/// phase that does not exist, because the screen rules a vendor ships lean on
/// that strictness to catch a typo in the state a rule claims.
fn phase_or_unknown<'de, D>(deserializer: D) -> std::result::Result<Phase, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Named {
        Known(Phase),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Named::deserialize(deserializer)? {
        Named::Known(phase) => phase,
        Named::Other(_) => Phase::Unknown,
    })
}

/// A question on the record: its words alone, or the whole of it.
///
/// Everything amx knows about a question beyond its words is read off the
/// screen, and the screen is only read once the hooks have gone quiet — so a
/// question that has only just been asked is its words and nothing else. That
/// is what amx has always written there, and it still means the same thing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Asked {
    Words(String),
    Whole(Known),
}

/// Everything the record has on one question. Each part is left out when amx
/// has not got it, so the document never claims to know more than it does —
/// and a document written before any of this existed still reads.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Known {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<Kind>,
    /// The call the question came from, whole. The question showing and its
    /// choices are written above as well as here, because they are what every
    /// reader of this document has always read and this adds a field rather
    /// than moving one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    asking: Vec<Ask>,
}

impl From<State> for Wire {
    fn from(state: State) -> Wire {
        // Named one by one rather than by `..`: a field added to the record
        // and left out of the document would be a field that silently does not
        // survive a restart.
        let State {
            state,
            name,
            seq,
            since,
            summary,
            question,
            options,
            asking,
            kind,
            result,
            source,
            exit,
            last_event,
            ended,
            seen,
            worked,
        } = state;

        Wire {
            state,
            name,
            seq,
            since,
            summary,
            question: match (question, kind) {
                // Nothing outstanding. Options with no question over them are
                // not written at all, which is what keeps an answered question
                // from leaving its choices behind.
                (None, None) => None,
                // A question amx knows the kind of and not the words: a menu
                // whose payload it could not read, or a call whose questions
                // have all been answered and is waiting to be submitted. The
                // choices showing under nothing are nobody's to press, but the
                // call itself is what is on the screen and holds the answers
                // given to it so far.
                (None, kind) => Some(Asked::Whole(Known {
                    kind,
                    asking,
                    ..Known::default()
                })),
                // The words alone, as amx has always written a question a hook
                // has only just carried.
                (Some(text), None) if options.is_empty() && asking.is_empty() => {
                    Some(Asked::Words(text))
                }
                (text, kind) => Some(Asked::Whole(Known {
                    text,
                    options,
                    kind,
                    asking,
                })),
            },
            result,
            source,
            exit,
            last_event,
            ended,
            seen,
            worked,
        }
    }
}

impl From<Wire> for State {
    fn from(wire: Wire) -> State {
        let (question, options, kind, asking) = match wire.question {
            Some(Asked::Words(text)) => (Some(text), Vec::new(), None, Vec::new()),
            Some(Asked::Whole(asked)) => (asked.text, asked.options, asked.kind, asked.asking),
            None => (None, Vec::new(), None, Vec::new()),
        };

        State {
            state: wire.state,
            name: wire.name,
            seq: wire.seq,
            since: wire.since,
            summary: wire.summary,
            question,
            options,
            asking,
            kind,
            result: wire.result,
            source: wire.source,
            exit: wire.exit,
            last_event: wire.last_event,
            ended: wire.ended,
            seen: wire.seen,
            worked: wire.worked,
        }
    }
}

/// One thing that happened, as it happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Epoch seconds when amx recorded it.
    pub at: u64,
    /// The vendor's hook event name, or amx's own word for what it did.
    pub kind: String,
    /// What arrived with it, whole and unedited.
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl Event {
    pub fn new(kind: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            at: now(),
            kind: kind.into(),
            payload,
        }
    }
}

/// Epoch seconds. Every timestamp amx records is one of these.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// One agent's directory.
#[derive(Debug, Clone)]
pub struct Agent {
    id: String,
    dir: PathBuf,
}

impl Agent {
    /// Make the directory and write the opening record.
    pub fn create(root: &Path, meta: &Meta) -> Result<Agent> {
        let dir = crate::paths::agent_dir_in(root, &meta.id)?;
        // The record is the document, not the directory: `new` makes the
        // directory first, puts the pane's handoff in it, and only writes the
        // record once there is a pane to record.
        if dir.join(META).exists() {
            bail!("agent `{}` already has a record", meta.id);
        }
        // A task and the answers to it are the owner's business alone — and
        // the directory is usually there already, made by whatever put the
        // pane's handoff in it, so the mode is set rather than asked for.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(paths::DIR_MODE)
            .create(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        paths::keep_to_the_owner(&dir, paths::DIR_MODE)?;

        let agent = Agent {
            id: meta.id.clone(),
            dir,
        };
        write_atomic(&agent.dir.join(META), &to_json(meta)?)?;
        write_atomic(
            &agent.dir.join(STATE),
            &to_json(&State {
                since: now(),
                ..State::default()
            })?,
        )?;
        Ok(agent)
    }

    /// An agent that already has a directory.
    pub fn open(root: &Path, id: &str) -> Result<Agent> {
        let dir = crate::paths::agent_dir_in(root, id)?;
        if !dir.is_dir() {
            bail!("no agent `{id}`");
        }
        Ok(Agent {
            id: id.to_string(),
            dir,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// How it was started.
    pub fn meta(&self) -> Result<Meta> {
        let path = self.dir.join(META);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
    }

    /// What it is doing. A record with no state document yet reads as the
    /// opening state rather than as an error.
    pub fn state(&self) -> Result<State> {
        let path = self.dir.join(STATE);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Where the log itself is, for a reader that tails it rather than reading
    /// it whole.
    pub fn events_path(&self) -> PathBuf {
        self.dir.join(EVENTS)
    }

    /// Everything that has happened, oldest first. A line that does not parse
    /// is skipped: a damaged tail must not cost a reader the whole history.
    pub fn events(&self) -> Result<Vec<Event>> {
        let path = self.dir.join(EVENTS);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect())
    }

    /// Take the writer's lock. Held until the returned value is dropped.
    pub fn writer(&self) -> Result<Writer<'_>> {
        let path = self.dir.join(LOCK);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(paths::FILE_MODE)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        paths::keep_to_the_owner(&path, paths::FILE_MODE)?;
        let lock = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
            .map_err(|(_, errno)| errno)
            .with_context(|| format!("locking {}", path.display()))?;
        Ok(Writer {
            agent: self,
            _lock: lock,
        })
    }

    /// Delete the record and everything in it.
    pub fn remove(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.dir)
            .with_context(|| format!("removing {}", self.dir.display()))
    }
}

/// The right to change one agent's record. Only one exists at a time, across
/// every amx process on the machine.
pub struct Writer<'a> {
    agent: &'a Agent,
    _lock: nix::fcntl::Flock<File>,
}

impl Writer<'_> {
    /// Read the state this writer is about to change.
    pub fn state(&self) -> Result<State> {
        self.agent.state()
    }

    /// Change the state, and record when it was changed.
    ///
    /// `since` moves only when the phase does: how long an agent has been
    /// working is the question a reader asks, and a summary arriving mid-turn
    /// must not reset the clock.
    ///
    /// The ending is stamped by the same rule, because it is the same event:
    /// the write that turns a phase terminal is the run ending, and an answer
    /// or an exit code arriving after it is not a second ending. An agent that
    /// leaves a terminal phase has not ended at all, so the stamp goes with it.
    ///
    /// And a span of work is added up by the same rule again: a phase moving
    /// out of `working` is the span ending, and this write is the only moment
    /// amx is told so. What the span was worth is settled here rather than
    /// worked out later from `since`, which the next phase takes for itself.
    pub fn update_state(&self, change: impl FnOnce(&mut State)) -> Result<State> {
        self.update_state_at(now(), change)
    }

    /// The same, with the clock named, so a test can lay a span of work out in
    /// the past rather than sit through one.
    fn update_state_at(&self, at: u64, change: impl FnOnce(&mut State)) -> Result<State> {
        let before = self.state()?;
        let mut after = before.clone();
        change(&mut after);

        if after.state != before.state {
            if before.state == Phase::Working {
                after.worked = before.worked_by(at);
            }
            after.since = at;
            after.ended = match after.state.is_terminal() {
                true => at,
                false => 0,
            };
        }
        after.last_event = at;

        write_atomic(&self.agent.dir.join(STATE), &to_json(&after)?)?;
        Ok(after)
    }

    /// Write down something read off the pane rather than heard from the
    /// agent.
    ///
    /// `last_event` stays where it was. It says how fresh the *record* is, and
    /// a reader that has looked at a screen has heard nothing: moving it would
    /// make the next reader trust this document over the pane it was copied
    /// from, and an agent that is standing at a question would read as working
    /// again until it went stale a second time.
    ///
    /// A change that changes nothing writes nothing, so a wall of agents being
    /// redrawn once a second costs one write per question and not one per
    /// look.
    pub fn observe(&self, change: impl FnOnce(&mut State)) -> Result<State> {
        let before = self.state()?;
        let mut after = before.clone();
        change(&mut after);

        if after != before {
            write_atomic(&self.agent.dir.join(STATE), &to_json(&after)?)?;
        }
        Ok(after)
    }

    /// Change the record of how the agent was started — the vendor's session
    /// id and its transcript arrive later, from a hook.
    pub fn update_meta(&self, change: impl FnOnce(&mut Meta)) -> Result<Meta> {
        let mut meta = self.agent.meta()?;
        change(&mut meta);
        write_atomic(&self.agent.dir.join(META), &to_json(&meta)?)?;
        Ok(meta)
    }

    /// Append one event. One line, one write, under the lock: two hooks firing
    /// together cannot split each other's records.
    pub fn append(&self, event: &Event) -> Result<()> {
        let path = self.agent.dir.join(EVENTS);
        let mut line = serde_json::to_vec(event).context("writing an event")?;
        line.push(b'\n');

        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(paths::FILE_MODE)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        paths::keep_to_the_owner(&path, paths::FILE_MODE)?;
        file.write_all(&line)
            .with_context(|| format!("appending to {}", path.display()))
    }
}

/// Every agent with a record under `root`, in no particular order.
pub fn list(root: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", root.display())),
    };

    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // A directory named like an id, with the record that makes it one.
        if crate::ids::is_valid(&id) && entry.path().join(META).is_file() {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).context("writing a record")?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Write `bytes` to `path` as a whole document: a reader sees what was there
/// before, or all of this, and never a mixture.
///
/// Shared with the view, which keeps one small document of its own beside the
/// agents: two views open at once, and the last one to quit must not publish
/// half a file to the next one that starts.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} is not a file amx can write", path.display()))?;
    let temporary = path.with_file_name(format!(
        ".{name}.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(paths::FILE_MODE)
        .open(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    // Before a byte of the document goes in, and not after: what is renamed
    // over the target is a file that was never readable by anybody else.
    paths::keep_to_the_owner(&temporary, paths::FILE_MODE)?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", temporary.display()))?;
    drop(file);

    std::fs::rename(&temporary, path)
        .with_context(|| format!("renaming {} onto {}", temporary.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    fn meta(id: &str) -> Meta {
        Meta {
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            dir: PathBuf::from("/srv/app"),
            worktree: Some(PathBuf::from("/srv/app/.amx/worktrees/fix-login-a1b")),
            branch: Some("amx/fix-login-a1b".to_string()),
            base: Some("0f1e2d3".to_string()),
            socket: Socket::Name("amx".to_string()),
            pane: PaneId::new("%7").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: now(),
        }
    }

    #[test]
    fn store_creates_a_record_and_reads_it_back() {
        let root = TempDir::new().unwrap();
        let written = meta("fix-login-a1b");
        let agent = Agent::create(root.path(), &written).unwrap();

        assert_eq!(agent.id(), "fix-login-a1b");
        assert_eq!(agent.dir(), root.path().join("fix-login-a1b"));
        assert_eq!(agent.meta().unwrap(), written);

        let reopened = Agent::open(root.path(), "fix-login-a1b").unwrap();
        assert_eq!(reopened.meta().unwrap(), written);
        assert_eq!(reopened.state().unwrap().state, Phase::Starting);
        assert!(reopened.events().unwrap().is_empty());
    }

    #[test]
    fn store_writes_a_record_into_a_directory_that_is_waiting_for_it() {
        // Spawning makes the directory before there is anything to record in
        // it, so a directory is not a record and does not stand in for one.
        let root = TempDir::new().unwrap();
        let dir = root.path().join("fix-login-a1b");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("handoff.json"), "{}").unwrap();

        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        assert_eq!(agent.meta().unwrap().task, "fix the login bug");
        assert!(
            dir.join("handoff.json").exists(),
            "and nothing was swept up"
        );

        // Twice is a mistake worth naming.
        assert!(Agent::create(root.path(), &meta("fix-login-a1b")).is_err());
    }

    #[test]
    fn store_keeps_a_record_to_its_owner() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("fix-login-a1b");
        Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();

        let mode = std::fs::metadata(&dir).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o700,
            "an agent's task and its answers are nobody else's business"
        );
    }

    /// The permission bits, as anything but amx would read them.
    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A mode as a set of permissions, for a test that leaves one lying about.
    fn mode(bits: u32) -> std::fs::Permissions {
        std::fs::Permissions::from_mode(bits)
    }

    #[test]
    fn hardening_every_file_in_a_record_is_the_owners_alone() {
        // Neither the directory nor the files in it are amx's to assume: the
        // directory is made before there is a record to put in it, a mode
        // passed to `open` is a request the umask may take bits out of, and it
        // is ignored outright for a file that is already there.
        let root = TempDir::new().unwrap();
        let dir = root.path().join("fix-login-a1b");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, mode(0o755)).unwrap();
        std::fs::write(dir.join(EVENTS), "").unwrap();
        std::fs::set_permissions(dir.join(EVENTS), mode(0o644)).unwrap();

        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();
        writer
            .append(&Event::new("SessionStart", serde_json::json!({})))
            .unwrap();
        writer.update_state(|s| s.state = Phase::Working).unwrap();
        writer
            .update_meta(|m| m.session = Some("abc-123".to_string()))
            .unwrap();
        drop(writer);

        assert_eq!(mode_of(&dir), 0o700, "the directory");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            assert_eq!(mode_of(&path), 0o600, "{}", path.display());
            checked += 1;
        }
        assert_eq!(checked, 4, "the record, the state, the log and the lock");
    }

    #[test]
    fn store_refuses_an_id_that_is_not_one() {
        let root = TempDir::new().unwrap();
        assert!(Agent::open(root.path(), "../elsewhere").is_err());
        assert!(Agent::open(root.path(), "never-made-abc").is_err());
    }

    #[test]
    fn store_stamps_the_moment_a_run_ended_and_leaves_it_there() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        let working = writer.update_state(|s| s.state = Phase::Working).unwrap();
        assert_eq!(working.ended, 0, "nothing has ended");

        let done = writer
            .update_state(|s| {
                s.state = Phase::Done;
                s.exit = Some(0);
            })
            .unwrap();
        assert!(done.ended > 0);
        assert_eq!(done.ended, done.since);
        assert_eq!(written(&agent)["ended"], done.ended);

        // An answer written after the ending is not a second ending.
        let after = writer
            .update_state(|s| s.result = Some("wrote the parser".to_string()))
            .unwrap();
        assert_eq!(after.ended, done.ended);

        // And an agent put back to work has not ended at all.
        let again = writer.update_state(|s| s.state = Phase::Working).unwrap();
        assert_eq!(again.ended, 0);
    }

    #[test]
    fn store_adds_up_the_spans_an_agent_spent_working() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        // Six seconds of work, and then a question nobody answers for an hour.
        writer
            .update_state_at(1_000, |s| s.state = Phase::Working)
            .unwrap();
        let asked = writer
            .update_state_at(1_006, |s| s.state = Phase::Waiting)
            .unwrap();
        assert_eq!(asked.worked, 6);

        let answered = writer
            .update_state_at(4_606, |s| s.state = Phase::Working)
            .unwrap();
        assert_eq!(
            answered.worked, 6,
            "an hour of waiting is not an hour's work"
        );

        // Four seconds more, and the run ends.
        let done = writer
            .update_state_at(4_610, |s| {
                s.state = Phase::Done;
                s.exit = Some(0);
            })
            .unwrap();
        assert_eq!(done.worked, 10);
        assert_eq!(written(&agent)["worked"], 10);

        // Nothing written after the ending is work.
        let after = writer
            .update_state_at(9_000, |s| s.result = Some("the tests pass now".to_string()))
            .unwrap();
        assert_eq!(after.worked, 10);
    }

    #[test]
    fn store_counts_a_span_of_work_nothing_ever_closed() {
        // The record adds up at the write that moves the phase, so an agent the
        // pane went out from under is still on the record as working with its
        // last span open. Whoever asks says how far to count it.
        let working = State {
            state: Phase::Working,
            since: 1_200,
            worked: 20,
            ..State::default()
        };
        assert_eq!(working.worked_by(1_300), 120);

        // A phase that is not work has nothing open to count.
        let waiting = State {
            state: Phase::Waiting,
            since: 1_200,
            worked: 20,
            ..State::default()
        };
        assert_eq!(waiting.worked_by(9_000), 20);

        // And a document with no moment to date a span from is one amx cannot
        // say worked at all.
        let undated = State {
            state: Phase::Working,
            ..State::default()
        };
        assert_eq!(undated.worked_by(9_000), 0);
    }

    #[test]
    fn store_moves_the_clock_when_the_phase_moves_and_not_before() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        let working = writer
            .update_state(|s| {
                s.state = Phase::Working;
                s.summary = Some("Reading src/main.rs".to_string());
            })
            .unwrap();
        assert_eq!(working.state, Phase::Working);
        assert!(working.since > 0);
        assert_eq!(working.last_event, working.since);

        // A second event in the same phase leaves the clock where it was.
        let still = writer
            .update_state(|s| s.summary = Some("Editing src/cli.rs".to_string()))
            .unwrap();
        assert_eq!(still.since, working.since);
        assert_eq!(still.summary.as_deref(), Some("Editing src/cli.rs"));

        assert_eq!(agent.state().unwrap(), still);
    }

    /// The document on disk, as a reader that is not amx would find it.
    fn written(agent: &Agent) -> serde_json::Value {
        let text = std::fs::read_to_string(agent.dir().join(STATE)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn store_writes_a_question_with_the_answers_it_offers() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Waiting;
                s.asks(Some("Do you want to proceed?".to_string()));
                s.options = vec!["Yes".to_string(), "No".to_string()];
            })
            .unwrap();

        let document = written(&agent);
        assert_eq!(document["question"]["text"], "Do you want to proceed?");
        assert_eq!(document["question"]["options"][1], "No");

        let read = agent.state().unwrap();
        assert_eq!(read.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(read.options, ["Yes", "No"]);
    }

    #[test]
    fn store_writes_a_question_it_knows_nothing_about_as_its_words() {
        // Everything amx knows beyond the words comes off the screen, and the
        // screen is not read until the hooks go quiet. A question that has
        // only just been asked is its words, and that is how it is written.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| s.asks(Some("Claude needs your permission".to_string())))
            .unwrap();

        assert_eq!(written(&agent)["question"], "Claude needs your permission");
        assert_eq!(
            agent.state().unwrap().question.as_deref(),
            Some("Claude needs your permission")
        );
    }

    #[test]
    fn store_leaves_no_options_behind_when_a_question_is_answered() {
        // `answer` clears the question and nothing else. Options with no
        // question over them are somebody else's answers, and the document has
        // no place to put them.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();
        writer
            .update_state(|s| {
                s.asks(Some("Do you want to proceed?".to_string()));
                s.options = vec!["Yes".to_string(), "No".to_string()];
            })
            .unwrap();

        writer.update_state(|s| s.question = None).unwrap();
        assert_eq!(written(&agent)["question"], serde_json::Value::Null);
        assert!(agent.state().unwrap().options.is_empty());
    }

    #[test]
    fn store_writes_the_kind_of_thing_a_question_is() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Waiting;
                s.asks(Some("Which fixture should the port keep?".to_string()));
                s.options = vec!["the sqlite one".to_string(), "the docker one".to_string()];
                s.kind = Some(Kind::Question);
            })
            .unwrap();

        let document = written(&agent);
        assert_eq!(document["question"]["kind"], "question");
        assert_eq!(
            document["question"]["text"],
            "Which fixture should the port keep?"
        );
        assert_eq!(agent.state().unwrap().kind, Some(Kind::Question));
    }

    /// A choice with the sentence the screen draws under it.
    fn choice(label: &str, description: &str) -> Choice {
        Choice {
            label: label.to_string(),
            description: Some(description.to_string()),
            preview: None,
        }
    }

    /// The three-question call measured against claude 2.1.240 on 2026-08-24,
    /// as `docs/question-shapes.md` records its payload: two questions taking
    /// one choice each and a third taking several.
    fn a_call_of_three() -> Vec<Ask> {
        vec![
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
                header: Some("Storage".to_string()),
                text: "Which store should hold sessions?".to_string(),
                options: vec![
                    choice("Redis", "Fast, volatile"),
                    choice("Postgres", "Durable, already deployed"),
                ],
                multi: false,
                answer: None,
            },
            Ask {
                header: Some("Rollout".to_string()),
                text: "Which rollout steps should run?".to_string(),
                options: vec![
                    choice("Canary", "Five percent first"),
                    choice("Migrate", "Run the schema change"),
                    choice("Announce", "Post to the channel"),
                ],
                multi: true,
                answer: None,
            },
        ]
    }

    #[test]
    fn store_writes_every_question_of_a_call_that_asks_several() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Waiting;
                s.asks_all(a_call_of_three());
                s.kind = Some(Kind::Question);
            })
            .unwrap();

        // The question showing is where a question has always been.
        let document = written(&agent);
        assert_eq!(
            document["question"]["text"],
            "Which runtime should the service target?"
        );
        assert_eq!(document["question"]["options"][0], "Node");
        assert_eq!(document["question"]["kind"], "question");

        // And the rest of the call is under it: what no screen carries.
        let asking = &document["question"]["asking"];
        assert_eq!(asking.as_array().unwrap().len(), 3);
        assert_eq!(asking[1]["header"], "Storage");
        assert_eq!(
            asking[0]["options"][0]["description"],
            "Widest library support"
        );
        assert_eq!(asking[0]["multi"], false);
        assert_eq!(asking[2]["multi"], true);

        let read = agent.state().unwrap();
        assert_eq!(read.asking, a_call_of_three());
        assert_eq!(
            read.question.as_deref(),
            Some("Which runtime should the service target?")
        );
        assert_eq!(read.options, ["Node", "Deno"]);
        assert!(!read.multi(), "and the one showing takes one choice");
    }

    #[test]
    fn store_answering_one_question_leaves_the_next_one_pending() {
        // Measured against 2.1.240: answering a tab does not return the vendor
        // to its composer. It records the answer, moves to the tab after it,
        // and the prompt is still up.
        let mut state = State {
            state: Phase::Waiting,
            kind: Some(Kind::Question),
            ..State::default()
        };
        state.asks_all(a_call_of_three());

        state.answered("Node");
        assert_eq!(
            state.question.as_deref(),
            Some("Which store should hold sessions?")
        );
        assert_eq!(state.options, ["Redis", "Postgres"]);
        assert_eq!(state.asking[0].answer.as_deref(), Some("Node"));
        assert_eq!(state.kind, Some(Kind::Question), "the prompt is still up");

        state.answered("Redis");
        assert_eq!(
            state.question.as_deref(),
            Some("Which rollout steps should run?")
        );
        assert!(state.multi(), "and this one takes more than one choice");

        // Every question answered: nothing left to ask, every answer on the
        // record, and the vendor's own Submit tab on the screen.
        state.answered("Canary, Announce");
        assert_eq!(state.pending(), None);
        assert_eq!(state.question, None);
        assert!(state.options.is_empty());
        assert!(!state.multi());
        let said: Vec<_> = state
            .asking
            .iter()
            .filter_map(|ask| ask.answer.as_deref())
            .collect();
        assert_eq!(said, ["Node", "Redis", "Canary, Announce"]);

        // With nothing pending there is nothing an answer belongs to.
        state.answered("late");
        assert_eq!(state.asking[2].answer.as_deref(), Some("Canary, Announce"));
    }

    #[test]
    fn store_leaves_no_call_behind_when_its_question_is_over() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();
        writer
            .update_state(|s| {
                s.state = Phase::Waiting;
                s.asks_all(a_call_of_three());
                s.kind = Some(Kind::Question);
            })
            .unwrap();

        // The questions of a call belong to the call. Once nothing is
        // outstanding they are answers nobody can give to a screen that is
        // gone, exactly as the choices under one question are.
        writer.update_state(|s| s.asks(None)).unwrap();
        assert_eq!(written(&agent)["question"], serde_json::Value::Null);
        assert!(agent.state().unwrap().asking.is_empty());
    }

    #[test]
    fn store_keeps_the_preview_that_puts_a_notes_field_on_a_question() {
        // Measured against 2.1.240 on 2026-08-24: the vendor draws the notes
        // field when a choice carries a preview, and `n` on a menu without one
        // does nothing. Nothing on the screen says which sort it is looking
        // at, so the record has to.
        let previewed = Ask {
            header: Some("Layout".to_string()),
            text: "Which header layout should the page use?".to_string(),
            options: vec![Choice {
                label: "Stacked".to_string(),
                description: Some("Title over subtitle".to_string()),
                preview: Some("+----------+\n| TITLE    |\n+----------+".to_string()),
            }],
            multi: false,
            answer: None,
        };
        assert!(previewed.takes_notes());
        assert!(!a_call_of_three()[0].takes_notes());

        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Waiting;
                s.asks_all(vec![previewed.clone()]);
                s.kind = Some(Kind::Question);
            })
            .unwrap();

        assert_eq!(agent.state().unwrap().asking, [previewed]);
    }

    #[test]
    fn store_writes_a_kind_it_has_no_words_for() {
        // What kind of thing is outstanding is worth having even where the
        // words are not to be had, and the choices under a question amx cannot
        // quote are nobody's to press.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Waiting;
                s.options = vec!["Yes".to_string(), "No".to_string()];
                s.kind = Some(Kind::Permission);
            })
            .unwrap();

        let document = written(&agent);
        assert_eq!(document["question"]["kind"], "permission");
        assert_eq!(document["question"]["text"], serde_json::Value::Null);
        assert_eq!(document["question"]["options"], serde_json::Value::Null);

        let read = agent.state().unwrap();
        assert_eq!(read.kind, Some(Kind::Permission));
        assert_eq!(read.question, None);
        assert!(read.options.is_empty());
    }

    #[test]
    fn store_lets_a_question_that_is_over_take_its_kind_with_it() {
        let mut state = State {
            state: Phase::Waiting,
            question: Some("Do you want to proceed?".to_string()),
            options: vec!["Yes".to_string(), "No".to_string()],
            kind: Some(Kind::Permission),
            ..State::default()
        };

        // Words arriving for the question already outstanding are only words:
        // the hook that carries them says nothing about what kind of screen
        // they are on, and it is the same screen.
        state.asks(Some("Claude needs your permission to use Bash".to_string()));
        assert_eq!(state.kind, Some(Kind::Permission));

        state.asks(None);
        assert_eq!(state.kind, None, "and nothing outstanding has a kind");
    }

    #[test]
    fn store_takes_what_a_screen_saw_without_correcting_a_hook() {
        let hooked = State {
            question: Some("Claude needs your permission to use Bash".to_string()),
            ..State::default()
        };
        let seen = Question {
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
        };

        let mut state = hooked.clone();
        assert!(state.learns_from(&seen), "the options are news");
        state.learn(&seen);
        assert_eq!(
            state.question, hooked.question,
            "the vendor's own words stand"
        );
        assert_eq!(state.options, ["Yes", "No"]);
        assert!(
            !state.learns_from(&seen),
            "and looking again learns nothing"
        );

        // With nothing on the record the screen is all there is.
        let mut nothing_heard = State::default();
        nothing_heard.learn(&seen);
        assert_eq!(
            nothing_heard.question.as_deref(),
            Some("Do you want to proceed?")
        );
    }

    #[test]
    fn store_a_reader_that_learned_nothing_writes_nothing() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();
        let heard = writer.update_state(|s| s.state = Phase::Waiting).unwrap();

        let looked = writer.observe(|s| s.learn(&Question::default())).unwrap();
        assert_eq!(looked, heard, "including the clock");

        // And what it does learn is written without claiming to have heard it.
        let seen = Question {
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string()],
        };
        let noted = writer.observe(|s| s.learn(&seen)).unwrap();
        assert_eq!(noted.question.as_deref(), Some("Do you want to proceed?"));
        assert_eq!(
            noted.last_event, heard.last_event,
            "a screen is not a thing the agent said"
        );
        assert_eq!(agent.state().unwrap(), noted);
    }

    #[test]
    fn acts_a_record_keeps_when_somebody_last_looked_at_the_agent() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();
        let ended = writer
            .update_state(|s| {
                s.state = Phase::Done;
                s.result = Some("wrote the parser".to_string());
            })
            .unwrap();

        let looked = writer.observe(|s| s.seen = now()).unwrap();
        assert!(looked.seen >= ended.last_event);
        assert_eq!(
            looked.last_event, ended.last_event,
            "a look is not something the agent said"
        );
        assert_eq!(written(&agent)["seen"], looked.seen);

        // A document from before anybody could be said to have looked is one
        // nobody has looked at.
        std::fs::write(agent.dir().join(STATE), r#"{"state":"done"}"#).unwrap();
        assert_eq!(agent.state().unwrap().seen, 0);
    }

    #[test]
    fn acts_a_record_keeps_what_a_person_renamed_an_agent_to() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .observe(|s| s.name = Some("auth".to_string()))
            .unwrap();

        assert_eq!(written(&agent)["name"], "auth");
        assert_eq!(agent.state().unwrap().name.as_deref(), Some("auth"));
        assert_eq!(
            agent.meta().unwrap().id,
            "fix-login-a1b",
            "and the id the record is filed under is where it was"
        );

        // A document written before anybody could rename anything is a record
        // of an agent nobody has renamed.
        std::fs::write(agent.dir().join(STATE), r#"{"state":"idle"}"#).unwrap();
        assert_eq!(agent.state().unwrap().name, None);
    }

    #[test]
    fn store_records_a_terminal_state_with_its_exit_code() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .update_state(|s| {
                s.state = Phase::Failed;
                s.exit = Some(2);
                s.result = Some("could not reach the api".to_string());
                s.source = Some(Source::Payload);
            })
            .unwrap();

        let read = agent.state().unwrap();
        assert!(read.state.is_terminal());
        assert_eq!(read.exit, Some(2));
        assert_eq!(read.source, Some(Source::Payload));
        assert!(!Phase::Working.is_terminal());
    }

    #[test]
    fn store_learns_the_session_a_hook_reports() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();

        agent
            .writer()
            .unwrap()
            .update_meta(|m| {
                m.session = Some("abc-123".to_string());
                m.transcript = Some(PathBuf::from("/home/dev/.claude/projects/x/abc-123.jsonl"));
            })
            .unwrap();

        let read = agent.meta().unwrap();
        assert_eq!(read.session.as_deref(), Some("abc-123"));
        assert_eq!(read.task, "fix the login bug", "the rest is untouched");
    }

    #[test]
    fn store_reads_a_record_written_by_an_older_amx() {
        // Fields are added, never renamed: a document from before a field
        // existed still reads, with the default in its place.
        let root = TempDir::new().unwrap();
        let dir = root.path().join("fix-login-a1b");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(META),
            r#"{"id":"fix-login-a1b","task":"t","dir":"/srv/app",
                "socket":{"name":"amx"},"pane":"%7","created":1}"#,
        )
        .unwrap();
        std::fs::write(dir.join(STATE), r#"{"state":"working","seq":3}"#).unwrap();

        let agent = Agent::open(root.path(), "fix-login-a1b").unwrap();
        let meta = agent.meta().unwrap();
        assert_eq!(meta.pane, PaneId::new("%7").unwrap());
        assert_eq!(meta.worktree, None);

        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.seq, 3);
        assert_eq!(state.question, None);
        assert_eq!(state.ended, 0, "and no run of it has ended");
        assert_eq!(state.worked, 0, "and no span of work is added up on it");

        // A question written before amx could read the choices under it is a
        // string, and a string is still a question.
        std::fs::write(
            dir.join(STATE),
            r#"{"state":"waiting","question":"Do you want to proceed?"}"#,
        )
        .unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.question.as_deref(), Some("Do you want to proceed?"));
        assert!(state.options.is_empty());
        assert!(state.asking.is_empty());

        // And a question written before amx held the call it came from is one
        // question, with nothing behind it.
        std::fs::write(
            dir.join(STATE),
            r#"{"state":"waiting","question":{"text":"Do you want to proceed?",
                "options":["Yes","No"],"kind":"question"}}"#,
        )
        .unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.options, ["Yes", "No"]);
        assert_eq!(state.kind, Some(Kind::Question));
        assert!(state.asking.is_empty());
        assert_eq!(state.pending(), None);
        assert!(!state.multi());
    }

    #[test]
    fn store_keeps_events_in_the_order_they_arrived() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        writer
            .append(&Event::new(
                "SessionStart",
                serde_json::json!({"session_id": "a"}),
            ))
            .unwrap();
        writer
            .append(&Event::new(
                "UserPromptSubmit",
                serde_json::json!({"prompt": "go"}),
            ))
            .unwrap();

        let events = agent.events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "SessionStart");
        assert_eq!(events[1].payload["prompt"], "go");
        assert!(events[0].at > 0);
    }

    #[test]
    fn store_survives_a_damaged_line_without_losing_the_rest() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent
            .writer()
            .unwrap()
            .append(&Event::new("Stop", serde_json::json!({})))
            .unwrap();
        // A machine that died mid-write leaves a partial last line.
        let mut file = OpenOptions::new()
            .append(true)
            .open(agent.dir().join(EVENTS))
            .unwrap();
        file.write_all(b"{\"at\":1,\"kind\":\"Stop\"").unwrap();

        let events = agent.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "Stop");
    }

    #[test]
    fn store_never_interleaves_two_appenders() {
        let root = TempDir::new().unwrap();
        let agent = Arc::new(Agent::create(root.path(), &meta("fix-login-a1b")).unwrap());

        // Payloads big enough that a naive write would be split by another.
        let filler = "x".repeat(4096);
        let writers = 8;
        let each = 25;

        std::thread::scope(|scope| {
            for writer in 0..writers {
                let agent = Arc::clone(&agent);
                let filler = filler.clone();
                scope.spawn(move || {
                    for n in 0..each {
                        let w = agent.writer().unwrap();
                        w.append(&Event::new(
                            format!("hook-{writer}"),
                            serde_json::json!({"n": n, "filler": filler}),
                        ))
                        .unwrap();
                    }
                });
            }
        });

        let text = std::fs::read_to_string(agent.dir().join(EVENTS)).unwrap();
        assert_eq!(
            text.lines().count(),
            writers * each,
            "every record is exactly one line"
        );
        for line in text.lines() {
            serde_json::from_str::<Event>(line).expect("a whole record per line");
        }
        assert_eq!(agent.events().unwrap().len(), writers * each);
    }

    #[test]
    fn store_two_writers_never_lose_each_others_work() {
        // The read-modify-write the lock exists for: two hooks firing at once
        // both read the state, both change it, and the second must not write
        // over what the first decided.
        let root = TempDir::new().unwrap();
        let agent = Arc::new(Agent::create(root.path(), &meta("fix-login-a1b")).unwrap());
        let writers = 8;
        let each = 20;

        std::thread::scope(|scope| {
            for _ in 0..writers {
                let agent = Arc::clone(&agent);
                scope.spawn(move || {
                    for _ in 0..each {
                        agent
                            .writer()
                            .unwrap()
                            .update_state(|s| s.seq += 1)
                            .unwrap();
                    }
                });
            }
        });

        assert_eq!(agent.state().unwrap().seq, writers * each);
    }

    #[test]
    fn store_readers_never_see_half_a_document() {
        let root = TempDir::new().unwrap();
        let agent = Arc::new(Agent::create(root.path(), &meta("fix-login-a1b")).unwrap());
        let done = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            let writing = {
                let agent = Arc::clone(&agent);
                let done = Arc::clone(&done);
                scope.spawn(move || {
                    let writer = agent.writer().unwrap();
                    for n in 0..200 {
                        writer
                            .update_state(|s| {
                                s.seq = n;
                                s.summary = Some("y".repeat(8192));
                            })
                            .unwrap();
                    }
                    done.store(true, Ordering::Release);
                })
            };

            // A reader takes no lock, so it reads straight through the writes.
            let mut reads = 0;
            while !done.load(Ordering::Acquire) {
                agent
                    .state()
                    .expect("a reader always sees a whole document");
                reads += 1;
            }
            writing.join().unwrap();
            assert!(reads > 0, "the reader must have overlapped the writer");
        });

        assert_eq!(agent.state().unwrap().seq, 199);
    }

    #[test]
    fn store_lists_records_and_ignores_what_is_not_one() {
        let root = TempDir::new().unwrap();
        Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        Agent::create(root.path(), &meta("port-importer-c3d")).unwrap();
        std::fs::write(root.path().join("stray.txt"), "not an agent").unwrap();
        std::fs::create_dir(root.path().join("half-made")).unwrap();

        let mut ids = list(root.path()).unwrap();
        ids.sort();
        assert_eq!(ids, ["fix-login-a1b", "port-importer-c3d"]);
    }

    #[test]
    fn store_removes_a_record_whole() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        agent.remove().unwrap();

        assert!(!agent.dir().exists());
        assert!(list(root.path()).unwrap().is_empty());
        assert!(Agent::open(root.path(), "fix-login-a1b").is_err());
    }

    #[test]
    fn store_writes_a_document_by_replacing_it() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("doc.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let left_behind: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left_behind, ["doc.json"], "no temporary file survives");
    }
}
