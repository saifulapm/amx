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

/// How the agent was started, and how to reach it again.
///
/// Fields are added, never renamed or removed: a record outlives the version
/// of amx that wrote it, so every field that may be absent carries a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub task: String,
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
        if self.question.is_none() {
            self.kind = None;
        }
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
/// shapes meet: the record keeps a question and the answers it offers under a
/// single `question` key, because they are one thing to everything that reads
/// them, while in memory the text is a field of its own, because the text
/// alone is what most of amx asks for. Options with no question over them are
/// not written at all, which is what keeps an answered question from leaving
/// its choices behind.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct Wire {
    state: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    seq: u64,
    since: u64,
    summary: Option<String>,
    question: Option<Asked>,
    result: Option<String>,
    source: Option<Source>,
    exit: Option<i32>,
    last_event: u64,
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
            kind,
            result,
            source,
            exit,
            last_event,
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
                // whose payload it could not read. The kind is the whole of
                // what there is to say about it, and choices with no question
                // over them are nobody's to press.
                (None, kind) => Some(Asked::Whole(Known {
                    kind,
                    ..Known::default()
                })),
                // The words alone, as amx has always written a question a hook
                // has only just carried.
                (Some(text), None) if options.is_empty() => Some(Asked::Words(text)),
                (text, kind) => Some(Asked::Whole(Known {
                    text,
                    options,
                    kind,
                })),
            },
            result,
            source,
            exit,
            last_event,
        }
    }
}

impl From<Wire> for State {
    fn from(wire: Wire) -> State {
        let (question, options, kind) = match wire.question {
            Some(Asked::Words(text)) => (Some(text), Vec::new(), None),
            Some(Asked::Whole(asked)) => (asked.text, asked.options, asked.kind),
            None => (None, Vec::new(), None),
        };

        State {
            state: wire.state,
            name: wire.name,
            seq: wire.seq,
            since: wire.since,
            summary: wire.summary,
            question,
            options,
            kind,
            result: wire.result,
            source: wire.source,
            exit: wire.exit,
            last_event: wire.last_event,
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
    pub fn update_state(&self, change: impl FnOnce(&mut State)) -> Result<State> {
        let before = self.state()?;
        let mut after = before.clone();
        change(&mut after);

        let at = now();
        if after.state != before.state {
            after.since = at;
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
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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
            dir: PathBuf::from("/srv/app"),
            worktree: Some(PathBuf::from("/srv/app/.amx/worktrees/fix-login-a1b")),
            branch: Some("amx/fix-login-a1b".to_string()),
            base: Some("0f1e2d3".to_string()),
            socket: Socket::Name("amx".to_string()),
            pane: PaneId::new("%7").unwrap(),
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
