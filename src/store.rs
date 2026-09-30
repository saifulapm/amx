//! The per-agent record on disk.
//!
//! Each agent has a directory `<state root>/<id>/` holding:
//!
//! - `meta.json`: how the agent was started and where to reach it.
//! - `state.json`: what it is doing, as the last event left it.
//! - `events.jsonl`: one line per event, in arrival order.
//!
//! Invariants:
//!
//! - Every mutation goes through a [`Writer`], which holds an exclusive
//!   `flock` for its lifetime, so concurrent hooks never lose writes or split
//!   a line.
//! - Readers never lock. Documents are written to a temporary file and renamed
//!   into place, so a reader sees the old or the new document, never a mix. A
//!   reader takes the lock only to record something new it read off a pane
//!   (see [`Writer::observe`]).
//! - Renames are not fsynced, so a crash can lose the last write. Readers that
//!   find the state stale fall back to the pane.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;
use crate::tmux::{PaneId, Socket};

pub(crate) const META: &str = "meta.json";
const STATE: &str = "state.json";
const EVENTS: &str = "events.jsonl";
const LOCK: &str = "lock";
const SPAWN_LOCK: &str = "spawn.lock";
const CLAIM: &str = "claim";
/// Text the vendor is streaming during a turn, removed when the turn ends.
/// Written by the vendor's reporter, only read here.
pub const LIVE: &str = "live";
/// Heartbeat file whose mtime a vendor's reporter refreshes every few seconds
/// while a turn runs, removed when it ends. Written by the vendor side, only
/// read here; see [`Agent::heartbeat`].
pub const HEARTBEAT: &str = "heartbeat";
/// Stamped by a reader each time it sees a hook-reporting vendor's pane show a
/// running turn; see [`Agent::seen`]. A turn end no hook reported (Esc
/// mid-tool) counts work up to this stamp.
pub const SEEN: &str = "seen";
/// The pane's output, piped here by tmux (see [`crate::spawn::boot`]): a
/// command's whole output, or the first [`crate::spawn::BOOT_BYTES`] of an
/// agent's pane. See [`Agent::output`].
pub const OUTPUT: &str = "output";
/// How much of a transcript's end [`Agent::transcript_tail`] reads.
const TAIL: u64 = 64 * 1024;
/// How much of a command's output [`Agent::output_tail`] reads: about three
/// thousand 80-column rows, more than a card ever pages through.
pub const OUTPUT_TAIL: u64 = 256 * 1024;

/// An agent's phase as the hooks and exit record report it.
///
/// Readers weigh this against its age and against the pane itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Spawned, nothing heard yet.
    #[default]
    Starting,
    /// Working on a turn.
    Working,
    /// Stopped on a question.
    Waiting,
    /// The turn ended; the agent is at its prompt.
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
    /// Whether nothing more will come from this agent.
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

    /// The word shown for this phase in rows and tables.
    ///
    /// `Idle` reads `done`, like the header's counter: an ended turn is done
    /// whether or not the process remains, and the glyph shows which. The
    /// record still holds `idle`, which `amx wait --for idle` and `on_idle`'s
    /// `AMX_STATE` match on.
    pub fn word(self) -> &'static str {
        match self {
            Phase::Idle => "done",
            phase => phase.as_str(),
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a result came from. The transcript is written asynchronously, so the
/// source says how far to trust the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The turn-end hook's payload, the freshest source.
    Payload,
    /// The tail of the session transcript.
    Transcript,
    /// The last screenful of the pane.
    Screen,
}

/// A question an agent stopped on, with the choices it offers.
///
/// The text is the vendor's own when a hook carried it, otherwise amx's
/// reading of the pane. The options always come from the pane: no hook
/// carries them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Question {
    /// What is being asked.
    pub text: String,
    /// The numbered choices, in screen order. Empty until the screen is read,
    /// and for a question answered with free text.
    pub options: Vec<String>,
    /// Whether the choices were read off the cursor mark. False when the
    /// vendor drew its own numbers.
    ///
    /// A walked list's numbers are amx's own (see `rules::Rule::marks`), so a
    /// choice is taken by moving the cursor, not by typing its digit.
    pub walked: bool,
    /// The 1-based choice the cursor mark was on, for a walked list; `None`
    /// otherwise. A walk starts here: since claude 2.1.276 the trust gate's
    /// list wraps at both ends (2.1.259 clamped), so walking to the top first
    /// lands wherever the cursor was.
    pub marked: Option<usize>,
}

/// The kind of prompt an agent is stopped on.
///
/// Each takes a different answer: a permission box allows or refuses one tool
/// call, an AskUserQuestion menu takes a choice or free text, and the
/// folder-trust screen decides whether the vendor may start in the directory.
///
/// Only set from an explicit signal: a permission hook, the vendor's
/// notification type, the question tool, or the screen rule that matched.
/// Never inferred from the question's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A tool call waiting to be allowed or refused.
    Permission,
    /// The vendor's own question, answered with a choice or text.
    Question,
    /// The folder-trust screen shown before a session starts.
    Trust,
}

/// One choice under a question, as the vendor wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Choice {
    /// The label an answer names and gets back.
    pub label: String,
    /// The sentence drawn under the label.
    pub description: Option<String>,
    /// The block drawn beside the choice in place of descriptions. Any
    /// preview turns on the notes field, and the chosen one is returned with
    /// the note.
    pub preview: Option<String>,
}

/// One question of an `AskUserQuestion` call.
///
/// A call holds up to four questions shown as tabs on one screen. Their count,
/// headers, descriptions and multi-select flags exist only in the payload: the
/// tab strip elides headers as the pane narrows (claude 2.1.240; see
/// `docs/question-shapes.md`). So this is built from the payload, never from
/// the screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Ask {
    /// The short tab label.
    pub header: Option<String>,
    /// What is being asked.
    pub text: String,
    /// The choices, in screen order.
    pub options: Vec<Choice>,
    /// Whether it takes several choices. Set per question: one call can mix
    /// checkbox and single-choice tabs, driven by different keys.
    pub multi: bool,
    /// The answer sent, once answered. Answering one question moves the vendor
    /// to the next tab without ending the call, so each answer is kept on its
    /// own question.
    pub answer: Option<String>,
}

impl Ask {
    /// The choice labels.
    pub fn labels(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| option.label.clone())
            .collect()
    }

    /// Whether the vendor shows a notes field: only when a choice has a
    /// preview. Otherwise `n` does nothing.
    pub fn takes_notes(&self) -> bool {
        self.options.iter().any(|option| option.preview.is_some())
    }
}

/// How the agent was started, and how to reach it.
///
/// Fields are only ever added, never renamed or removed, and every field that
/// may be missing has a default, so records from older builds still read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub task: String,
    /// What runs the agent: the command `new` resolved, the parent's command
    /// for a fork, or the vendor an adoption found. `None` for a shell command
    /// and for records written before this field existed.
    #[serde(default)]
    pub agent: Option<String>,
    /// The model and effort flags the spawn passed. `None` when no flag was
    /// passed (the vendor's own default is unknown to amx), for a shell
    /// command, and for an adopted agent.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// The parent `amx sub` recorded, if any. `None` is a root: `amx new`,
    /// `sub --no-parent`, or an older record.
    ///
    /// A child whose parent record has been removed (`stop --delete`, `clear`,
    /// `sweep`) reads as a root.
    #[serde(default)]
    pub parent: Option<String>,
    /// Depth in the family: 0 for a root, parent's depth plus one otherwise.
    /// Reads as 0 from older records.
    #[serde(default)]
    pub depth: u32,
    /// The role the spawn named, whose file supplied its dials and brief.
    /// `None` when none was named, for a shell command, and for older records.
    #[serde(default)]
    pub role: Option<String>,
    /// Where the agent runs: its worktree, or the directory it was given.
    pub dir: PathBuf,
    /// The worktree amx made for it, if any.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    #[serde(default)]
    pub branch: Option<String>,
    /// The commit the worktree started from, for diffs.
    #[serde(default)]
    pub base: Option<String>,
    /// The tmux server the pane lives on. Every tmux call for this agent must
    /// target it, whichever server the caller is inside.
    pub socket: Socket,
    pub pane: PaneId,
    /// Started in the background, off the wall. `resume` keeps it there.
    #[serde(default)]
    pub bg: bool,
    /// The vendor's session id, learned from a hook. `resume` passes it back.
    #[serde(default)]
    pub session: Option<String>,
    /// The transcript that session writes.
    #[serde(default)]
    pub transcript: Option<PathBuf>,
    /// Epoch seconds.
    pub created: u64,
}

impl Meta {
    /// The agent's worktree while it still exists, else its directory.
    pub fn workdir(&self) -> &Path {
        match &self.worktree {
            Some(tree) if tree.is_dir() => tree,
            _ => &self.dir,
        }
    }
}

/// What the agent is doing, as the last event left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, from = "Wire", into = "Wire")]
pub struct State {
    pub state: Phase,
    /// A display name set by a rename. The id never changes; this only
    /// changes what the wall shows. Kept here, not in [`Meta`], because it is
    /// written while the agent runs.
    pub name: Option<String>,
    /// The vendor's title for the session, read from the transcript (see
    /// [`crate::conversation::session_title`]). Distinct from
    /// [`name`](State::name), which is set in amx.
    pub session_title: Option<String>,
    /// Bumped by each `send`, so `result` can tell this turn's end from the
    /// previous one.
    pub seq: u64,
    /// Epoch seconds when the agent entered this phase.
    pub since: u64,
    /// One line about what it is doing.
    pub summary: Option<String>,
    /// Background tasks still running when the turn ended.
    ///
    /// claude can end a turn with shells it started still running and lists
    /// them in the turn-end payload. A nonzero count keeps the agent working
    /// against the idle nudge that follows and the prompt rule on the pane;
    /// see [`crate::hook::apply`] and [`crate::derive::read`]. Cleared by the
    /// agent's next action.
    pub background: u32,
    /// The question it is waiting on.
    pub question: Option<String>,
    /// The choices that question offers, in screen order. They belong to the
    /// question and are never kept without one.
    pub options: Vec<String>,
    /// Whether the choices were read off a cursor mark (see
    /// [`Question::walked`]). Travels with the choices.
    pub walked: bool,
    /// The whole `AskUserQuestion` call: every question, its choices'
    /// descriptions, and which are answered. The question on screen is also
    /// mirrored in `question` and `options`.
    pub asking: Vec<Ask>,
    /// The kind of prompt, where something said so. Can be known without the
    /// words: an unreadable menu is still a menu.
    pub kind: Option<Kind>,
    /// Whether a vendor hook reported the question. False when amx read it
    /// off the pane.
    ///
    /// Selects how the next pane reading applies: [`learn`](State::learn) for
    /// a reported question, [`correct`](State::correct) otherwise. It is kept
    /// per question because it cannot be derived from the vendor: pi reports
    /// through an extension yet draws some screens (`/login`, `/trust`,
    /// `/model`, the startup trust gate) without reporting them.
    ///
    /// Travels with the question; false when nothing is outstanding.
    pub reported: bool,
    /// The answer from the last turn that ended.
    pub result: Option<String>,
    /// Where that answer came from.
    pub source: Option<Source>,
    /// The exit code of the agent's command, once it has one.
    pub exit: Option<i32>,
    /// Epoch seconds of the last recorded event. Readers use it to decide
    /// whether to trust this document over the pane.
    pub last_event: u64,
    /// Epoch seconds when the run ended, 0 while it runs. A finished row shows
    /// how long the run took, so the end is kept separately from `since`.
    pub ended: u64,
    /// Epoch seconds when amx released this agent's pane (see `amx _park`), 0
    /// otherwise. Tells a parked pane, which comes back on the next enter,
    /// attach or resume, from one that was killed.
    pub parked_at: u64,
    /// Epoch seconds when `amx interrupt` cut the turn short, 0 otherwise.
    ///
    /// claude fires no hook on an interrupt, so without this the record would
    /// read as working (see [`crate::derive::read`]). The vendor's next event
    /// clears it. Written with [`Writer::observe`], like `parked_at`.
    pub interrupted_at: u64,
    /// Epoch seconds when someone last opened this agent, 0 if never. Compared
    /// with `last_event` to tell unseen news.
    pub seen: u64,
    /// Seconds spent working, summed over every working span.
    ///
    /// Wall time over a run includes time waiting on answers. Each span is
    /// added once, by the write that moves the phase out of `working`.
    pub worked: u64,
    /// The screen the last reader saw on the pane and when it first appeared.
    /// `None` until something has looked.
    pub still: Option<Still>,
    /// Set by `amx resume <id>` with no message: the session will open idle.
    ///
    /// Every other boot has a first turn, so `starting` lasts only until the
    /// vendor reports work. A bare resume opens at the prompt and reports only
    /// the session start, and the pane may not be readable (a pi with a
    /// custom footer). [`crate::hook::apply`] consumes this on session start.
    pub opens_idle: bool,
    /// Whether the vendor reported a turn start and not yet its end.
    ///
    /// The phase cannot say this: a turn moves between working and waiting,
    /// and pi can raise a dialog outside any turn. See [`crate::hook::apply`],
    /// where a refusal returns to working inside a turn and to idle outside
    /// one.
    pub turn_open: bool,
    /// Sends an interrupt put back into the vendor's composer, oldest first.
    ///
    /// pi restores queued messages on cancel, where claude drops them (see
    /// [`crate::vendor::Vendor::restores_queued_on_cancel`]). The text sits
    /// unsubmitted, so a paste would be sent along with it, and `send` refuses
    /// until the vendor's next prompt clears it. Written with
    /// [`Writer::observe`].
    pub composer_holds: Vec<String>,
}

/// A screen a reader saw, and when it first appeared.
///
/// Kept on the record because readers are usually short-lived processes: how
/// long a screen has held still (see [`crate::rules::SETTLED_LOOKS`]) must
/// survive across `amx ls` invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Still {
    /// Hash of the screen above the vendor's chrome. Only compared for
    /// equality, so each agent costs one number.
    pub screen: u64,
    /// Epoch seconds when a reader first saw this screen. Unchanged while the
    /// screen stays the same.
    pub since: u64,
}

impl State {
    /// Set the question the agent is asking, from a hook.
    ///
    /// Clears the options and the call, which belong to the previous question.
    /// The kind is cleared only when the question is: new words for a question
    /// already outstanding describe the same screen. Marks the question as
    /// [`reported`](State::reported).
    pub fn asks(&mut self, question: Option<String>) {
        self.question = question;
        self.options.clear();
        self.walked = false;
        self.asking.clear();
        self.reported = self.question.is_some();
        if self.question.is_none() {
            self.kind = None;
        }
    }

    /// Set every question of an `AskUserQuestion` call, from a hook.
    ///
    /// The first unanswered question is mirrored into `question` and
    /// `options`, where readers look for the question on screen.
    pub fn asks_all(&mut self, asking: Vec<Ask>) {
        self.asking = asking;
        self.shows_the_pending_one();
    }

    /// Record the answer to the question on screen and show the next one.
    ///
    /// Answering one question moves the vendor to the next tab without ending
    /// the call. Once every question has an answer, nothing is left to ask and
    /// the answers stay on the record, as the vendor's Submit tab shows them.
    pub fn answered(&mut self, answer: impl Into<String>) {
        if let Some(pending) = self.asking.iter_mut().find(|ask| ask.answer.is_none()) {
            pending.answer = Some(answer.into());
        }
        self.shows_the_pending_one();
    }

    /// The call's question on screen: the first without an answer.
    pub fn pending(&self) -> Option<&Ask> {
        self.asking.iter().find(|ask| ask.answer.is_none())
    }

    /// Whether the question on screen takes several choices.
    pub fn multi(&self) -> bool {
        self.pending().is_some_and(|ask| ask.multi)
    }

    /// Mirror the call's pending question into `question` and `options`.
    ///
    /// Only reached from the hook-driven [`asks_all`](State::asks_all) and
    /// [`answered`](State::answered), so it marks the question reported, as
    /// [`asks`](State::asks) does.
    fn shows_the_pending_one(&mut self) {
        let (text, options) = self
            .pending()
            .map(|ask| (ask.text.clone(), ask.labels()))
            .unzip();
        self.question = text;
        self.options = options.unwrap_or_default();
        self.walked = false;
        self.reported = self.question.is_some();
    }

    /// Seconds worked as of `at`, including a span still open.
    ///
    /// Spans are added when the phase leaves `working`, so a record left
    /// mid-turn (the pane died before anything wrote the phase out) has one
    /// span still open. The caller picks `at`. A working record with no
    /// `since` counts no open span.
    pub fn worked_by(&self, at: u64) -> u64 {
        let open = match (self.state, self.since) {
            (Phase::Working, since) if since > 0 => at.saturating_sub(since),
            _ => 0,
        };
        self.worked.saturating_add(open)
    }

    /// Whether a screen reading would add anything the record lacks. Checked
    /// before taking the writer lock.
    pub fn learns_from(&self, seen: &Question) -> bool {
        (self.question.is_none() && !seen.text.is_empty())
            || (self.options.is_empty() && !seen.options.is_empty())
    }

    /// Fill in what a screen reading adds, overwriting nothing.
    ///
    /// Used for a question a hook reported: the hook's words stand, and the
    /// screen only fills gaps (the options, which no hook carries, and the text
    /// when no hook gave one). Filling options keeps the question reported;
    /// taking the text marks it as read from the screen.
    pub fn learn(&mut self, seen: &Question) {
        if self.question.is_none() && !seen.text.is_empty() {
            self.question = Some(seen.text.clone());
            self.reported = false;
        }
        if self.options.is_empty() {
            self.options.clone_from(&seen.options);
            self.walked = seen.walked;
        }
    }

    /// Whether a screen reading would change what the record says. Checked
    /// before taking the writer lock, like [`learns_from`](State::learns_from).
    pub fn corrected_by(&self, seen: Option<&Question>) -> bool {
        let (text, options, walked) = match asked(seen) {
            Some(seen) => (
                Some(seen.text.as_str()),
                seen.options.as_slice(),
                seen.walked,
            ),
            None => (None, &[][..], false),
        };
        self.question.as_deref() != text || self.options != options || self.walked != walked
    }

    /// Replace the question with a screen reading.
    ///
    /// Used for a question no hook reported (see
    /// [`reported`](State::reported)): the record holds only an earlier
    /// reading of the same pane, and a pane shows one screen at a time. The
    /// question and its choices are replaced together, and a screen with
    /// nothing to answer clears them. Filling field by field, as
    /// [`learn`](State::learn) does, once left the trust selector's choices
    /// under the login box's question.
    pub fn correct(&mut self, seen: Option<&Question>) {
        let seen = asked(seen);
        self.question = seen.map(|seen| seen.text.clone());
        self.options = seen.map(|seen| seen.options.clone()).unwrap_or_default();
        self.walked = seen.is_some_and(|seen| seen.walked);
        self.reported = false;
    }

    /// The state a new session of the same agent starts from, for `resume`
    /// and `/clear`.
    ///
    /// The previous session's answer and exit code are dropped. The send
    /// count, the seconds worked and the display name are kept.
    pub fn for_a_new_session(&self) -> State {
        State {
            seq: self.seq,
            worked: self.worked,
            name: self.name.clone(),
            ..State::default()
        }
    }
}

/// The reading if it has a question. A reading with no text asked nothing,
/// whatever else it shows.
fn asked(seen: Option<&Question>) -> Option<&Question> {
    seen.filter(|seen| !seen.text.is_empty())
}

/// The on-disk shape of [`State`].
///
/// It differs in one place: the question text, its options and its call are
/// stored together under one `question` key, while in memory the text is its
/// own field because most of amx only needs the text. Options with no
/// question are not written, so an answered question never leaves its choices
/// behind.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Wire {
    #[serde(deserialize_with = "phase_or_unknown")]
    state: Phase,
    name: Option<String>,
    session_title: Option<String>,
    seq: u64,
    since: u64,
    summary: Option<String>,
    background: u32,
    question: Option<Asked>,
    result: Option<String>,
    source: Option<Source>,
    exit: Option<i32>,
    last_event: u64,
    ended: u64,
    parked_at: u64,
    interrupted_at: u64,
    seen: u64,
    worked: u64,
    still: Option<Still>,
    opens_idle: bool,
    turn_open: bool,
    composer_holds: Vec<String>,
}

/// A known phase, or [`Phase::Unknown`] for one this build does not know.
///
/// `state.json` may come from a newer build, and the rest of the document
/// should still read. [`Phase`] itself stays strict, because vendor screen
/// rules rely on it to catch typos in the phase a rule claims.
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

/// A stored question: its words alone, or everything known about it.
///
/// Anything beyond the words comes from reading the screen, so a question a
/// hook just reported is only words. The words-only shape therefore also
/// means "reported by the vendor"; a screen reading is always written whole
/// so it can say otherwise. Documents from before the `reported` field read
/// correctly under the same rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Asked {
    Words(String),
    Whole(Known),
}

/// Everything known about one question. Missing parts are omitted, so the
/// document never claims more than amx knows and older documents still read.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Known {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    /// Whether the choices were read off a cursor mark (see
    /// [`State::walked`]). Written only when true; absent means the vendor
    /// numbered the list, as in every older document.
    #[serde(skip_serializing_if = "is_not")]
    walked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<Kind>,
    /// The whole call. The question on screen and its choices are also
    /// written above, where existing readers look for them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    asking: Vec<Ask>,
    /// Whether the vendor reported this question (see [`State::reported`]).
    /// Written only when true: the whole shape without it is a screen
    /// reading, and a hook's question with nothing else is the words alone.
    #[serde(skip_serializing_if = "is_not")]
    reported: bool,
}

/// Skip writing a `false`.
fn is_not(said: &bool) -> bool {
    !said
}

impl From<State> for Wire {
    fn from(state: State) -> Wire {
        // Destructure every field so a new field cannot be silently left out
        // of the document.
        let State {
            state,
            name,
            session_title,
            seq,
            since,
            summary,
            background,
            question,
            options,
            walked,
            asking,
            kind,
            reported,
            result,
            source,
            exit,
            last_event,
            ended,
            parked_at,
            interrupted_at,
            seen,
            worked,
            still,
            opens_idle,
            turn_open,
            composer_holds,
        } = state;

        Wire {
            state,
            name,
            session_title,
            seq,
            since,
            summary,
            background,
            question: match (question, kind) {
                // Nothing outstanding. Options without a question are dropped.
                (None, None) => None,
                // The kind without the words: an unreadable menu, or a call
                // whose questions are all answered and awaiting submit. The
                // call is kept for its answers; stray options are dropped.
                (None, kind) => Some(Asked::Whole(Known {
                    kind,
                    asking,
                    ..Known::default()
                })),
                // A hook's question with nothing else is written as its words
                // alone, which is what marks it reported. A words-only screen
                // reading takes the whole shape below.
                (Some(text), None) if reported && options.is_empty() && asking.is_empty() => {
                    Some(Asked::Words(text))
                }
                (text, kind) => Some(Asked::Whole(Known {
                    text,
                    options,
                    walked,
                    kind,
                    asking,
                    reported,
                })),
            },
            result,
            source,
            exit,
            last_event,
            ended,
            parked_at,
            interrupted_at,
            seen,
            worked,
            still,
            opens_idle,
            turn_open,
            composer_holds,
        }
    }
}

impl From<Wire> for State {
    fn from(wire: Wire) -> State {
        let (question, options, walked, kind, asking, reported) = match wire.question {
            Some(Asked::Words(text)) => (Some(text), Vec::new(), false, None, Vec::new(), true),
            Some(Asked::Whole(asked)) => (
                asked.text,
                asked.options,
                asked.walked,
                asked.kind,
                asked.asking,
                asked.reported,
            ),
            None => (None, Vec::new(), false, None, Vec::new(), false),
        };

        State {
            state: wire.state,
            name: wire.name,
            session_title: wire.session_title,
            seq: wire.seq,
            since: wire.since,
            summary: wire.summary,
            background: wire.background,
            question,
            options,
            walked,
            asking,
            kind,
            reported,
            result: wire.result,
            source: wire.source,
            exit: wire.exit,
            last_event: wire.last_event,
            ended: wire.ended,
            parked_at: wire.parked_at,
            interrupted_at: wire.interrupted_at,
            seen: wire.seen,
            worked: wire.worked,
            still: wire.still,
            opens_idle: wire.opens_idle,
            turn_open: wire.turn_open,
            composer_holds: wire.composer_holds,
        }
    }
}

/// One recorded event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Epoch seconds when amx recorded it.
    pub at: u64,
    /// The vendor's hook event name, or amx's own name for what it did.
    pub kind: String,
    /// The payload as received.
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

/// Which edge of a turn an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Opens,
    Closes,
}

/// Seconds of work the log's turns add up to, each from its opening event to
/// its closing one. `None` if the log has no turn edges.
///
/// For records whose `worked` total was never kept. A second opening inside an
/// open turn is a message steered into it, not a new turn, and a turn never
/// closed counts nothing.
pub fn worked_in(events: &[Event], edge: impl Fn(&str) -> Option<Edge>) -> Option<u64> {
    let mut edged = false;
    let mut open: Option<u64> = None;
    let mut worked = 0u64;
    for event in events {
        match edge(&event.kind) {
            Some(Edge::Opens) => {
                edged = true;
                open.get_or_insert(event.at);
            }
            Some(Edge::Closes) => {
                edged = true;
                if let Some(at) = open.take() {
                    worked = worked.saturating_add(event.at.saturating_sub(at));
                }
            }
            None => {}
        }
    }
    edged.then_some(worked)
}

/// Epoch seconds. Every timestamp amx records is one of these.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A file's mtime in epoch seconds, or `None` if there is no file.
pub(crate) fn modified_at(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|file| file.modified())
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// One agent's directory.
#[derive(Debug, Clone)]
pub struct Agent {
    id: String,
    dir: PathBuf,
}

impl Agent {
    /// Make the directory and write the initial record.
    pub fn create(root: &Path, meta: &Meta) -> Result<Agent> {
        let dir = crate::paths::agent_dir_in(root, &meta.id)?;
        // `new` creates the directory and writes the pane's handoff into it
        // before there is a pane to record, so only `meta.json` marks a
        // record.
        if dir.join(META).exists() {
            bail!("agent `{}` already has a record", meta.id);
        }
        // The directory usually exists already, so set its mode explicitly.
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

    /// Open an agent whose directory exists.
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

    /// Text the vendor is streaming now (see [`LIVE`]). `None` between turns
    /// and for vendors that do not stream.
    pub fn live(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join(LIVE))
            .ok()
            .filter(|text| !text.trim().is_empty())
    }

    /// When the vendor's reporter last beat during a turn, in epoch seconds
    /// (see [`HEARTBEAT`]). `None` between turns and for vendors that do not
    /// beat.
    ///
    /// The beat is the file's mtime, so the reporter only has to touch it.
    pub fn heartbeat(&self) -> Option<u64> {
        modified_at(&self.dir.join(HEARTBEAT))
    }

    /// When a reader last saw this agent's pane show a running turn, in epoch
    /// seconds (see [`SEEN`]).
    pub fn seen(&self) -> Option<u64> {
        modified_at(&self.dir.join(SEEN))
    }

    /// Stamp [`SEEN`] at `at`. Takes no lock, like the vendor's heartbeat.
    pub fn saw_working(&self, at: u64) -> Result<()> {
        let file = File::create(self.dir.join(SEEN))?;
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(at))?;
        Ok(())
    }

    /// The whole of what the pane printed (see [`OUTPUT`]), for one-off
    /// readers like `amx logs`. The card uses [`output_tail`](Self::output_tail).
    ///
    /// Rendered as a terminal would show it (see [`crate::ansi::laid_out`]):
    /// overwritten rows collapse, cursor moves become spacing, and escape
    /// codes are removed. Invalid UTF-8 is replaced, not fatal.
    ///
    /// `None` if there is no file, or once the vendor has spoken (the record
    /// names a transcript): then the record and transcript are the account and
    /// boot output is just paint. The file matters for a vendor that died
    /// before its first hook, and for a command, which reports nothing.
    pub fn output(&self) -> Option<String> {
        if self.spoke() {
            return None;
        }
        let bytes = std::fs::read(self.dir.join(OUTPUT)).ok()?;
        Some(crate::ansi::laid_out(&String::from_utf8_lossy(&bytes)))
    }

    /// The last [`OUTPUT_TAIL`] bytes of what the pane printed, for the card.
    ///
    /// The card is re-read every second and a long build can print tens of
    /// megabytes, so only the end is read. Rendered and hidden as
    /// [`output`](Self::output) is. The partial first row is dropped, since
    /// the card would show it as a row the command never printed, unless the
    /// tail holds no row break at all.
    pub fn output_tail(&self) -> Option<String> {
        if self.spoke() {
            return None;
        }
        let mut file = File::open(self.dir.join(OUTPUT)).ok()?;
        let from = file.metadata().ok()?.len().saturating_sub(OUTPUT_TAIL);
        file.seek(SeekFrom::Start(from)).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let printed = String::from_utf8_lossy(&bytes);
        let whole = match from {
            0 => printed.as_ref(),
            _ => printed
                .split_once('\n')
                .map_or(printed.as_ref(), |(_, rest)| rest),
        };
        Some(crate::ansi::laid_out(whole))
    }

    /// Whether the vendor has reported anything, i.e. the record names a
    /// transcript.
    ///
    /// `meta.session` does not tell: amx writes it at spawn for vendors that
    /// take a session id (pi's `--session-id`), before the vendor says a word.
    /// Only a report sets the transcript. A directory with no `meta.json`
    /// counts as not having spoken.
    fn spoke(&self) -> bool {
        self.meta().is_ok_and(|meta| meta.transcript.is_some())
    }

    /// The end of the transcript the record names.
    ///
    /// Transcripts grow to megabytes and rows are redrawn every second, so
    /// this reads only the last [`TAIL`] bytes. The read usually starts inside
    /// a line; readers skip lines that are not JSON (see
    /// [`crate::conversation`]), which covers that partial line and any split
    /// character.
    ///
    /// If the window holds no line break except the final one (a huge last
    /// entry, such as a large tool result), the tail is that whole entry plus
    /// the [`TAIL`] bytes before it, so the entry before is still readable.
    ///
    /// `None` if the record names no transcript or the file is missing. Takes
    /// only `meta`, so callers without an open [`Agent`] can use it.
    pub fn transcript_tail(meta: &Meta) -> Option<String> {
        let path = meta.transcript.as_ref()?;
        let mut file = File::open(path).ok()?;
        let mut from = file.metadata().ok()?.len().saturating_sub(TAIL);
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(from)).ok()?;
        file.read_to_end(&mut bytes).ok()?;
        let body = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        if from > 0 && !body.contains(&b'\n') {
            from = Self::line_start(&mut file, from)?.saturating_sub(TAIL);
            bytes.clear();
            file.seek(SeekFrom::Start(from)).ok()?;
            file.read_to_end(&mut bytes).ok()?;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The start of the line holding `at`: just past the nearest line break
    /// before it, scanning back [`TAIL`] bytes at a time, or 0.
    fn line_start(file: &mut File, at: u64) -> Option<u64> {
        let mut end = at;
        let mut chunk = Vec::new();
        while end > 0 {
            let start = end.saturating_sub(TAIL);
            chunk.resize((end - start) as usize, 0);
            file.seek(SeekFrom::Start(start)).ok()?;
            file.read_exact(&mut chunk).ok()?;
            if let Some(at) = chunk.iter().rposition(|byte| *byte == b'\n') {
                return Some(start + at as u64 + 1);
            }
            end = start;
        }
        Some(0)
    }

    /// How it was started.
    pub fn meta(&self) -> Result<Meta> {
        let path = self.dir.join(META);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
    }

    /// What it is doing. A missing state document reads as the initial state.
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

    /// The event log's path, for readers that tail it.
    pub fn events_path(&self) -> PathBuf {
        self.dir.join(EVENTS)
    }

    /// Every event, oldest first. Unparseable lines are skipped, so a damaged
    /// tail does not lose the history.
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

    /// Take the writer lock, held until the returned value is dropped.
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

/// The right to change one agent's record. At most one exists at a time
/// across all amx processes.
pub struct Writer<'a> {
    agent: &'a Agent,
    _lock: nix::fcntl::Flock<File>,
}

impl Writer<'_> {
    /// Read the state this writer is about to change.
    pub fn state(&self) -> Result<State> {
        self.agent.state()
    }

    /// Change the state and stamp `last_event`.
    ///
    /// When the phase changes: `since` moves (a mid-turn summary must not
    /// reset the clock); `ended` is set on entering a terminal phase and
    /// cleared on leaving one; and a span leaving `working` is added to
    /// `worked`, since this write is the only moment amx learns the span
    /// ended.
    pub fn update_state(&self, change: impl FnOnce(&mut State)) -> Result<State> {
        self.update_state_at(now(), change)
    }

    /// [`update_state`](Self::update_state) at a given time, for tests.
    fn update_state_at(&self, at: u64, change: impl FnOnce(&mut State)) -> Result<State> {
        self.update_state_closing_at(at, at, change)
    }

    /// Change the state on behalf of someone other than the agent: `stop`,
    /// `resume`, the exit hook.
    ///
    /// An open working span closes when the agent was last heard, not now: a
    /// record left working over a pane that died ten hours ago did not work
    /// for ten hours. "Heard" is the latest of `last_event`, `since` and the
    /// heartbeat, capped at now, as in [`crate::derive`].
    pub fn update_state_heard(
        &self,
        heartbeat: Option<u64>,
        change: impl FnOnce(&mut State),
    ) -> Result<State> {
        self.update_state_heard_at(now(), heartbeat, change)
    }

    fn update_state_heard_at(
        &self,
        at: u64,
        heartbeat: Option<u64>,
        change: impl FnOnce(&mut State),
    ) -> Result<State> {
        let before = self.state()?;
        let heard = before
            .last_event
            .max(before.since)
            .max(heartbeat.unwrap_or_default())
            .min(at);
        self.update_state_closing_at(at, heard, change)
    }

    /// The write itself: stamped at `at`, with an open span closed at `close`.
    fn update_state_closing_at(
        &self,
        at: u64,
        close: u64,
        change: impl FnOnce(&mut State),
    ) -> Result<State> {
        let before = self.state()?;
        let mut after = before.clone();
        change(&mut after);

        if after.state != before.state {
            if before.state == Phase::Working {
                after.worked = before.worked_by(close);
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

    /// Record something a reader saw on the pane.
    ///
    /// `last_event` is left alone: it measures how fresh the record is, and a
    /// screen reading is not news from the agent. Moving it would make the
    /// next reader trust the record over the pane.
    ///
    /// Writes nothing when nothing changed, so redrawing a wall of agents
    /// costs one write per new question, not one per look.
    pub fn observe(&self, change: impl FnOnce(&mut State)) -> Result<State> {
        let before = self.state()?;
        let mut after = before.clone();
        change(&mut after);

        if after != before {
            write_atomic(&self.agent.dir.join(STATE), &to_json(&after)?)?;
        }
        Ok(after)
    }

    /// Change the meta. The session id and transcript arrive later, from a
    /// hook.
    pub fn update_meta(&self, change: impl FnOnce(&mut Meta)) -> Result<Meta> {
        let mut meta = self.agent.meta()?;
        change(&mut meta);
        write_atomic(&self.agent.dir.join(META), &to_json(&meta)?)?;
        Ok(meta)
    }

    /// Append one event as a single write under the lock, so concurrent hooks
    /// never interleave lines.
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
        // An id-shaped directory holding a record.
        if crate::ids::is_valid(&id) && entry.path().join(META).is_file() {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Take `spawn.lock`, next to the agents directory, under which a spawn
/// counts the caps and claims its place. Held until the file is dropped.
///
/// One machine-wide lock: the caps count every record, and two spawns that
/// both counted before either claimed would both find room.
pub fn spawn_lock(root: &Path) -> Result<File> {
    let path = root.parent().unwrap_or(root).join(SPAWN_LOCK);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(paths::FILE_MODE)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(file)
}

/// A place under the caps held by a spawn still setting up: a file naming the
/// project it counts against, locked while the spawn runs.
///
/// A claim whose spawn died is unlocked and counts for nothing. Dropping the
/// claim removes the file.
pub struct Claim {
    path: PathBuf,
    _file: File,
}

impl Claim {
    /// Claim a place in `dir` for an agent of `project`.
    pub fn hold(dir: &Path, project: &Path) -> Result<Claim> {
        let path = dir.join(CLAIM);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(paths::FILE_MODE)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        file.write_all(project.as_os_str().as_encoded_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(Claim { path, _file: file })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Places claimed under `root` by spawns still setting up: each claim's id
/// and project.
pub fn claims(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", root.display())),
    };

    let mut held = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // A claim gone since the listing is a spawn that finished.
        let Ok(mut file) = File::open(entry.path().join(CLAIM)) else {
            continue;
        };
        if !matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)) {
            continue;
        }
        let mut project = Vec::new();
        file.read_to_end(&mut project)
            .with_context(|| format!("reading the claim of {id}"))?;
        held.push((id, PathBuf::from(OsString::from_vec(project))));
    }
    Ok(held)
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).context("writing a record")?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Write `bytes` to `path` atomically: readers see the old content or all of
/// the new, never a mix.
///
/// Also used for the view's own file next to the agents, which several views
/// may write.
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
    // Restrict the mode before writing, so the file is never readable by
    // others.
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
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
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
    fn store_reads_a_record_with_no_family_as_a_root_at_zero() {
        // `parent` and `depth` were added later; an older record reads as a
        // root at depth 0.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let fresh = agent.meta().unwrap();
        assert_eq!(fresh.parent, None);
        assert_eq!(fresh.depth, 0);

        let text = std::fs::read_to_string(agent.dir().join(META)).unwrap();
        let mut document: serde_json::Value = serde_json::from_str(&text).unwrap();
        document.as_object_mut().unwrap().remove("parent");
        document.as_object_mut().unwrap().remove("depth");
        std::fs::write(agent.dir().join(META), document.to_string()).unwrap();

        let older = agent.meta().unwrap();
        assert_eq!(older.parent, None);
        assert_eq!(older.depth, 0);
    }

    #[test]
    fn store_reads_a_record_with_no_role_as_none() {
        // `role` was added later still; an older record reads as having none.
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();

        let text = std::fs::read_to_string(agent.dir().join(META)).unwrap();
        let mut document: serde_json::Value = serde_json::from_str(&text).unwrap();
        document.as_object_mut().unwrap().remove("role");
        std::fs::write(agent.dir().join(META), document.to_string()).unwrap();

        assert_eq!(agent.meta().unwrap().role, None);
    }

    #[test]
    fn store_reads_a_beat_off_the_record() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        assert_eq!(agent.heartbeat(), None, "nothing has beaten yet");

        // The beat is the file's mtime, not its content.
        std::fs::write(agent.dir().join(HEARTBEAT), "").unwrap();
        let beat = agent.heartbeat().expect("a beat");
        assert!(
            beat.abs_diff(now()) <= 1,
            "a beat is when it was written: {beat}"
        );
    }

    #[test]
    fn store_writes_a_record_into_a_directory_that_is_waiting_for_it() {
        // Spawning creates the directory before there is a record, so a bare
        // directory does not count as one.
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

        // Creating the same record twice is an error.
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

    /// The permission bits of `path`.
    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Permissions with the given mode bits.
    fn mode(bits: u32) -> std::fs::Permissions {
        std::fs::Permissions::from_mode(bits)
    }

    #[test]
    fn every_file_in_a_record_is_the_owners_alone() {
        // The directory may exist before the record, the umask can strip bits
        // from the mode passed to `open`, and that mode is ignored for an
        // existing file, so every path must be chmodded explicitly.
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

        // An answer written after the end does not move the end.
        let after = writer
            .update_state(|s| s.result = Some("wrote the parser".to_string()))
            .unwrap();
        assert_eq!(after.ended, done.ended);

        // Leaving a terminal phase clears it.
        let again = writer.update_state(|s| s.state = Phase::Working).unwrap();
        assert_eq!(again.ended, 0);
    }

    #[test]
    fn store_keeps_the_moment_it_let_a_pane_go() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        let idle = writer.update_state(|s| s.state = Phase::Idle).unwrap();
        assert_eq!(idle.parked_at, 0, "its pane is still there");

        // Parking is amx's doing, not news from the agent, so `last_event`
        // stays.
        let parked = writer.observe(|s| s.parked_at = 1_700).unwrap();
        assert_eq!(parked.parked_at, 1_700);
        assert_eq!(parked.last_event, idle.last_event);
        assert_eq!(written(&agent)["parked_at"], 1_700);
        assert_eq!(agent.state().unwrap().parked_at, 1_700);

        // A document from before the field reads as never parked.
        std::fs::write(agent.dir().join(STATE), r#"{"state":"idle"}"#).unwrap();
        assert_eq!(agent.state().unwrap().parked_at, 0);
    }

    #[test]
    fn store_keeps_the_count_of_shells_a_turn_left_running() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        let ended = writer
            .update_state(|s| {
                s.state = Phase::Working;
                s.background = 2;
            })
            .unwrap();
        assert_eq!(ended.background, 2);
        assert_eq!(written(&agent)["background"], 2);
        assert_eq!(agent.state().unwrap().background, 2);

        // A document from before the field reads as nothing left running.
        std::fs::write(agent.dir().join(STATE), r#"{"state":"idle"}"#).unwrap();
        assert_eq!(agent.state().unwrap().background, 0);
    }

    #[test]
    fn store_keeps_the_moment_it_cut_a_turn_short() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        let working = writer.update_state(|s| s.state = Phase::Working).unwrap();
        assert_eq!(working.interrupted_at, 0, "nothing has been cut short");

        // An interrupt is amx's doing, not news from the agent, so
        // `last_event` stays.
        let cut = writer.observe(|s| s.interrupted_at = 1_700).unwrap();
        assert_eq!(cut.interrupted_at, 1_700);
        assert_eq!(cut.last_event, working.last_event);
        assert_eq!(written(&agent)["interrupted_at"], 1_700);
        assert_eq!(agent.state().unwrap().interrupted_at, 1_700);

        // A document from before the field reads as never interrupted.
        std::fs::write(agent.dir().join(STATE), r#"{"state":"working"}"#).unwrap();
        assert_eq!(agent.state().unwrap().interrupted_at, 0);
    }

    #[test]
    fn store_adds_up_the_spans_an_agent_spent_working() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        // Six seconds of work, then a question left unanswered for an hour.
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

        // Four more seconds, then the run ends.
        let done = writer
            .update_state_at(4_610, |s| {
                s.state = Phase::Done;
                s.exit = Some(0);
            })
            .unwrap();
        assert_eq!(done.worked, 10);
        assert_eq!(written(&agent)["worked"], 10);

        // Writes after the end add no work.
        let after = writer
            .update_state_at(9_000, |s| s.result = Some("the tests pass now".to_string()))
            .unwrap();
        assert_eq!(after.worked, 10);
    }

    #[test]
    fn store_counts_a_span_of_work_nothing_ever_closed() {
        // Spans are added when the phase changes, so a record whose pane died
        // mid-turn still has one open. The caller says how far to count it.
        let working = State {
            state: Phase::Working,
            since: 1_200,
            worked: 20,
            ..State::default()
        };
        assert_eq!(working.worked_by(1_300), 120);

        // A phase other than working has no open span.
        let waiting = State {
            state: Phase::Waiting,
            since: 1_200,
            worked: 20,
            ..State::default()
        };
        assert_eq!(waiting.worked_by(9_000), 20);

        // A working record with no `since` counts no open span.
        let undated = State {
            state: Phase::Working,
            ..State::default()
        };
        assert_eq!(undated.worked_by(9_000), 0);
    }

    #[test]
    fn a_stale_span_closes_where_the_agent_was_last_heard() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();

        // A turn opened at 1_000, last heard at 1_100, whose pane then died.
        // Ten hours later it is stopped.
        writer
            .update_state_at(1_000, |s| s.state = Phase::Working)
            .unwrap();
        writer.update_state_at(1_100, |_| {}).unwrap();
        let stopped = writer
            .update_state_heard_at(37_100, None, |s| s.state = Phase::Stopped)
            .unwrap();
        assert_eq!(stopped.worked, 100, "ten hours of a dead pane are not work");
        assert_eq!(stopped.ended, 37_100, "the stop is still when it stopped");

        // A heartbeat after the last hook counts as heard.
        writer
            .update_state_at(40_000, |s| s.state = Phase::Working)
            .unwrap();
        let beaten = writer
            .update_state_heard_at(80_000, Some(40_050), |s| s.state = Phase::Stopped)
            .unwrap();
        assert_eq!(beaten.worked, 150);

        // A heartbeat in the future is capped at the write.
        writer
            .update_state_at(90_000, |s| s.state = Phase::Working)
            .unwrap();
        let capped = writer
            .update_state_heard_at(90_010, Some(99_999), |s| s.state = Phase::Stopped)
            .unwrap();
        assert_eq!(capped.worked, 160);
    }

    #[test]
    fn a_log_adds_its_turns_up_from_edge_to_edge() {
        let at = |at, kind: &str| Event {
            at,
            kind: kind.to_string(),
            payload: serde_json::Value::Null,
        };
        let edge = |kind: &str| match kind {
            "Prompt" => Some(Edge::Opens),
            "End" => Some(Edge::Closes),
            _ => None,
        };
        let log = [
            at(1_000, "Start"),
            at(1_010, "Prompt"),
            at(1_020, "Tool"),
            // Steered into the running turn, which continues.
            at(1_030, "Prompt"),
            at(1_050, "End"),
            // A day idle at the prompt is not work.
            at(87_450, "Prompt"),
            at(87_455, "End"),
            // A turn never closed counts nothing.
            at(90_000, "Prompt"),
        ];
        assert_eq!(worked_in(&log, edge), Some(45));
        assert_eq!(worked_in(&log[..1], edge), None, "a log with no turn edges");
        assert_eq!(worked_in(&[at(1, "Prompt"), at(1, "End")], edge), Some(0));
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

        // Another event in the same phase leaves `since` alone.
        let still = writer
            .update_state(|s| s.summary = Some("Editing src/cli.rs".to_string()))
            .unwrap();
        assert_eq!(still.since, working.since);
        assert_eq!(still.summary.as_deref(), Some("Editing src/cli.rs"));

        assert_eq!(agent.state().unwrap(), still);
    }

    /// The state document on disk, parsed as plain JSON.
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
        // A question a hook just reported is only words, and is written as a
        // plain string.
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
        // Clearing the question drops its options too; the document has no
        // place for options without a question.
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

    /// A choice with a description.
    fn choice(label: &str, description: &str) -> Choice {
        Choice {
            label: label.to_string(),
            description: Some(description.to_string()),
            preview: None,
        }
    }

    /// A three-question call as claude 2.1.240 sends it (see
    /// `docs/question-shapes.md`): two single-choice questions and one
    /// multi-choice.
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

        // The question on screen is where readers look for it.
        let document = written(&agent);
        assert_eq!(
            document["question"]["text"],
            "Which runtime should the service target?"
        );
        assert_eq!(document["question"]["options"][0], "Node");
        assert_eq!(document["question"]["kind"], "question");

        // The rest of the call, which no screen carries, is under it.
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
        // In claude 2.1.240, answering a tab moves to the next one; the
        // prompt stays up.
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

        // All answered: nothing left to ask, every answer kept, and the
        // vendor's Submit tab on screen.
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

        // With nothing pending, a further answer goes nowhere.
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

        // The call goes with its question, like the options do.
        writer.update_state(|s| s.asks(None)).unwrap();
        assert_eq!(written(&agent)["question"], serde_json::Value::Null);
        assert!(agent.state().unwrap().asking.is_empty());
    }

    #[test]
    fn store_keeps_the_preview_that_puts_a_notes_field_on_a_question() {
        // In claude 2.1.240 a preview on any choice draws the notes field, and
        // `n` does nothing without one. The screen does not show which, so the
        // record must.
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
        // The kind is kept even without the words, and options without a
        // question are dropped.
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

        // New words for the outstanding question keep its kind: the hook
        // carrying them does not name the screen.
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
            marked: None,
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
            walked: false,
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

        // With nothing on the record, the screen supplies the question.
        let mut nothing_heard = State::default();
        nothing_heard.learn(&seen);
        assert_eq!(
            nothing_heard.question.as_deref(),
            Some("Do you want to proceed?")
        );
    }

    #[test]
    fn store_says_whether_a_question_was_reported_or_read() {
        // `reported` travels with the question and decides whether the next
        // pane reading may replace it.
        let seen = Question {
            marked: None,
            text: "Run echo hi?".to_string(),
            options: vec!["Allow once".to_string(), "Deny".to_string()],
            walked: false,
        };

        let mut heard = State::default();
        heard.asks(Some("Claude needs your permission to use Bash".to_string()));
        assert!(heard.reported, "a hook carried it");
        heard.learn(&seen);
        assert!(
            heard.reported,
            "and a screen filling the choices under it does not make it \
             anybody else's word"
        );

        let mut read = State::default();
        read.learn(&seen);
        assert!(!read.reported, "the screen said it, with nothing to fill");
        read.correct(Some(&seen));
        assert!(!read.reported, "and a later screen still is not the vendor");

        // Clearing the question clears the flag, whoever clears it.
        for mut state in [heard, read] {
            state.asks(None);
            assert!(!state.reported, "nothing outstanding is nobody's word");
        }

        // A whole call is reported the same way as a single question.
        let mut call = State::default();
        call.asks_all(vec![Ask {
            header: None,
            text: "Which fixture should the port keep?".to_string(),
            options: vec![Choice {
                label: "the sqlite one".to_string(),
                description: None,
                preview: None,
            }],
            multi: false,
            answer: None,
        }]);
        assert!(call.reported);
        call.correct(Some(&seen));
        assert!(!call.reported);
    }

    #[test]
    fn store_round_trips_where_a_question_came_from() {
        // A reported question with nothing else is written as its words alone
        // and read back as reported, including from older documents.
        let mut heard = State::default();
        heard.asks(Some("Claude needs your permission".to_string()));
        let document = serde_json::to_value(heard.clone()).unwrap();
        assert_eq!(document["question"], "Claude needs your permission");
        assert!(
            serde_json::from_value::<State>(document).unwrap().reported,
            "the words alone are the vendor's own"
        );

        // Anything else carries the flag explicitly; without it, the question
        // is a screen reading.
        let mut read = State::default();
        read.correct(Some(&Question {
            marked: None,
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
            walked: false,
        }));
        let document = serde_json::to_value(read.clone()).unwrap();
        assert_eq!(document["question"]["reported"], serde_json::Value::Null);
        assert_eq!(serde_json::from_value::<State>(document).unwrap(), read);

        let mut heard_with_choices = read.clone();
        heard_with_choices.reported = true;
        let document = serde_json::to_value(heard_with_choices.clone()).unwrap();
        assert_eq!(document["question"]["reported"], true);
        assert_eq!(
            serde_json::from_value::<State>(document).unwrap(),
            heard_with_choices
        );
    }

    #[test]
    fn store_round_trips_a_list_amx_numbered_itself() {
        // Choices read off a cursor mark are not taken by typing a digit, so
        // `walked` travels with the options: written when true, absent when
        // false, and false on older records.
        let seen = Question {
            marked: None,
            text: "Run echo hi?".to_string(),
            options: vec!["Allow once".to_string(), "Deny".to_string()],
            walked: true,
        };
        let mut state = State::default();
        state.correct(Some(&seen));
        assert!(state.walked);

        let document = serde_json::to_value(state.clone()).unwrap();
        assert_eq!(document["question"]["walked"], true);
        assert_eq!(serde_json::from_value::<State>(document).unwrap(), state);

        // The same choices read off vendor numbers are taken differently, so
        // that change alone corrects the record.
        let numbered = Question {
            marked: None,
            walked: false,
            ..seen.clone()
        };
        assert!(state.corrected_by(Some(&numbered)));
        state.correct(Some(&numbered));
        let document = serde_json::to_value(state.clone()).unwrap();
        assert_eq!(
            document["question"]["walked"],
            serde_json::Value::Null,
            "a document that does not say is a list the vendor numbered"
        );
        assert_eq!(serde_json::from_value::<State>(document).unwrap(), state);

        // Older documents read as not walked, and the flag goes with the
        // question.
        let before: State = serde_json::from_str(
            r#"{"state":"waiting","question":{"text":"Run echo hi?","options":["Allow once"]}}"#,
        )
        .unwrap();
        assert_eq!(before.options, ["Allow once"]);
        assert!(!before.walked);

        let mut answered = State::default();
        answered.learn(&seen);
        assert!(
            answered.walked,
            "a screen's mark is learned with its choices"
        );
        answered.asks(Some("Claude needs your permission".to_string()));
        assert!(!answered.walked, "and a hook's question offers no walk");
    }

    #[test]
    fn store_lets_a_later_screen_correct_what_an_earlier_screen_said() {
        // For a question read off the screen, a later reading replaces an
        // earlier one: a pane shows one screen at a time.
        let mut state = State {
            question: Some("Run echo hi?".to_string()),
            options: vec!["Allow once".to_string(), "Deny".to_string()],
            ..State::default()
        };
        let later = Question {
            marked: None,
            text: "Which branch should I push to?".to_string(),
            options: Vec::new(),
            walked: false,
        };

        assert!(state.corrected_by(Some(&later)), "the pane has moved on");
        state.correct(Some(&later));
        assert_eq!(
            state.question.as_deref(),
            Some("Which branch should I push to?")
        );
        assert!(
            state.options.is_empty(),
            "and the choices went with the question they were drawn under"
        );
        assert!(
            !state.corrected_by(Some(&later)),
            "while looking again at the same screen corrects nothing"
        );

        // A screen with nothing to answer clears the question.
        assert!(state.corrected_by(None));
        state.correct(None);
        assert_eq!(state.question, None);
        assert!(state.options.is_empty());
        assert!(!state.corrected_by(None), "with nothing left to clear");

        // One screen's choices never land under another's question. Filling
        // one field at a time did that to a pi going from the login box (a key
        // prompt, no choices) to the trust selector (three choices).
        let mut typed_at = State::default();
        typed_at.correct(Some(&Question {
            marked: None,
            text: "Enter Cerebras API key".to_string(),
            options: Vec::new(),
            walked: false,
        }));
        typed_at.correct(Some(&Question {
            marked: None,
            text: "Project trust".to_string(),
            options: vec!["Trust".to_string(), "Do not trust".to_string()],
            walked: false,
        }));
        assert_eq!(typed_at.question.as_deref(), Some("Project trust"));
        assert_eq!(typed_at.options, ["Trust", "Do not trust"]);
    }

    #[test]
    fn store_a_reader_that_learned_nothing_writes_nothing() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("fix-login-a1b")).unwrap();
        let writer = agent.writer().unwrap();
        let heard = writer.update_state(|s| s.state = Phase::Waiting).unwrap();

        let looked = writer.observe(|s| s.learn(&Question::default())).unwrap();
        assert_eq!(looked, heard, "including the clock");

        // What it learns is written without moving `last_event`.
        let seen = Question {
            marked: None,
            text: "Do you want to proceed?".to_string(),
            options: vec!["Yes".to_string()],
            walked: false,
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
    fn a_record_keeps_when_somebody_last_looked_at_the_agent() {
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

        // A document from before the field reads as never seen.
        std::fs::write(agent.dir().join(STATE), r#"{"state":"done"}"#).unwrap();
        assert_eq!(agent.state().unwrap().seen, 0);
    }

    #[test]
    fn a_record_keeps_what_a_person_renamed_an_agent_to() {
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

        // A document from before the field reads as never renamed.
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
    fn store_reads_the_tail_of_the_transcript_the_record_names() {
        let root = TempDir::new().unwrap();
        let mut record = meta("fix-login-a1b");

        assert_eq!(
            Agent::transcript_tail(&record),
            None,
            "a record naming no transcript has no tail"
        );

        let path = root.path().join("abc-123.jsonl");
        record.transcript = Some(path.clone());
        assert_eq!(
            Agent::transcript_tail(&record),
            None,
            "and neither has one whose file is not there"
        );

        let padding = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"still working\"}]}}\n";
        std::fs::write(&path, padding).unwrap();
        assert_eq!(
            Agent::transcript_tail(&record).as_deref(),
            Some(padding),
            "a transcript shorter than the tail is read whole"
        );

        // Longer than the tail: 800 lines of 84 bytes and a call after them, so
        // the read starts 19 bytes into a line.
        let mut session = padding.repeat(800);
        session.push_str("{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"file_path\":\"src/importer.rs\"}}]}}\n");
        std::fs::write(&path, &session).unwrap();

        let tail = Agent::transcript_tail(&record).unwrap();
        assert_eq!(tail.len(), TAIL as usize, "the last 64 KiB of it");
        assert!(
            serde_json::from_str::<serde_json::Value>(tail.lines().next().unwrap()).is_err(),
            "cut inside a line, which is the half line the reading skips"
        );
        assert_eq!(
            crate::conversation::latest(crate::vendor::Transcript::Claude, &tail).as_deref(),
            Some("Read src/importer.rs"),
            "and the rest of the tail reads as the transcript it is"
        );
    }

    #[test]
    fn store_transcript_tail_holds_a_last_entry_bigger_than_the_window() {
        let root = TempDir::new().unwrap();
        let mut record = meta("fix-login-a1b");
        let path = root.path().join("abc-123.jsonl");
        record.transcript = Some(path.clone());

        // A tool result larger than the tail: the window starts inside it and
        // holds no whole line.
        let called = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"file_path\":\"big.log\"}}],\"usage\":{\"input_tokens\":1200}}}\n";
        let result = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":[{{\"type\":\"tool_result\",\"content\":\"{}\"}}]}}}}\n",
            "x".repeat(2 * TAIL as usize)
        );
        let session = "{\"type\":\"user\",\"message\":{\"content\":\"read it\"}}\n".repeat(2000)
            + called
            + &result;
        std::fs::write(&path, &session).unwrap();

        let tail = Agent::transcript_tail(&record).unwrap();
        assert!(tail.ends_with(&result), "the last entry whole");
        assert!(
            tail.len() <= result.len() + TAIL as usize,
            "and no more than a window before it"
        );
        assert_eq!(
            crate::conversation::context_and_last_words(crate::vendor::Transcript::Claude, &tail).0,
            Some(1200),
            "so the call before it still gives the context"
        );
    }

    #[test]
    fn store_reads_what_was_piped_beside_the_record() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("run-tests-a1b")).unwrap();
        assert_eq!(
            agent.output(),
            None,
            "a record whose boot has piped nothing has no file"
        );

        assert_eq!(
            agent.output_tail(),
            None,
            "and the card reading its end finds no file either"
        );

        std::fs::write(agent.dir().join(OUTPUT), "one\ntwo\n").unwrap();
        assert_eq!(
            agent.output().as_deref(),
            Some("one\ntwo"),
            "the whole of what the command printed, first line and last"
        );
        assert_eq!(
            agent.output_tail().as_deref(),
            Some("one\ntwo"),
            "and a file shorter than the cap is the tail, whole"
        );

        // Rendered as the terminal showed it: CRLF line ends, a progress bar
        // redrawn in place as one row, and escape codes removed.
        std::fs::write(
            agent.dir().join(OUTPUT),
            b"\x1b[32mgreen\x1b[0m\r\n 1/3\r 2/3\r 3/3 done\r\nlast",
        )
        .unwrap();
        assert_eq!(
            agent.output().as_deref(),
            Some("green\n 3/3 done\nlast"),
            "the paint stripped, the return before each newline gone, and \
             only the last draw of an overwritten row"
        );
        assert_eq!(
            agent.output_tail().as_deref(),
            agent.output().as_deref(),
            "the tail resolves its returns the same way"
        );

        std::fs::write(agent.dir().join(OUTPUT), b"caf\xc3\xa9 \xff\n").unwrap();
        assert_eq!(
            agent.output().as_deref(),
            Some("café \u{fffd}"),
            "and a byte that is not text is read past, not the whole file lost"
        );
    }

    #[test]
    fn store_reads_a_boot_as_the_screen_it_drew() {
        // Two boot frames drawn by cursor positioning, the second over the
        // first. Read as a byte stream, this was `Accessingworkspace:` on one
        // line.
        let boot = concat!(
            "\u{1b}[2J\u{1b}[H",
            "\u{1b}[1;1HAccessing",
            "\u{1b}[1;17Hworkspace:",
            "\u{1b}[2;3Hreading the files",
            "\u{1b}[2;3Hready\u{1b}[K",
        );
        let screen = "Accessing       workspace:\n  ready";

        let root = TempDir::new().unwrap();
        let agent = Agent::create(
            root.path(),
            &Meta {
                agent: Some("claude".to_string()),
                ..meta("fix-login-a1b")
            },
        )
        .unwrap();
        std::fs::write(agent.dir().join(OUTPUT), boot).unwrap();
        assert_eq!(agent.output().as_deref(), Some(screen));
        assert_eq!(
            agent.output_tail().as_deref(),
            Some(screen),
            "and the card reads the same screen the log does"
        );
    }

    #[test]
    fn store_reads_an_agents_dying_words_only_before_it_spoke() {
        let root = TempDir::new().unwrap();
        // A vendor that died before any report leaves only its boot output.
        let quiet = Agent::create(
            root.path(),
            &Meta {
                agent: Some("claude".to_string()),
                ..meta("fix-login-a1b")
            },
        )
        .unwrap();
        std::fs::write(quiet.dir().join(OUTPUT), "could not read the state file\n").unwrap();
        assert_eq!(
            quiet.output().as_deref(),
            Some("could not read the state file")
        );

        // Once a report names a transcript, boot output is no longer read. A
        // session id does not count: pi's is written at spawn.
        let spoke = Agent::create(
            root.path(),
            &Meta {
                agent: Some("claude".to_string()),
                transcript: Some(std::path::PathBuf::from("/srv/transcript.jsonl")),
                ..meta("port-it-b2c")
            },
        )
        .unwrap();
        std::fs::write(spoke.dir().join(OUTPUT), "could not read the state file\n").unwrap();
        assert_eq!(spoke.output(), None, "a record that spoke reads none of it");
        assert_eq!(spoke.output_tail(), None, "and the card does not either");
    }

    #[test]
    fn store_reads_the_end_of_a_long_command_output() {
        let root = TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &meta("build-a1b")).unwrap();

        // 3840 numbered 80-byte rows (300 KiB), so the read starts 16 bytes
        // into row 564.
        let printed: String = (1..=3840)
            .map(|n| format!("{:<79}\n", format!("row {n}")))
            .collect();
        std::fs::write(agent.dir().join(OUTPUT), &printed).unwrap();

        let tail = agent.output_tail().unwrap();
        let rows: Vec<&str> = printed.lines().map(str::trim_end).collect();
        assert!(
            tail.len() <= OUTPUT_TAIL as usize && rows.join("\n").ends_with(&tail),
            "a quarter megabyte of it at most, and the end of it"
        );
        assert_eq!(
            tail.lines().next().unwrap().trim_end(),
            "row 565",
            "opening on a whole row: the half of 564 the offset landed in goes"
        );
        assert_eq!(
            tail.lines().last().unwrap().trim_end(),
            "row 3840",
            "and ending on the last row the command printed"
        );
        assert!(
            agent.output().unwrap().starts_with("row 1\n"),
            "while the reader that wants all of it still gets the first row"
        );
    }

    #[test]
    fn store_reads_a_record_written_by_an_older_amx() {
        // Fields are only added; an older document reads with defaults.
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

        // An older document stores the question as a plain string.
        std::fs::write(
            dir.join(STATE),
            r#"{"state":"waiting","question":"Do you want to proceed?"}"#,
        )
        .unwrap();
        let state = agent.state().unwrap();
        assert_eq!(state.question.as_deref(), Some("Do you want to proceed?"));
        assert!(state.options.is_empty());
        assert!(state.asking.is_empty());

        // An older document has no call behind the question.
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
        // A crash mid-write leaves a partial last line.
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

        // Payloads large enough that unlocked writes could interleave.
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
        // Concurrent read-modify-writes must not lose updates.
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

            // Readers take no lock, so this reads during the writes.
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
