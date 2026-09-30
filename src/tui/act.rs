//! Actions the view takes on agents, and the composer line they are typed on.
//!
//! Each action runs the same code as its verb, in the view's own process, with
//! two differences. Failures come back as the one line the view shows, since a
//! terminal in raw mode cannot take stderr. And the view does not wait for the
//! vendor to confirm: `send` gives it five seconds, which a view cannot spend,
//! and the next reading shows the result anyway.
//!
//! A reply is routed by the agent's phase when the line is entered, not when
//! it was opened: text typed at a permission prompt answers the prompt.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, Ref, RefCell};
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::paint::{Card, Hunk};
use super::rows::{Narrow, shorten};
use crate::catalog::{self, Entry};
use crate::cli::{AgentArgs, AnswerArgs, NewArgs, StopArgs};
use crate::config::Config;
use crate::derive::View;
use crate::store::{Agent, Ask, Kind, Phase};
use crate::tmux::Server;
use crate::verbs::answer::Answered;
use crate::verbs::clear::Taken;
use crate::verbs::resume::Comeback;
use crate::{derive, exit, models, registry, spawn, store, verbs, worktree};

/// A line being typed, and what entering it does.
pub struct Composer {
    pub asking: Asking,
    pub text: String,
    /// Cursor position, in characters rather than bytes.
    pub at: usize,
    /// The permission the next agent would run with, drawn at the right end
    /// of the rule over the line.
    ///
    /// The view sets it once a frame, because the band that draws the rule is
    /// handed the line and not the view.
    pub allowed: Cell<Option<String>>,
    /// Completions for the word under the cursor.
    pub suggest: Option<Suggest>,
    /// The vendor catalog this line last read, cached across keystrokes.
    pub listed: RefCell<Option<Listed>>,
    /// The project the wall was showing when the line opened.
    ///
    /// A task runs there unless its line has a `d:`.
    pub under: Option<PathBuf>,
    /// The text each paste marker stands for: `[Pasted text #1]` is
    /// `pastes[0]`.
    pub pastes: Vec<String>,
    /// The walk through sent lines, while one is under way.
    pub walking: Option<Walking>,
}

/// A walk through the lines sent before.
pub struct Walking {
    /// Index of the line shown, newest first.
    at: usize,
    /// Text, cursor and pastes from before the walk, restored by stepping
    /// past the newest line.
    draft: (String, usize, Vec<String>),
}

/// Lines the view has sent, for the composer to recall.
///
/// Tasks and replies are separate lists, newest first, at most fifty each,
/// with an immediate repeat stored once.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backlog {
    tasks: Vec<String>,
    replies: Vec<String>,
}

/// How many lines each list keeps.
const REMEMBERED_LINES: usize = 50;

impl Backlog {
    /// Records a sent line, newest first.
    ///
    /// Only tasks, forks and replies are kept. A line equal to the newest one
    /// is not stored again.
    pub fn remember_line(&mut self, asking: &Asking, line: &str) {
        let lines = match asking {
            // A fork's first turn is a task for an agent that does not exist
            // yet, so forks share the task history.
            Asking::Task | Asking::Fork { .. } => &mut self.tasks,
            Asking::Reply => &mut self.replies,
            Asking::Name { .. } | Asking::Find => return,
        };
        if line.trim().is_empty() || lines.first().is_some_and(|newest| newest == line) {
            return;
        }
        lines.insert(0, line.to_string());
        lines.truncate(REMEMBERED_LINES);
    }

    /// The lines sent from this kind of line, newest first.
    pub fn lines_for(&self, asking: &Asking) -> &[String] {
        match asking {
            Asking::Task | Asking::Fork { .. } => &self.tasks,
            Asking::Reply => &self.replies,
            Asking::Name { .. } | Asking::Find => &[],
        }
    }
}

/// Completions for the word under the cursor, recomputed on every keystroke.
///
/// Narrowed from the catalog cached in [`Listed`].
pub struct Suggest {
    /// The word's span on the line, in characters. A completion replaces it.
    pub word: Range<usize>,
    /// Entries matching what has been typed of the word. Never empty.
    pub entries: Vec<Entry>,
    /// Index of the highlighted entry.
    pub chosen: usize,
}

/// The vendor's catalog as read for one vendor and project, kept for the life
/// of the line.
///
/// Reading it walks every place the vendor declares and parses each file's
/// frontmatter. With a plugin cache of a few hundred files that is too slow to
/// repeat per keystroke. A different vendor (`agent:`) or project (`d:`) on
/// the line invalidates it.
pub struct Listed {
    agent: String,
    project: PathBuf,
    entries: Vec<Entry>,
}

/// What entering the line does.
pub enum Asking {
    /// Start a new agent with the line as its task.
    Task,
    /// Send to the agent on the open card: a message, or an answer to its
    /// question.
    ///
    /// The agent is read off the card when enter is pressed, so it names no
    /// id here.
    Reply,
    /// Rename an agent. Nothing is sent to it.
    Name { id: String },
    /// Start a copy of an agent, as `amx fork` does, with the line as the
    /// copy's first turn. An empty line starts the copy idle.
    Fork { id: String },
    /// Narrow the wall. Applied on every keystroke and never sent.
    Find,
}

impl Composer {
    pub fn new(asking: Asking) -> Composer {
        Composer {
            asking,
            text: String::new(),
            at: 0,
            allowed: Cell::new(None),
            suggest: None,
            listed: RefCell::new(None),
            under: None,
            pastes: Vec::new(),
            walking: None,
        }
    }

    /// Recalls the next older (`older`) or newer sent line.
    ///
    /// Stepping newer past the newest restores the draft from before the walk.
    /// A recalled line is plain text with the cursor at its end and no pastes.
    /// Returns whether the line changed.
    pub fn recall(&mut self, sent: &[String], older: bool) -> bool {
        let at = match (&self.walking, older) {
            (None, true) => 0,
            (None, false) => return false,
            (Some(walking), true) => walking.at + 1,
            (Some(walking), false) => match walking.at.checked_sub(1) {
                Some(at) => at,
                None => {
                    let draft = self.walking.take().expect("a walk under way").draft;
                    (self.text, self.at, self.pastes) = draft;
                    return true;
                }
            },
        };
        let Some(line) = sent.get(at) else {
            return false;
        };
        match &mut self.walking {
            Some(walking) => walking.at = at,
            None => {
                self.walking = Some(Walking {
                    at,
                    draft: (
                        std::mem::take(&mut self.text),
                        self.at,
                        std::mem::take(&mut self.pastes),
                    ),
                });
            }
        }
        self.set_text(line.clone());
        true
    }

    /// Replaces the line with plain `text`, the cursor at its end.
    pub fn set_text(&mut self, text: String) {
        self.at = text.chars().count();
        self.text = text;
        self.pastes.clear();
    }

    /// Inserts text at the cursor and moves the cursor past it.
    pub fn insert(&mut self, text: &str) {
        let at = self.byte();
        self.text.insert_str(at, text);
        self.at += text.chars().count();
    }

    /// Inserts a paste at the cursor, folded behind a `[Pasted text #N]`
    /// marker when it is longer than `PASTED_CHARACTERS` or `PASTED_ROWS`.
    ///
    /// Pasting the same text again while its marker is still on the line
    /// unfolds the marker in place instead of adding a second one. A paste
    /// keeps its number after its marker is deleted, because
    /// [`whole`](Self::whole) maps markers to pastes by number.
    pub fn paste(&mut self, text: &str) {
        if text.chars().count() <= PASTED_CHARACTERS && text.lines().count() <= PASTED_ROWS {
            self.insert(text);
            return;
        }
        if let Some(standing) = self.standing(text) {
            self.at = self.text[..standing.start].chars().count() + text.chars().count();
            self.text.replace_range(standing, text);
            return;
        }
        self.pastes.push(text.to_string());
        self.insert(&marker(self.pastes.len()));
    }

    /// The byte range of the marker for an earlier paste equal to `text`, if
    /// that marker is still on the line.
    fn standing(&self, text: &str) -> Option<Range<usize>> {
        self.pastes
            .iter()
            .enumerate()
            .filter(|(_, pasted)| pasted == &text)
            .find_map(|(at, _)| {
                let marker = marker(at + 1);
                let from = self.text.find(&marker)?;
                Some(from..from + marker.len())
            })
    }

    /// The line as sent: every marker replaced by the paste it stands for.
    ///
    /// A marker edited into something else stays as the characters left.
    pub fn whole(&self) -> String {
        self.pastes
            .iter()
            .enumerate()
            .fold(self.text.clone(), |line, (at, pasted)| {
                line.replace(&marker(at + 1), pasted)
            })
    }

    /// Deletes the character before the cursor, or the whole marker ending
    /// there.
    ///
    /// Taking one character off a marker would leave a broken bracket in the
    /// sent text and silently drop the paste.
    pub fn delete_back(&mut self) {
        if self.at == 0 {
            return;
        }
        let to = self.byte();
        self.at -= self.marker_behind().unwrap_or(1);
        let from = self.byte();
        self.text.replace_range(from..to, "");
    }

    pub fn delete_forward(&mut self) {
        if self.at >= self.length() {
            return;
        }
        let from = self.byte();
        let to = self.byte_at(self.at + self.marker_ahead().unwrap_or(1));
        self.text.replace_range(from..to, "");
    }

    /// Deletes the word before the cursor, by the same rule as
    /// [`word_left`](Self::word_left). A marker counts as one word.
    pub fn delete_word_back(&mut self) {
        if self.marker_behind().is_some() {
            return self.delete_back();
        }
        let to = self.byte();
        self.word_left();
        let from = self.byte();
        self.text.replace_range(from..to, "");
    }

    /// The length in characters of a paste marker ending at the cursor.
    ///
    /// Only markers for pastes this line holds count; a bracket typed by hand
    /// is plain text.
    fn marker_behind(&self) -> Option<usize> {
        let before = &self.text[..self.byte()];
        self.markers()
            .find(|marker| before.ends_with(marker.as_str()))
            .map(|marker| marker.chars().count())
    }

    fn marker_ahead(&self) -> Option<usize> {
        let after = &self.text[self.byte()..];
        self.markers()
            .find(|marker| after.starts_with(marker.as_str()))
            .map(|marker| marker.chars().count())
    }

    fn markers(&self) -> impl Iterator<Item = String> {
        (1..=self.pastes.len()).map(marker)
    }

    /// Moves one character left, stopping at the start of the line.
    pub fn left(&mut self) {
        self.at = self.at.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.at = (self.at + 1).min(self.length());
    }

    /// Moves to the start of the whole line, not of the wrapped row.
    pub fn home(&mut self) {
        self.at = 0;
    }

    pub fn end(&mut self) {
        self.at = self.length();
    }

    /// Moves to the start of the previous word.
    ///
    /// Words split on whitespace only, so a dial like `m:opus` is one word.
    pub fn word_left(&mut self) {
        let line: Vec<char> = self.text.chars().collect();
        let mut at = self.at.min(line.len());
        while at > 0 && line[at - 1].is_whitespace() {
            at -= 1;
        }
        while at > 0 && !line[at - 1].is_whitespace() {
            at -= 1;
        }
        self.at = at;
    }

    pub fn word_right(&mut self) {
        let line: Vec<char> = self.text.chars().collect();
        let mut at = self.at.min(line.len());
        while at < line.len() && line[at].is_whitespace() {
            at += 1;
        }
        while at < line.len() && !line[at].is_whitespace() {
            at += 1;
        }
        self.at = at;
    }

    /// Moves the highlight through the suggestions, wrapping at both ends.
    pub fn choose(&mut self, by: isize) {
        let Some(suggest) = &mut self.suggest else {
            return;
        };
        let many = suggest.entries.len();
        if many == 0 {
            return;
        }
        suggest.chosen = (suggest.chosen + many).saturating_add_signed(by) % many;
    }

    /// Replaces the word under the cursor with the chosen suggestion and
    /// clears the suggestions.
    ///
    /// Adds a trailing space unless whitespace already follows or the entry
    /// is a directory (ending in `/`), which the next keystroke continues.
    pub fn complete(&mut self) {
        let Some(suggest) = self.suggest.take() else {
            return;
        };
        let Some(entry) = suggest.entries.get(suggest.chosen) else {
            return;
        };
        let word = match self.text.chars().nth(suggest.word.end) {
            _ if entry.spelled.ends_with('/') => entry.spelled.clone(),
            Some(after) if after.is_whitespace() => entry.spelled.clone(),
            _ => format!("{} ", entry.spelled),
        };
        let (from, to) = (
            self.byte_at(suggest.word.start),
            self.byte_at(suggest.word.end),
        );
        self.text.replace_range(from..to, &word);
        self.at = suggest.word.start + word.chars().count();
    }

    /// Whether enter should complete the word rather than submit the line:
    /// there are suggestions and the word is not yet spelled like the chosen
    /// one.
    ///
    /// Without this, typing `/review` in full leaves one suggestion (itself)
    /// and takes two enters to send.
    pub fn finishing(&self) -> bool {
        let Some(suggest) = &self.suggest else {
            return false;
        };
        let Some(entry) = suggest.entries.get(suggest.chosen) else {
            return false;
        };
        let typed: String = self
            .text
            .chars()
            .take(suggest.word.end)
            .skip(suggest.word.start)
            .collect();
        typed != entry.spelled
    }

    /// The cached catalog for this vendor and project, calling `read` when
    /// there is none or it was read for another vendor or project.
    ///
    /// `read` is a parameter so the caching can be tested without a disk.
    fn catalog(
        &self,
        agent: &str,
        project: &Path,
        read: impl FnOnce() -> Vec<Entry>,
    ) -> Ref<'_, [Entry]> {
        let stale = self
            .listed
            .borrow()
            .as_ref()
            .is_none_or(|listed| listed.agent != agent || listed.project != project);
        if stale {
            *self.listed.borrow_mut() = Some(Listed {
                agent: agent.to_string(),
                project: project.to_path_buf(),
                entries: read(),
            });
        }
        Ref::map(self.listed.borrow(), |listed| {
            listed
                .as_ref()
                .map_or(&[][..], |listed| listed.entries.as_slice())
        })
    }

    /// The cursor as a byte offset into `text`.
    fn byte(&self) -> usize {
        self.byte_at(self.at)
    }

    /// Byte offset of character `at`, clamped to the end of the line.
    fn byte_at(&self, at: usize) -> usize {
        self.text
            .char_indices()
            .nth(at)
            .map_or(self.text.len(), |(byte, _)| byte)
    }

    /// Length in characters.
    fn length(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether this is a task line starting with `!`, which runs a shell
    /// command instead of starting an agent.
    ///
    /// Read from [`whole`](Self::whole), so a pasted script folded into a
    /// marker still counts.
    pub fn commanding(&self) -> bool {
        matches!(self.asking, Asking::Task) && self.whole().starts_with(BANG)
    }

    /// The mode word drawn on the rule over the line.
    ///
    /// Uppercase so it reads at a glance. It becomes `COMMAND` as soon as the
    /// line starts with `!`.
    pub fn label(&self) -> &'static str {
        if self.commanding() {
            return "COMMAND";
        }
        match &self.asking {
            Asking::Task => "TASK",
            // Not drawn: a reply is the card's last row, under the card's
            // own rule.
            Asking::Reply => "REPLY",
            Asking::Name { .. } => "RENAME",
            Asking::Fork { .. } => "FORK",
            // Not drawn: the find line is a single row with no rule.
            Asking::Find => "FIND",
        }
    }

    /// What the line is aimed at, drawn on the rule beside the label.
    ///
    /// A rename or fork names the agent's id. A task names the project it will
    /// run in, when the line was opened under one, written the way the path
    /// headings write it.
    pub fn about(&self) -> Option<String> {
        match &self.asking {
            Asking::Task => self
                .under
                .as_deref()
                .map(|dir| format!("in {}", shorten(dir, std::env::home_dir().as_deref()))),
            // The card's own rule already names the reply's agent.
            Asking::Find | Asking::Reply => None,
            Asking::Name { id } | Asking::Fork { id } => Some(id.clone()),
        }
    }
}

/// A paste longer than this many characters, or [`PASTED_ROWS`] rows, is
/// folded behind a marker. Roughly what the composer shows at its widest and
/// tallest.
const PASTED_CHARACTERS: usize = 800;
const PASTED_ROWS: usize = 3;

/// The marker for the `nth` paste on a line, numbered from one per line.
fn marker(nth: usize) -> String {
    format!("[Pasted text #{nth}]")
}

/// Reads a find line made only of `s:` tokens as a state narrowing.
///
/// Any other word makes the line a name search, so "s:waiting is what to
/// check" still finds an agent by that text.
pub fn narrowing(line: &str) -> Option<Vec<Narrow>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() || !tokens.iter().all(|token| token.starts_with(STATE)) {
        return None;
    }

    Some(
        tokens
            .iter()
            .map(|token| {
                // A bare `s:` drops the state narrowing.
                let want = &token[STATE.len()..];
                Narrow::State((!want.is_empty()).then(|| want.to_string()))
            })
            .collect(),
    )
}

/// The find-line token that narrows by state.
pub const STATE: &str = "s:";

/// What a find line narrows the wall to.
///
/// A line of only `s:` tokens narrows by state. Anything else is one name
/// search, untokenised, so `port the` matches that text. An empty line clears
/// the name.
pub fn finding(line: &str) -> Vec<Narrow> {
    if let Some(narrowing) = narrowing(line) {
        return narrowing;
    }
    let want = line.trim();
    vec![Narrow::Name((!want.is_empty()).then(|| want.to_string()))]
}

/// The dial tokens a task line may start with.
const DIALS: [&str; 9] = [
    MODEL, PERMISSION, EFFORT, WORKTREE, DIR, AGENT, BASE, REQUEST, BRANCH,
];

/// Names the vendor; the other dials are checked against it.
const AGENT: &str = "agent:";

/// The vendor's own dials, whose values come from its registry entry.
const MODEL: &str = "m:";
const PERMISSION: &str = "p:";
const EFFORT: &str = "e:";

/// amx's own dials: worktree on or off, directory, base ref, pull request and
/// branch.
const WORKTREE: &str = "w:";
const DIR: &str = "d:";
const BASE: &str = "b:";
const REQUEST: &str = "pr:";
const BRANCH: &str = "on:";

/// The values `w:` takes. `changes` means a worktree with the uncommitted work
/// moved into it.
const TREE: [&str; 3] = ["on", "off", "changes"];

/// The mark that names a file or one of the vendor's agents.
const AT: &str = "@";

/// A task line starting with this runs the rest as a shell command.
const BANG: char = '!';

/// The dials a task line's leading tokens set. The default leaves every dial
/// to the config.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Turned {
    /// The line starts with `!` and runs a shell command.
    pub exec: bool,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub permission: Option<String>,
    pub effort: Option<String>,
    /// Whether the agent gets its own worktree, where the line said.
    pub worktree: Option<bool>,
    /// The `d:` value as typed. `~` and relative paths are resolved later,
    /// against the view's directory.
    pub dir: Option<String>,
    /// The `b:` ref as typed; the spawn resolves it.
    pub base: Option<String>,
    /// The `on:` branch as typed; the spawn finds it locally or on the
    /// origin.
    pub branch: Option<String>,
    /// The `pr:` number, which sets both the base and the branch.
    pub pr: Option<u64>,
    /// Move uncommitted work into the new worktree (`w:changes`).
    pub with_changes: bool,
}

/// The outcome of starting an agent.
pub enum Started {
    /// Started: the new id, for keys that go to it, and the line to show.
    Yes { id: String, said: String },
    /// Nothing was started, and why.
    No(String),
}

/// Splits a task line into its leading dial tokens and the task.
///
/// Only leading tokens count, so `port the m:opus importer` is all task. A
/// task that must begin with a dial word needs `amx new` at a shell prompt.
fn tokens(line: &str) -> (Vec<(&'static str, &str)>, &str) {
    let mut found = Vec::new();
    let mut rest = line;
    loop {
        let ahead = rest.trim_start();
        let word = ahead.split_whitespace().next().unwrap_or_default();
        let Some(dial) = DIALS.iter().find(|dial| word.starts_with(**dial)) else {
            break;
        };
        found.push((*dial, &word[dial.len()..]));
        rest = &ahead[word.len()..];
    }
    match found.is_empty() {
        true => (found, line),
        false => (found, rest.trim_start()),
    }
}

/// Reads a task line's dials and returns them with the remaining task, or the
/// refusal for a bad value.
///
/// Values are checked against the registry table `new` resolves against, so a
/// value the vendor would reject is refused here instead of killing the pane.
/// `agent:` takes any command, as `--agent` does; an unknown vendor just has
/// no dials. A line starting with `!` is read by [`commanded`] instead.
pub fn turned(config: &Config, line: &str) -> Result<(Turned, String), String> {
    if let Some(rest) = line.strip_prefix(BANG) {
        return commanded(rest);
    }
    let (tokens, task) = tokens(line);
    let mut turned = Turned::default();

    // The vendor first, since it decides which dials exist.
    for (dial, value) in &tokens {
        if *dial == AGENT {
            if value.is_empty() {
                return Err("agent: takes a command".to_string());
            }
            turned.agent = Some((*value).to_string());
        }
    }
    let agent = turned.agent.clone().unwrap_or_else(|| config.agent.clone());
    let entry = registry::entry(&agent);

    for (dial, value) in &tokens {
        match *dial {
            AGENT => {}
            WORKTREE => {
                turned.worktree = Some(match *value {
                    "on" => true,
                    "off" => false,
                    // Moving work implies a worktree.
                    "changes" => {
                        turned.with_changes = true;
                        true
                    }
                    _ => return Err(format!("w:{value}: on, off or changes")),
                });
            }
            DIR => {
                if value.is_empty() {
                    return Err("d: takes a directory".to_string());
                }
                turned.dir = Some((*value).to_string());
            }
            BASE => {
                if value.is_empty() {
                    return Err("b: takes a ref".to_string());
                }
                turned.base = Some((*value).to_string());
            }
            REQUEST => {
                let Ok(number) = value.parse::<u64>() else {
                    return Err("pr: takes a number".to_string());
                };
                turned.pr = Some(number);
            }
            BRANCH => {
                if value.is_empty() {
                    return Err("on: takes a branch".to_string());
                }
                turned.branch = Some((*value).to_string());
            }
            MODEL => {
                turned.model = Some(pointed(&agent, dial, entry.and_then(|e| e.model), value)?);
            }
            EFFORT => {
                turned.effort = Some(pointed(&agent, dial, entry.and_then(|e| e.effort), value)?);
            }
            _ => {
                turned.permission = Some(pointed(
                    &agent,
                    dial,
                    entry.and_then(|e| e.permission),
                    value,
                )?);
            }
        }
    }

    // The flag pairs clap refuses, checked after all tokens so order does not
    // matter.
    if turned.pr.is_some() {
        if turned.base.is_some() {
            return Err("pr: and b: — a request says what it is cut from".to_string());
        }
        if turned.worktree == Some(false) || turned.with_changes {
            return Err("pr: and w: — a request is a tree of its own".to_string());
        }
    }
    if turned.branch.is_some() {
        if turned.base.is_some() {
            return Err("on: and b: — a branch says what it is cut from".to_string());
        }
        if turned.pr.is_some() {
            return Err("on: and pr: — a request is a branch of its own".to_string());
        }
        // `w:changes` is allowed: the uncommitted work can go on that branch.
        if turned.worktree == Some(false) {
            return Err("on: and w:off — a branch is a tree of its own".to_string());
        }
    }
    Ok((turned, task.to_string()))
}

/// Reads a command row's dials and returns them with the command.
///
/// Only `d:` applies. A `sh -c` row launches no vendor, so the vendor dials
/// and `agent:` mean nothing (as `--exec` refuses them), and it runs in the
/// checkout it was typed in, so `w:`, `b:`, `pr:` and `on:` do not apply.
fn commanded(rest: &str) -> Result<(Turned, String), String> {
    let (tokens, command) = tokens(rest);
    let mut turned = Turned {
        exec: true,
        ..Turned::default()
    };
    for (dial, value) in &tokens {
        if *dial != DIR {
            return Err(format!(
                "{dial}{value}: a command row takes d: and no other dial"
            ));
        }
        if value.is_empty() {
            return Err("d: takes a directory".to_string());
        }
        turned.dir = Some((*value).to_string());
    }
    Ok((turned, command.trim_start().to_string()))
}

/// Checks a value for one of the vendor's own dials.
fn pointed(
    agent: &str,
    dial: &str,
    spec: Option<registry::DialSpec>,
    value: &str,
) -> Result<String, String> {
    let Some(spec) = spec else {
        return Err(format!(
            "{dial}{value}: amx knows no such dial for {}",
            registry::program(agent)
        ));
    };
    if value.is_empty() {
        return Err(format!("{dial} takes a value"));
    }
    if !registry::accepts(&spec, value) {
        return Err(format!(
            "{dial}{value}: {} takes {}",
            registry::program(agent),
            spec.cycle.join(", ")
        ));
    }
    Ok(value.to_string())
}

/// Completions for the word under the cursor, if any.
///
/// Only on lines whose text reaches a vendor (task, reply, fork), and never on
/// a `!` command row, where `/etc` is a path. Runs per keystroke: a plain word
/// costs nothing, a path reads a directory, `on:` runs git, and the vendor
/// catalog is read once per line.
///
/// `wall` is the projects on the wall, offered to `d:` beside the
/// directories under it.
pub fn suggest(
    composer: &Composer,
    config: &Config,
    project: &Path,
    wall: &[PathBuf],
) -> Option<Suggest> {
    // On a reply or fork, `config` and `project` are the agent's own. Only
    // the task line takes dials; see [`answering`].
    if !matches!(
        composer.asking,
        Asking::Task | Asking::Reply | Asking::Fork { .. }
    ) || composer.commanding()
    {
        return None;
    }
    let word = under_the_cursor(&composer.text, composer.at)?;
    let typed: String = composer
        .text
        .chars()
        .take(word.end)
        .skip(word.start)
        .collect();

    let entries = answering(composer, &typed, config, project, wall);
    (!entries.is_empty()).then_some(Suggest {
        word,
        entries,
        chosen: 0,
    })
}

/// The span of the word containing the cursor, in characters, or nothing on
/// whitespace.
///
/// The whole word, not just the part before the cursor, so a completion
/// replaces a word being corrected in the middle.
fn under_the_cursor(text: &str, at: usize) -> Option<Range<usize>> {
    let line: Vec<char> = text.chars().collect();
    let (mut from, mut to) = (at.min(line.len()), at.min(line.len()));
    while from > 0 && !line[from - 1].is_whitespace() {
        from -= 1;
    }
    while to < line.len() && !line[to].is_whitespace() {
        to += 1;
    }
    (from < to).then_some(from..to)
}

/// The vendor the line's words are for: its `agent:` token, else the
/// configured one.
fn asked_of(config: &Config, line: &str) -> String {
    let (tokens, _) = tokens(line);
    tokens
        .iter()
        .find(|(dial, value)| *dial == AGENT && !value.is_empty())
        .map_or_else(|| config.agent.clone(), |(_, value)| (*value).to_string())
}

/// The entries that complete the word being typed.
///
/// On a task line the dials come first: `agent:` from the registry, `m:`,
/// `p:` and `e:` from the vendor's values, `w:` from [`TREE`], `on:` from the
/// checkout's branches and `d:` from directories. Then the vendor's sigil
/// (`/`, or `$` for codex) lists skills, commands and built-ins, and `@` lists
/// its agents, falling back to file paths.
///
/// A vendor with no catalog places, or a machine with no home directory,
/// still gets file paths.
fn answering(
    composer: &Composer,
    typed: &str,
    config: &Config,
    project: &Path,
    wall: &[PathBuf],
) -> Vec<Entry> {
    let line = composer.text.as_str();
    // Dials only on the task line. On a reply they are words of the message,
    // and the vendor is the running agent's own.
    let starting = matches!(composer.asking, Asking::Task);
    let agent = match starting {
        true => asked_of(config, line),
        false => config.agent.clone(),
    };
    if starting {
        if typed.starts_with(AGENT) {
            return vendors(typed);
        }
        if let Some(values) = dialled(&agent, typed, config) {
            return values;
        }
        // Branches are read where the agent will run, the checkout its `d:`
        // names.
        if typed.starts_with(BRANCH) {
            return branches_here(&running(line, project), typed);
        }
        // A `d:` itself is read against the view's directory, not against a
        // `d:`.
        if typed.starts_with(DIR) {
            let mut found = paths(DIR, typed, project, true);
            found.extend(on_the_wall(wall, typed));
            return found;
        }
    }

    // `/` for claude and pi, `$` for codex; on codex `/` lists nothing.
    let sigil = registry::entry(&agent)
        .and_then(|vendor| vendor.catalog)
        .map(|catalog| catalog.sigil);
    let kinds: &[catalog::Kind] = match typed.chars().next() {
        Some('@') => &[catalog::Kind::Agent],
        first if first.is_some() && first == sigil => &[
            catalog::Kind::Skill,
            catalog::Kind::Command,
            catalog::Kind::Builtin,
        ],
        _ => return Vec::new(),
    };
    let listed = composer.catalog(&agent, project, || catalogued(&agent, project));
    let named = named(&listed, typed, kinds);
    // Paths on a task line are read where its `d:` says; on a reply, where
    // the agent runs.
    let here = match starting {
        true => running(line, project),
        false => project.to_path_buf(),
    };
    match named.is_empty() && typed.starts_with(AT) {
        true => paths(AT, typed, &here, false),
        false => named,
    }
}

/// Everything the vendor loads by name, from the places its entry declares.
fn catalogued(agent: &str, project: &Path) -> Vec<Entry> {
    let places = registry::entry(agent).and_then(|vendor| vendor.catalog);
    let (Some(places), Some(home)) = (places, std::env::home_dir()) else {
        return Vec::new();
    };
    catalog::listing(&places, &home, project)
}

/// The entries of these kinds whose spelling starts with `typed`.
fn named(entries: &[Entry], typed: &str, kinds: &[catalog::Kind]) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| kinds.contains(&entry.kind) && entry.spelled.starts_with(typed))
        .cloned()
        .collect()
}

/// `agent:` completions for every vendor in the registry.
fn vendors(typed: &str) -> Vec<Entry> {
    registry::entries()
        .iter()
        .map(|vendor| worded(format!("{AGENT}{}", vendor.name)))
        .filter(|entry| entry.spelled.starts_with(typed))
        .collect()
}

/// Completions for a dial value, or `None` when the word is not a dial.
///
/// `p:` and `e:` offer the vendor's cycle, the same list `turned` checks
/// against, so nothing is offered that the spawn would refuse. `m:` offers
/// [`models::models_of`], the list the model key walks, plus the default
/// sentinel. A dial the vendor does not declare offers nothing.
fn dialled(agent: &str, typed: &str, config: &Config) -> Option<Vec<Entry>> {
    let vendor = registry::entry(agent);
    if typed.starts_with(MODEL) {
        // No model dial, no values.
        let _model = vendor?.model?;
        return Some(
            std::iter::once(registry::DEFAULT.to_string())
                .chain(models::models_of(vendor?, config))
                .map(|value| worded(format!("{MODEL}{value}")))
                .filter(|entry| entry.spelled.starts_with(typed))
                .collect(),
        );
    }
    let (dial, cycle): (&str, &[&str]) = match typed {
        _ if typed.starts_with(PERMISSION) => (PERMISSION, vendor?.permission?.cycle),
        _ if typed.starts_with(EFFORT) => (EFFORT, vendor?.effort?.cycle),
        _ if typed.starts_with(WORKTREE) => (WORKTREE, &TREE),
        _ => return None,
    };
    Some(
        cycle
            .iter()
            .map(|value| worded(format!("{dial}{value}")))
            .filter(|entry| entry.spelled.starts_with(typed))
            .collect(),
    )
}

/// `on:` completions from the local branches of the checkout at `here`.
///
/// Local refs only: listing the origin's would mean a network call. A
/// directory outside a repository, or a failing git, offers nothing.
pub fn branches_here(here: &Path, typed: &str) -> Vec<Entry> {
    let read = std::process::Command::new("git")
        .current_dir(here)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .stdin(std::process::Stdio::null())
        .output();
    let Ok(read) = read else {
        return Vec::new();
    };
    if !read.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&read.stdout)
        .lines()
        .map(|branch| worded(format!("{BRANCH}{branch}")))
        .filter(|entry| entry.spelled.starts_with(typed))
        .collect()
}

/// Completions for a path word, from the directory it names.
///
/// Read the way a shell in `here` would: `~` is home and a relative name is
/// under `here`. Results keep the word's mark and prefix, and directories end
/// in `/` so completing one adds no space. `.git` and `.amx` are left out.
/// `folders` limits the results to directories, for `d:`.
fn paths(mark: &str, typed: &str, here: &Path, folders: bool) -> Vec<Entry> {
    let said = typed.strip_prefix(mark).unwrap_or(typed);
    let (dir, leaf) = match said.rfind('/') {
        Some(at) => said.split_at(at + 1),
        None => ("", said),
    };
    // A directory that is missing or unreadable offers nothing.
    let Ok(at) = aimed(dir, here) else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(at) else {
        return Vec::new();
    };

    let mut found: Vec<Entry> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let folder = entry.path().is_dir();
            let offered = name.starts_with(leaf)
                && !KEPT_BACK.contains(&name.as_str())
                && (folder || !folders);
            let slash = if folder { "/" } else { "" };
            offered.then(|| worded(format!("{mark}{dir}{name}{slash}")))
        })
        .collect();
    // Sorted so the result does not depend on the filesystem's order.
    found.sort_by(|one, two| one.spelled.cmp(&two.spelled));
    found
}

/// Directories never offered as a path.
const KEPT_BACK: [&str; 2] = [".git", ".amx"];

/// `d:` completions for every project an agent on the wall runs in.
///
/// People usually start an agent where they already have one, and those
/// directories are rarely under the view's own.
fn on_the_wall(wall: &[PathBuf], typed: &str) -> Vec<Entry> {
    wall.iter()
        .map(|project| worded(format!("{DIR}{}", project.display())))
        .filter(|entry| entry.spelled.starts_with(typed))
        .collect()
}

/// The directory paths on this line are read against: the line's `d:` where
/// it names a directory, else `project`.
///
/// A `d:` naming nothing yet is still being typed.
fn running(line: &str, project: &Path) -> PathBuf {
    let (tokens, _) = tokens(line);
    tokens
        .iter()
        .find(|(dial, value)| *dial == DIR && !value.is_empty())
        .and_then(|(_, value)| aimed(value, project).ok())
        .unwrap_or_else(|| project.to_path_buf())
}

/// A completion amx supplies itself (vendor, dial value, path), with no
/// description.
fn worded(spelled: String) -> Entry {
    Entry {
        spelled,
        kind: catalog::Kind::Builtin,
        about: String::new(),
    }
}

/// The outcome of editing a line in `$EDITOR`.
pub enum Edited {
    /// The editor saved this, and it replaces the line.
    Line(String),
    /// The editor exited with an error, so the line stays. Holds the message.
    No(String),
}

/// The editor to run: `$VISUAL`, then `$EDITOR`, then `vi`.
///
/// `$VISUAL` first because it names the editor for an interactive terminal,
/// which is where this line is being edited.
fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|said| !said.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Opens the line in an editor and returns what it saved.
///
/// Through a file and `sh -c`, as `git commit` does, because `$EDITOR` is
/// often a command line of its own.
pub fn edited(text: &str) -> Result<Edited> {
    let path = std::env::temp_dir().join(format!("amx-task-{}.md", std::process::id()));
    edited_in(&editor(), &path, text)
}

/// [`edited`] with the editor and file named, for tests.
fn edited_in(editor: &str, path: &Path, text: &str) -> Result<Edited> {
    // The temp directory is shared and the name is predictable, so remove any
    // file there and create it with create_new instead of truncating, which
    // would write through a planted symlink.
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    drop(file);
    crate::paths::keep_to_the_owner(path, 0o600)?;

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} {}", quoted(path)))
        .status()
        .with_context(|| format!("running {editor}"))?;
    let written = std::fs::read_to_string(path);
    // Never leave the task in a world-readable directory.
    let _ = std::fs::remove_file(path);

    if !status.success() {
        return Ok(Edited::No(format!("{editor} left the line as it was")));
    }
    let written = written.with_context(|| format!("reading {} back", path.display()))?;
    // Drop the file's trailing newline; newlines inside are the task's own.
    Ok(Edited::Line(
        written.trim_end_matches('\n').replace("\r\n", "\n"),
    ))
}

/// A path quoted as one `sh` word.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Tasks shorter than this many characters are confirmed before starting.
///
/// A stray keystroke after `n` opens the line leaves a one-letter task, and
/// starting an agent on it costs a pane and a record. Short tasks are asked
/// about, not refused, since "wip" can be meant.
const ENOUGH: usize = 4;

/// The task on this line if it is too short to start without asking.
///
/// Measured on the task without its dials: `m:opus fix` is a three-character
/// task. A `!` command row is never asked about, since `ls` is a real
/// command.
pub fn slight(config: &Config, line: &str) -> Option<String> {
    let (turned, task) = turned(config, line).ok()?;
    if turned.exec {
        return None;
    }
    // Quoted back on the footer's one row, so collapse newlines.
    let task = task.split_whitespace().collect::<Vec<_>>().join(" ");
    (!task.is_empty() && task.chars().count() < ENOUGH).then_some(task)
}

/// Starts an agent on the typed line, or runs it where it is a `!` command
/// row.
///
/// The agent runs in the line's `d:` if it has one, else `under` (the project
/// the line was opened under), else `here` (the view's directory).
pub fn start(
    root: &Path,
    config: &Config,
    line: &str,
    under: Option<&Path>,
    here: &Path,
) -> Result<Started> {
    let (turned, task) = match turned(config, line) {
        Ok(read) => read,
        Err(refusal) => return Ok(Started::No(refusal)),
    };
    if task.trim().is_empty() {
        return Ok(Started::No(match turned.exec {
            true => "the row is a command; now say what to run".to_string(),
            false => "the dials are turned; now say what to do".to_string(),
        }));
    }

    let here = here.to_path_buf();
    // Resolved before anything is made, so a `d:` naming nothing is refused
    // instead of leaving a half-made spawn. A relative `d:` is read against the
    // view's directory, as a shell prompt there would.
    let dir = match &turned.dir {
        Some(said) => match aimed(said, &here) {
            Ok(dir) => dir,
            Err(refusal) => return Ok(Started::No(refusal)),
        },
        None => under.map_or(here, Path::to_path_buf),
    };
    // Checked here so the refusal names `w:changes`, not the `new` flag
    // nobody typed.
    if turned.with_changes && !worktree::has_changes_to_carry(&dir)? {
        return Ok(Started::No(format!(
            "w:changes: nothing in {} to move",
            dir.display()
        )));
    }
    // An `@agent` is looked up in the project the agent runs in. A command row
    // passes its text to the shell unchanged.
    let (vendor_args, task) = match turned.exec {
        true => (Vec::new(), task),
        false => {
            let agent = turned.agent.clone().unwrap_or_else(|| config.agent.clone());
            as_agent(&agent, &task, &dir)
        }
    };

    // `new` has a flag for going without a worktree and none for forcing one,
    // so a `w:` is applied to the config instead.
    let mut config = config.clone();
    if let Some(worktree) = turned.worktree {
        config.worktrees = worktree;
    }

    let dials = AgentArgs {
        command: turned.agent,
        model: turned.model,
        permission: turned.permission,
        effort: turned.effort,
    };
    let named = dials.command.is_some()
        || dials.model.is_some()
        || dials.permission.is_some()
        || dials.effort.is_some();
    let args = NewArgs {
        task: Some(task),
        // The view passes the task inline; `ctrl+g` is its editor.
        file: None,
        edit: false,
        name: None,
        role: None,
        dir: None,
        no_worktree: false,
        base: turned.base,
        branch: turned.branch,
        pr: turned.pr,
        with_changes: turned.with_changes,
        exec: turned.exec,
        agent: named.then_some(dials),
        vendor_args,
        // A context brief is a subagent's preamble, and a view spawn has no
        // parent.
        context_brief: None,
        parent: None,
    };

    spawned(root, &dir, &config, &args, "started")
}

/// Turns a leading `@name` into the vendor's agent flag, returning the argv
/// and the task without the word.
///
/// Only at the front of the task, and only for an agent in the vendor's own
/// catalog: the flag goes to the vendor, which fails the pane on a name it does
/// not know. Otherwise, or for a vendor with no agent flag, the task is left
/// whole.
fn as_agent(agent: &str, task: &str, project: &Path) -> (Vec<String>, String) {
    let whole = || (Vec::new(), task.to_string());
    let Some(flag) = registry::entry(agent)
        .and_then(|vendor| vendor.catalog)
        .and_then(|catalog| catalog.agent_flag)
    else {
        return whole();
    };

    let rest = task.trim_start();
    let word = rest.split_whitespace().next().unwrap_or_default();
    let Some(name) = word.strip_prefix(AT).filter(|name| !name.is_empty()) else {
        return whole();
    };
    if !catalogued(agent, project)
        .iter()
        .any(|entry| entry.kind == catalog::Kind::Agent && entry.spelled == word)
    {
        return whole();
    }

    (
        vec![flag.to_string(), name.to_string()],
        rest[word.len()..].trim_start().to_string(),
    )
}

/// Resolves a `d:` value as a shell in `here` would (`~` is home, relative is
/// under `here`), or says why it names no directory.
fn aimed(said: &str, here: &Path) -> Result<PathBuf, String> {
    let path = match said.strip_prefix('~') {
        Some(under) => match std::env::home_dir() {
            Some(home) => home.join(under.trim_start_matches('/')),
            None => return Err(format!("d:{said}: there is no home directory")),
        },
        None => here.join(said),
    };
    if !path.is_dir() {
        return Err(format!("d:{said}: nothing is at {}", path.display()));
    }
    Ok(path)
}

/// Runs `new` and reduces its output to the view's one line.
fn spawned(
    root: &Path,
    dir: &Path,
    config: &Config,
    args: &NewArgs,
    said: &str,
) -> Result<Started> {
    let (mut started, mut refused) = (Vec::new(), Vec::new());
    let code = verbs::new::run(
        root,
        dir,
        spawn::env_snapshot(std::env::vars()),
        config,
        args,
        &mut started,
        &mut refused,
    )?;
    if code != exit::OK {
        return Ok(Started::No(one_line(&refused)));
    }
    // On success `new` prints only the id.
    let id = one_line(&started);
    Ok(Started::Yes {
        said: format!("{said} {id}"),
        id,
    })
}

/// Starts a copy of an agent via `amx fork`, with `task` as its first turn or
/// none.
///
/// The verb decides everything (which session to copy, whether the vendor can
/// fork, the project cap) and its stderr is captured for the view's line.
pub fn spawn_copy(root: &Path, id: &str, task: Option<&str>) -> Result<Started> {
    let (mut out, mut problems) = (Vec::new(), Vec::new());
    let code = verbs::fork::run(
        root,
        id,
        task,
        &spawn::env_snapshot(std::env::vars()),
        &mut out,
        &mut problems,
        false,
    )?;
    if code != exit::OK {
        return Ok(Started::No(one_line(&problems)));
    }
    // On success `fork` prints only the copy's id. The line names both
    // agents.
    let copy = one_line(&out);
    Ok(Started::Yes {
        said: format!("forked {id} as {copy}"),
        id: copy,
    })
}

/// The outcome of a reply.
pub enum Replied {
    /// Delivered, with what was done.
    Yes(String),
    /// Nothing was sent, and why.
    No(String),
}

/// Sends a line to an agent, routed by its phase at this moment.
///
/// A waiting agent is answered with `amx answer`'s grammar: choices, boxes
/// checked by name, and words only where the prompt has a row for them. Words
/// at a permission box would select whatever is highlighted, so the verb
/// refuses them before anything reaches the pane.
///
/// An ended or parked agent is resumed with the line, as `amx resume <id>
/// "message"` does. A command row has no vendor to resume and is refused.
pub fn reply(root: &Path, id: &str, text: &str) -> Result<Replied> {
    let view = derive::view(root, id, store::now())?;
    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());

    match view.phase() {
        Phase::Waiting => {
            let typed = card_line(text, view.state.pending());
            match verbs::answer::given(&agent, &server, &view, &typed)? {
                Answered::Yes => Ok(Replied::Yes(format!("answered {id}"))),
                Answered::No(refused) => Ok(Replied::No(format!("{id}: {refused}"))),
            }
        }
        phase if phase.is_terminal() || parked(&view) => {
            if view.meta.agent.is_none() {
                return Ok(Replied::No(format!(
                    "{id} is {}; nothing is listening",
                    verbs::interrupt::doing(&view)
                )));
            }
            let env = spawn::env_snapshot(std::env::vars());
            match verbs::resume::picked_up(root, id, Some(text), &env)? {
                Comeback::Back => Ok(Replied::Yes(format!("resumed {id}"))),
                Comeback::No(why) => Ok(Replied::No(why)),
            }
        }
        _ => {
            verbs::send::deliver(&agent, &server, &view.meta.pane, text)?;
            Ok(Replied::Yes(format!("sent to {id}")))
        }
    }
}

/// A card-line comment on a hunk: `path:line`, the hunk in a `diff` fence, then
/// the words.
///
/// The fence grows a backtick while the hunk contains one that long, since a
/// patch to a markdown file often holds a triple backtick that would close it.
pub fn on_hunk(hunk: &Hunk, words: &str) -> String {
    let mut fence = String::from("```");
    while hunk.text.contains(&fence) {
        fence.push('`');
    }
    format!(
        "{}:{}\n\n{fence}diff\n{}\n{fence}\n\n{words}",
        hunk.path, hunk.line, hunk.text
    )
}

/// A whole review as one message: the opening, then each commented hunk in
/// patch order.
///
/// Sent as one turn so the agent does not start on the first hunk before the
/// rest arrive. A single hunk with no opening is byte-for-byte what
/// [`on_hunk`] writes. A blank opening is omitted.
pub fn on_hunks(opening: &str, notes: &[(&Hunk, &str)]) -> String {
    let opening = (!opening.trim().is_empty()).then(|| opening.to_string());
    opening
        .into_iter()
        .chain(notes.iter().map(|(hunk, words)| on_hunk(hunk, words)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Whether a line typed at this agent would reach it, for the card to say so
/// before anybody types.
///
/// A live agent takes it; an ended or parked one takes it by being resumed,
/// if there is a session to resume. An ended command row has no vendor to
/// resume. Reads the same record as [`reply`], so the card and the keystroke
/// agree.
pub fn listening(root: &Path, view: &View) -> bool {
    if !(view.phase().is_terminal() || parked(view)) {
        return true;
    }
    view.meta.agent.is_some()
        && Agent::open(root, view.id())
            .is_ok_and(|agent| verbs::resume::can_come_back(&view.meta, agent.dir()))
}

/// Whether amx took this agent's pane while it sat idle.
///
/// A parked agent still reads idle but nothing is running, so a line to it is
/// a resume, not a send.
fn parked(view: &View) -> bool {
    view.verdict.evidence == derive::Evidence::LetGo
}

/// The card line as `amx answer` arguments.
///
/// The whole line goes as the key, and the verb tells a key, choices and
/// words apart as it does at a shell prompt. The exception is a question whose
/// choices carry a preview: claude 2.1.240 draws a notes field there and no
/// free-text row, so a key followed by words is the key plus a note.
fn card_line(text: &str, asked: Option<&Ask>) -> AnswerArgs {
    let text = text.trim();
    let whole = |key: &str| AnswerArgs {
        key: Some(key.to_string()),
        text: None,
        note: None,
    };

    if !asked.is_some_and(Ask::takes_notes) {
        return whole(text);
    }
    match text.split_once(char::is_whitespace) {
        Some((key, note)) if verbs::answer::named(key).is_some() && !note.trim().is_empty() => {
            AnswerArgs {
                key: Some(key.to_string()),
                text: None,
                note: Some(note.trim().to_string()),
            }
        }
        _ => whole(text),
    }
}

/// Whether a digit on this card is the answer itself, sent on the press.
///
/// Only for the vendor's own question with read choices that takes one
/// choice, where the vendor's screen also submits on the digit. Not for:
///
/// - a multi-choice question, where a digit checks one box;
/// - a question with previews, where a note follows the key;
/// - a permission box, whose numbers are amx's reading of the pane and whose
///   allow cannot be undone;
/// - a walked list (pi dialogs included), whose numbers are amx's own and
///   whose press would be an enter on a row, also an irreversible allow.
pub fn picks(kind: Option<Kind>, options: &[String], asked: Option<&Ask>, walked: bool) -> bool {
    kind == Some(Kind::Question)
        && !walked
        && !options.is_empty()
        && !asked.is_some_and(|ask| ask.multi || ask.takes_notes())
}

/// The card's hint for what this prompt takes. The verb refuses input in the
/// same terms.
///
/// Mentions checking several boxes only for a multi-choice question and notes
/// only for one with previews. Numbers appear only when amx has read the
/// choices, and are described as picking on the press where [`picks`] says
/// so. On a walked list the numbers are the only input.
pub fn invitation(
    kind: Option<Kind>,
    options: &[String],
    asked: Option<&Ask>,
    walked: bool,
) -> String {
    let numbers = match options.len() {
        0 => None,
        1 => Some("1".to_string()),
        many => Some(format!("1-{}", many.min(9))),
    };
    if walked && let Some(numbers) = &numbers {
        return format!("press {numbers}");
    }
    let choices = numbers.map(|numbers| match picks(kind, options, asked, walked) {
        true => format!("{numbers} picks"),
        false => format!("press {numbers}"),
    });
    let Some(choices) = choices else {
        return match kind {
            Some(Kind::Question) => "type an answer".to_string(),
            // The vendor numbers nothing on a trust screen, and the verb
            // takes only a walk or `esc` there.
            Some(Kind::Trust) => "type down enter, up enter or esc".to_string(),
            _ => "press y, n or 1-9".to_string(),
        };
    };

    let several = match asked.is_some_and(|ask| ask.multi) {
        true => format!("{choices}, 1,3 for several"),
        false => choices,
    };
    match (kind, asked) {
        (Some(Kind::Question), Some(ask)) if ask.takes_notes() => {
            format!("{several}, and words after it are a note")
        }
        (Some(Kind::Question), _) => format!("{several}, or type an answer"),
        // A pending `AskUserQuestion` is the vendor's menu whatever kind an
        // older amx recorded, and a menu has no y or n. The verb refuses words
        // here too.
        (_, Some(_)) => several,
        _ => format!("{several}, y or n"),
    }
}

/// `ctrl+r` renames through the verb, so limits, refusals and the record
/// write match `amx rename`.
pub use crate::verbs::rename::{Renamed, rename};

/// Records that the agent has been looked at.
///
/// Like a rename, this does not move the record's own clock, and a look that
/// changes nothing writes nothing.
pub fn looked(root: &Path, id: &str) -> Result<()> {
    let agent = Agent::open(root, id)?;
    agent.writer()?.observe(|state| state.seen = store::now())?;
    Ok(())
}

/// Stops an agent: the pane goes, the record stays.
///
/// Uses the verb's defaults for what a prompt would ask: the worktree goes
/// (unless it holds uncommitted work), the branch and record stay. Deleting
/// the record is [`forget`], a separate keystroke.
pub fn stop(root: &Path, view: &View) -> Result<String> {
    let args = StopArgs {
        id: view.id().to_string(),
        force: true,
        delete: false,
        worktree: None,
        branch: None,
    };
    let mut said = Vec::new();
    let mut nobody = std::io::empty();
    verbs::stop::run(root, &args, &mut nobody, &mut said)?;
    Ok(one_line(&said))
}

/// Forgets an ended agent through `amx clear`'s code, which keeps a worktree
/// holding uncommitted work and the record naming it.
fn forgetting(root: &Path, view: &View) -> Result<Taken> {
    verbs::clear::forget_row(root, view)
}

/// Forgets an agent, returning the view's line and whether a worktree was
/// kept.
///
/// The view calls this on the second press of an armed row. The flag lets the
/// caller show a kept worktree as a refusal.
pub fn forget(root: &Path, view: &View) -> Result<(String, bool)> {
    Ok(match forgetting(root, view)? {
        Taken::Gone => (format!("{} forgotten", view.id()), false),
        Taken::Holding(tree) => (
            format!(
                "keeping {}: {} holds work no commit has",
                view.id(),
                tree.display()
            ),
            true,
        ),
    })
}

/// Forgets each of these, returning a summary line and whether any worktree
/// was kept.
///
/// Goes row by row through the single-row path, so the same worktree safety
/// applies. One failure does not stop the rest; it is reported at the end.
pub fn forget_all(root: &Path, views: &[&View]) -> Result<(String, bool)> {
    let (mut gone, mut kept) = (0, 0);
    let mut trouble = Vec::new();
    for view in views {
        match forgetting(root, view) {
            Ok(Taken::Gone) => gone += 1,
            Ok(Taken::Holding(_)) => kept += 1,
            Err(e) => trouble.push(format!("{}: {e:#}", view.id())),
        }
    }

    let mut said = format!("forgot {gone}");
    if kept > 0 {
        said.push_str(&format!(" · kept {kept} holding work no commit has"));
    }
    // An error so the view draws it as a failure: part of the request did not
    // happen.
    if !trouble.is_empty() {
        bail!("{said} · {} would not go: {}", trouble.len(), trouble[0]);
    }
    Ok((said, kept > 0))
}

/// The agent's full patch, as a card.
///
/// Taken once when asked; running `git diff` on every reading would cost too
/// much.
pub fn changes(root: &Path, view: &View) -> Result<Card> {
    // The full patch, not `--stat`.
    let mut patch = Vec::new();
    verbs::diff::run(root, view.id(), false, &mut patch)?;

    let patch = String::from_utf8_lossy(&patch).into_owned();
    Ok(Card {
        id: view.id().to_string(),
        phase: view.phase(),
        question: None,
        options: Vec::new(),
        walked: false,
        kind: None,
        body: match patch.trim().is_empty() {
            true => "nothing changed yet".to_string(),
            false => patch,
        },
        changes: true,
        answer: false,
        listening: listening(root, view),
        queued: Vec::new(),
    })
}

/// Opens the row's pull request in the browser via the forge CLI.
///
/// The CLI holds the login and knows the repository. The cached look does not
/// record which forge answered, so gh is tried first and glab after, the order
/// `pr.rs` uses. Not waited on in the draw loop, since it can block until
/// the browser tab closes.
pub fn open(view: &View, number: u64) -> Result<()> {
    let at = match &view.meta.worktree {
        Some(tree) if tree.is_dir() => tree.clone(),
        _ => view.meta.dir.clone(),
    };
    opened(&at, Path::new("gh"), Path::new("glab"), number)
}

/// [`open`] with the forge commands named, so tests can use fakes instead of
/// the machine's gh.
fn opened(at: &Path, gh: &Path, glab: &Path, number: u64) -> Result<()> {
    match browse(at, gh, "pr", number) {
        // No gh installed: try glab.
        Err(trouble) if trouble.kind() == std::io::ErrorKind::NotFound => {
            browse(at, glab, "mr", number).map_err(|trouble| match trouble.kind() {
                std::io::ErrorKind::NotFound => anyhow!("neither gh nor glab is on the PATH"),
                _ => anyhow!("running glab: {trouble}"),
            })
        }
        went => went.map_err(|trouble| anyhow!("running gh: {trouble}")),
    }
}

/// Starts one forge CLI to open a request in the browser.
fn browse(at: &Path, forge: &Path, request: &str, number: u64) -> std::io::Result<()> {
    let mut child = std::process::Command::new(forge)
        .current_dir(at)
        .args([request, "view", &number.to_string(), "--web"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Waited on off the draw loop, or every press leaves a zombie for the
    // life of the view.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Runs a key-bound command for an agent, in its worktree or else its
/// directory.
///
/// Through `sh -c`, since the binding is a command line. The command owns the
/// terminal until it exits, so only a failure to start or a non-zero exit is
/// reported.
pub fn run_bound(root: &Path, id: &str, command: &str) -> Result<()> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let dir = meta.worktree.clone().unwrap_or_else(|| meta.dir.clone());
    // Otherwise the spawn fails with ENOENT, which reads as the command
    // missing. A stop removes a clean tree, so this is the common case.
    if !dir.is_dir() {
        bail!("{} is gone", dir.display());
    }

    let ended = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&dir)
        .envs(crate::errand::surroundings(&agent, &meta))
        .status()
        .with_context(|| format!("running `{command}`"))?;

    match ended.code() {
        // Killed by a signal, e.g. ctrl+c closing a pager: not an error.
        Some(exit::OK) | None => Ok(()),
        Some(code) => bail!("{command} exited {code}"),
    }
}

/// A verb's output as one line, non-empty lines joined with ` · `.
fn one_line(written: &[u8]) -> String {
    String::from_utf8_lossy(written)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawn::Handoff;
    use crate::store::{Choice, Meta};
    use crate::tmux::{PaneId, Socket};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn a_line_says_what_it_is_aimed_at_before_anybody_types_into_it() {
        // A task opened outside any project names nothing.
        assert_eq!(Composer::new(Asking::Task).about(), None);

        // Opened under a project, it names where it will run.
        let mut under = Composer::new(Asking::Task);
        under.under = Some(PathBuf::from("/src/api"));
        assert_eq!(under.about().as_deref(), Some("in /src/api"));

        // A reply's agent is already named by the card's rule.
        assert_eq!(Composer::new(Asking::Reply).about(), None);

        let rename = Composer::new(Asking::Name {
            id: "fix-login-b2c".to_string(),
        });
        assert_eq!(rename.about().as_deref(), Some("fix-login-b2c"));
    }

    #[test]
    fn composer_puts_what_is_typed_where_the_cursor_stands() {
        let mut line = Composer::new(Asking::Task);
        line.insert("port the imprter");
        assert_eq!(line.at, 16, "a line stands at the end of what is on it");

        // Back to just after the `r` of "imprter".
        for _ in 0..4 {
            line.left();
        }
        line.insert("o");
        assert_eq!(line.text, "port the importer");
        assert_eq!(line.at, 13, "and the cursor is after what was typed");

        // Counted in characters, not bytes.
        let mut line = Composer::new(Asking::Name {
            id: "fix-login-a1b".to_string(),
        });
        line.insert("a é c");
        line.left();
        line.left();
        line.insert("b");
        assert_eq!(line.text, "a éb c");
    }

    #[test]
    fn composer_folds_a_long_paste_behind_a_marker_and_sends_what_it_holds() {
        // A short paste goes in as text.
        let mut line = Composer::new(Asking::Task);
        line.insert("port ");
        line.paste("the importer\nand its tests");
        assert_eq!(line.text, "port the importer\nand its tests");
        assert!(line.pastes.is_empty());
        assert_eq!(line.whole(), line.text);

        // Four rows is past the limit, so it folds behind a marker.
        let mut line = Composer::new(Asking::Task);
        line.insert("what went wrong here: ");
        let log = "one\ntwo\nthree\nfour";
        line.paste(log);
        assert_eq!(line.text, "what went wrong here: [Pasted text #1]");
        assert_eq!(line.at, line.text.chars().count());
        assert_eq!(line.whole(), format!("what went wrong here: {log}"));

        // So does one long paragraph. The second paste is marker #2.
        line.insert(" and ");
        let dump = "x".repeat(801);
        line.paste(&dump);
        assert_eq!(
            line.text,
            "what went wrong here: [Pasted text #1] and [Pasted text #2]"
        );
        assert_eq!(
            line.whole(),
            format!("what went wrong here: {log} and {dump}")
        );

        // The `!` is read from the whole line, so a pasted script is a command
        // row.
        let mut line = Composer::new(Asking::Task);
        line.paste("!set -e\ncargo build\ncargo test\ncargo clippy");
        assert_eq!(line.text, "[Pasted text #1]");
        assert_eq!(line.label(), "COMMAND");
    }

    #[test]
    fn composer_unfolds_a_marker_when_the_same_paste_lands_on_the_line_again() {
        let log = "one\ntwo\nthree\nfour";
        let folded = || {
            let mut line = Composer::new(Asking::Task);
            line.insert("why: ");
            line.paste(log);
            line.insert(" then");
            assert_eq!(line.text, "why: [Pasted text #1] then");
            line
        };

        // Pasting the same text again unfolds its marker in place. What is
        // sent does not change.
        let mut line = folded();
        line.home();
        line.paste(log);
        assert_eq!(line.text, format!("why: {log} then"));
        assert_eq!(line.at, "why: ".chars().count() + log.chars().count());
        assert_eq!(line.whole(), format!("why: {log} then"));

        // Once the marker is deleted the same text folds again, as #2, since
        // #1 still names the first paste.
        let mut line = folded();
        line.at = "why: [Pasted text #1]".chars().count();
        line.delete_back();
        line.paste(log);
        assert_eq!(line.text, "why: [Pasted text #2] then");
        assert_eq!(line.whole(), format!("why: {log} then"));

        // A different paste folds as usual.
        let mut line = folded();
        line.end();
        let dump = "x".repeat(801);
        line.paste(&dump);
        assert_eq!(line.text, "why: [Pasted text #1] then[Pasted text #2]");
        assert_eq!(line.whole(), format!("why: {log} then{dump}"));
    }

    #[test]
    fn composer_takes_a_marker_whole_from_either_side_of_it() {
        let folded = || {
            let mut line = Composer::new(Asking::Task);
            line.insert("why: ");
            line.paste("one\ntwo\nthree\nfour");
            line.insert(" then");
            assert_eq!(line.text, "why: [Pasted text #1] then");
            line
        };

        // Backspace at the end of a marker takes the whole marker.
        let mut line = folded();
        line.at = "why: [Pasted text #1]".chars().count();
        line.delete_back();
        assert_eq!(line.text, "why:  then");
        assert_eq!(line.whole(), "why:  then");

        // Delete at its start does the same.
        let mut line = folded();
        line.at = "why: ".chars().count();
        line.delete_forward();
        assert_eq!(line.text, "why:  then");

        // So does ctrl+w, which would otherwise take only `#1]`.
        let mut line = folded();
        line.at = "why: [Pasted text #1]".chars().count();
        line.delete_word_back();
        assert_eq!(line.text, "why:  then");

        // The same characters typed by hand are plain text.
        let mut line = Composer::new(Asking::Task);
        line.insert("[Pasted text #1]");
        line.delete_back();
        assert_eq!(line.text, "[Pasted text #1");
    }

    #[test]
    fn composer_walks_the_cursor_by_a_character_a_word_and_to_the_ends() {
        let mut line = Composer::new(Asking::Task);
        line.insert("port the importer");

        line.home();
        line.left();
        assert_eq!(line.at, 0, "neither end walks off the line");
        line.end();
        line.right();
        assert_eq!(line.at, 17);

        line.word_left();
        assert_eq!(line.at, 9, "the front of the word it was at the end of");
        line.word_left();
        assert_eq!(line.at, 5);
        line.word_left();
        line.word_left();
        assert_eq!(line.at, 0, "and the front of the line is where they stop");

        line.word_right();
        assert_eq!(line.at, 4, "the end of the word in front of it");
        line.word_right();
        line.word_right();
        line.word_right();
        assert_eq!(line.at, 17, "and the end of the line is where those stop");
    }

    #[test]
    fn composer_takes_back_the_character_and_the_word_the_cursor_stands_after() {
        let mut line = Composer::new(Asking::Task);
        line.insert("port the importer");

        line.word_left();
        line.delete_back();
        assert_eq!((line.text.as_str(), line.at), ("port theimporter", 8));
        line.delete_forward();
        assert_eq!(
            (line.text.as_str(), line.at),
            ("port themporter", 8),
            "the one under it goes and the cursor stays where it was"
        );

        // Neither key deletes past its end of the line.
        line.home();
        line.delete_back();
        assert_eq!((line.text.as_str(), line.at), ("port themporter", 0));
        line.end();
        line.delete_forward();
        assert_eq!((line.text.as_str(), line.at), ("port themporter", 15));

        // ctrl+w deletes back to where word_left would go.
        let mut line = Composer::new(Asking::Task);
        line.insert("port the importer");
        line.word_left();
        line.delete_word_back();
        assert_eq!((line.text.as_str(), line.at), ("port importer", 5));
        line.delete_word_back();
        assert_eq!((line.text.as_str(), line.at), ("importer", 0));
        line.delete_word_back();
        assert_eq!(
            (line.text.as_str(), line.at),
            ("importer", 0),
            "and the front of the line is where it stops too"
        );

        // Counted in characters, not bytes.
        let mut line = Composer::new(Asking::Task);
        line.insert("a é c");
        line.left();
        line.left();
        line.delete_back();
        assert_eq!((line.text.as_str(), line.at), ("a  c", 2));
    }

    #[test]
    fn a_line_names_itself_in_one_word_on_the_rule_over_it() {
        assert_eq!(Composer::new(Asking::Task).label(), "TASK");
        assert_eq!(
            Composer::new(Asking::Name {
                id: "fix-login-b2c".to_string(),
            })
            .label(),
            "RENAME"
        );
        assert_eq!(
            Composer::new(Asking::Fork {
                id: "fix-login-b2c".to_string(),
            })
            .label(),
            "FORK"
        );
    }

    #[test]
    fn fork_line_names_the_agent_it_copies_and_is_walked_back_among_the_tasks() {
        let copying = || Asking::Fork {
            id: "fix-login-b2c".to_string(),
        };
        assert_eq!(
            Composer::new(copying()).about().as_deref(),
            Some("fix-login-b2c"),
            "the rule names the agent the copy is of, by the id every verb \
             takes it by"
        );

        // A fork line is kept and recalled with the tasks.
        let mut sent = Backlog::default();
        sent.remember_line(&copying(), "now do it with sqlite");
        assert_eq!(sent.lines_for(&Asking::Task), ["now do it with sqlite"]);
        assert_eq!(sent.lines_for(&copying()), ["now do it with sqlite"]);
    }

    #[test]
    fn fork_from_the_view_answers_in_the_line_the_verb_would_have_written() {
        // A project cap of zero refuses the fork before anything is made, which
        // shows the verb's stderr comes back as one plain line.
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();
        let meta = Meta {
            role: None,
            parent: None,
            depth: 0,
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            dir: here.path().to_path_buf(),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx-not-a-server".to_string()),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: Some("6f1c9f4e-0d5b-4a51-9f6e-2b1f0c3d4e5a".to_string()),
            transcript: None,
            created: store::now(),
        };
        let origin = Agent::create(root.path(), &meta).unwrap();
        spawn::write_handoff(
            origin.dir(),
            &Handoff {
                task: meta.task.clone(),
                command: vec!["claude".to_string(), meta.task.clone()],
            },
        )
        .unwrap();
        std::fs::create_dir(here.path().join(".amx")).unwrap();
        std::fs::write(here.path().join(".amx/config.toml"), "max_agents = 0\n").unwrap();
        crate::consent::allow_in(
            root.path(),
            &crate::paths::project_config(here.path()).unwrap(),
        )
        .unwrap();

        let Started::No(why) =
            spawn_copy(root.path(), "fix-login-a1b", Some("do it again")).unwrap()
        else {
            panic!("a copy was started over the project's own cap");
        };
        assert!(why.starts_with("amx fork: "), "{why:?}");
        assert!(why.contains("max_agents is 0"), "{why:?}");
        assert!(!why.contains('\u{1b}'), "{why:?}");
        assert_eq!(
            crate::store::list(root.path()).unwrap(),
            ["fix-login-a1b"],
            "and nothing was made on the way to finding out"
        );
    }

    /// Creates an agent record in `dir`, with an optional worktree.
    fn record(root: &Path, id: &str, dir: &Path, worktree: Option<&Path>) -> Agent {
        Agent::create(
            root,
            &Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: Some("claude".to_string()),
                model: None,
                effort: None,
                dir: dir.to_path_buf(),
                worktree: worktree.map(Path::to_path_buf),
                branch: None,
                base: None,
                socket: Socket::Name("amx-not-a-server".to_string()),
                pane: PaneId::new("%404").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: store::now(),
            },
        )
        .expect("the record")
    }

    #[test]
    fn a_bound_key_runs_where_the_agent_works_with_the_agent_around_it() {
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();
        let tree = TempDir::new().unwrap();
        let said = here.path().join("said");
        let agent = record(root.path(), "fix-login-a1b", here.path(), Some(tree.path()));

        // Writes its directory and the agent's environment variables.
        let command = format!(
            "{{ pwd; echo \"$AMX_ID|$AMX_DIR|$AMX_AGENT_DIR|$AMX_WORKTREE|$AMX_NESTED\"; }} > {}",
            said.display()
        );
        run_bound(root.path(), "fix-login-a1b", &command).expect("the command");

        let written = std::fs::read_to_string(&said).expect("what the command wrote");
        let (ran_in, pairs) = written.split_once('\n').expect("a line and the pairs");
        assert_eq!(
            std::fs::canonicalize(ran_in).unwrap(),
            std::fs::canonicalize(tree.path()).unwrap(),
            "the tree amx cut for the agent is where its key runs: {ran_in}"
        );
        assert_eq!(
            pairs.trim_end(),
            format!(
                "fix-login-a1b|{}|{}|{}|1",
                agent.dir().display(),
                crate::spawn::scratch(agent.dir()).unwrap().display(),
                tree.path().display()
            )
        );

        // Without a worktree it runs in the agent's directory.
        let work = TempDir::new().unwrap();
        record(root.path(), "no-tree-b2c", work.path(), None);
        let said = here.path().join("elsewhere");
        run_bound(
            root.path(),
            "no-tree-b2c",
            &format!("pwd > {}", said.display()),
        )
        .expect("the command");
        assert_eq!(
            std::fs::canonicalize(std::fs::read_to_string(&said).unwrap().trim()).unwrap(),
            std::fs::canonicalize(work.path()).unwrap()
        );
    }

    #[test]
    fn a_bound_command_that_ended_badly_is_named_with_the_code_it_gave() {
        // The command has already shown its own error on the terminal, so the
        // view only names it and its exit code.
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();
        record(root.path(), "fix-login-a1b", here.path(), None);

        let said = format!(
            "{:#}",
            run_bound(root.path(), "fix-login-a1b", "exit 3").unwrap_err()
        );
        assert_eq!(said, "exit 3 exited 3");

        // A missing record is an error.
        let said = format!(
            "{:#}",
            run_bound(root.path(), "never-made-abc", "true").unwrap_err()
        );
        assert!(said.contains("no agent"), "{said}");
    }

    #[test]
    fn a_bound_key_on_an_agent_whose_tree_is_gone_says_so() {
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();
        let tree = here.path().join("removed-tree");
        record(root.path(), "fix-login-a1b", here.path(), Some(&tree));

        let said = format!(
            "{:#}",
            run_bound(root.path(), "fix-login-a1b", "true").unwrap_err()
        );
        assert_eq!(said, format!("{} is gone", tree.display()));
    }

    /// An `AskUserQuestion` question with two choices. `multi` allows several
    /// choices; `previewed` gives the first a preview, which adds a notes
    /// field.
    fn asked(multi: bool, previewed: bool) -> Ask {
        let choice = |label: &str, preview: bool| Choice {
            label: label.to_string(),
            description: None,
            preview: preview.then(|| "+----------+".to_string()),
        };
        Ask {
            header: Some("Fixtures".to_string()),
            text: "Which fixture should the port keep?".to_string(),
            options: vec![
                choice("the sqlite one", previewed),
                choice("the docker one", false),
            ],
            multi,
            answer: None,
        }
    }

    #[test]
    fn card_invites_the_answers_the_prompt_in_front_of_somebody_will_take() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        let one = ["Yes".to_string()];

        // A single-choice question: digits pick on the press, or type words.
        assert_eq!(
            invitation(Some(Kind::Question), &two, None, false),
            "1-2 picks, or type an answer"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &one, None, false),
            "1 picks, or type an answer",
            "and one choice is one number"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &[], None, false),
            "type an answer",
            "and a menu whose choices amx has not read yet names none"
        );

        // A multi-choice question checks boxes by number.
        assert_eq!(
            invitation(Some(Kind::Question), &two, Some(&asked(true, false)), false),
            "press 1-2, 1,3 for several, or type an answer"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &[], Some(&asked(true, false)), false),
            "type an answer",
            "with no choices read there is nothing to check"
        );

        // With previews there is a notes field and no free-text row, so words
        // after the key are a note.
        assert_eq!(
            invitation(Some(Kind::Question), &two, Some(&asked(false, true)), false),
            "press 1-2, and words after it are a note"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &two, Some(&asked(true, true)), false),
            "press 1-2, 1,3 for several, and words after it are a note",
            "and a checkbox question can carry one too"
        );

        // A permission box and a trust screen take one key, never words.
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert_eq!(
                invitation(kind, &two, None, false),
                "press 1-2, y or n",
                "{kind:?}"
            );
            assert_eq!(
                invitation(kind, &one, None, false),
                "press 1, y or n",
                "{kind:?}"
            );
        }
        for kind in [Some(Kind::Permission), None] {
            assert_eq!(
                invitation(kind, &[], None, false),
                "press y, n or 1-9",
                "with nothing read off the screen, the grammar itself: {kind:?}"
            );
        }

        // claude 2.1.259's trust gate draws no numbers, and the verb takes only
        // a walk or `esc` there.
        assert_eq!(
            invitation(Some(Kind::Trust), &[], None, false),
            "type down enter, up enter or esc"
        );

        // A walked list: amx numbers the rows itself and the verb walks to the
        // chosen one, so numbers are all the card offers, whatever the kind.
        let five = [
            "Trust".to_string(),
            "Trust parent".to_string(),
            "Trust and remember".to_string(),
            "Do not trust".to_string(),
            "Do not trust (this session only)".to_string(),
        ];
        assert_eq!(
            invitation(Some(Kind::Trust), &five, None, true),
            "press 1-5"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &two, None, true),
            "press 1-2",
            "and a walked dialog names its two the same way"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &one, None, true),
            "press 1",
            "and one choice is one number"
        );

        // A pending question under a record that says otherwise: an older amx
        // wrote `permission` over every menu. The screen is the call's menu,
        // so no y or n is offered.
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert_eq!(
                invitation(kind, &two, Some(&asked(false, false)), false),
                "press 1-2",
                "{kind:?}"
            );
            assert_eq!(
                invitation(kind, &two, Some(&asked(true, false)), false),
                "press 1-2, 1,3 for several",
                "{kind:?}"
            );
        }
    }

    #[test]
    fn card_reads_a_digit_as_the_answer_only_where_one_choice_is_the_answer() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        let question = |asked: Option<&Ask>| picks(Some(Kind::Question), &two, asked, false);

        // A single-choice question, with or without the payload read.
        assert!(question(None));
        assert!(question(Some(&asked(false, false))));

        // Multi-choice checks boxes, and a previewed question takes a note that
        // starts with the key, so neither sends on the press.
        assert!(!question(Some(&asked(true, false))));
        assert!(!question(Some(&asked(false, true))));

        // No choices read, or a permission or trust screen read off the pane.
        assert!(!picks(Some(Kind::Question), &[], None, false));
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert!(!picks(kind, &two, None, false), "{kind:?}");
            assert!(
                !picks(kind, &two, Some(&asked(false, false)), false),
                "{kind:?}"
            );
        }

        // A walked list can be a tool gate, so the digit fills the line and
        // enter sends the walk.
        assert!(!picks(Some(Kind::Question), &two, None, true));
    }

    #[test]
    fn card_hands_the_verb_the_whole_line_wherever_words_are_an_answer() {
        let line = |text: &str, asked: Option<&Ask>| {
            let args = card_line(text, asked);
            (args.key, args.note)
        };
        let plain = asked(false, false);

        // Keys, box lists and words all go whole as the key; the verb tells
        // them apart.
        for typed in ["2", "1,3", "neither, keep both"] {
            assert_eq!(
                line(typed, Some(&plain)),
                (Some(typed.to_string()), None),
                "{typed:?}"
            );
        }
        assert_eq!(
            line("  2  ", None),
            (Some("2".to_string()), None),
            "trimmed, so a stray space is not an answer of its own"
        );

        // A previewed question has a notes field and no free-text row, so
        // words after the key are the note.
        let previewed = asked(false, true);
        assert_eq!(
            line("1 prefer the stacked one", Some(&previewed)),
            (
                Some("1".to_string()),
                Some("prefer the stacked one".to_string())
            )
        );
        assert_eq!(
            line("1", Some(&previewed)),
            (Some("1".to_string()), None),
            "and a choice with nothing after it rides alone"
        );
        assert_eq!(
            line("prefer the stacked one", Some(&previewed)),
            (Some("prefer the stacked one".to_string()), None),
            "a line that does not open with a key is quoted back whole rather \
             than by its first word"
        );
    }

    #[test]
    fn card_sends_the_hunk_under_the_cursor_in_front_of_the_words() {
        let hunk = |text: &str| Hunk {
            path: "src/foo.rs".to_string(),
            line: 12,
            row: 4,
            text: text.to_string(),
        };

        // `path:line`, then the hunk in a fence, then the words.
        assert_eq!(
            on_hunk(
                &hunk("@@ -12,2 +12,3 @@\n context\n+added"),
                "why this row?"
            ),
            "src/foo.rs:12\n\n```diff\n@@ -12,2 +12,3 @@\n context\n+added\n```\n\nwhy this row?"
        );

        // A hunk containing a triple backtick gets a four-backtick fence.
        let fenced = on_hunk(&hunk("@@ -1,1 +1,2 @@\n+```sh"), "and this?");
        assert_eq!(
            fenced,
            "src/foo.rs:12\n\n````diff\n@@ -1,1 +1,2 @@\n+```sh\n````\n\nand this?"
        );
    }

    #[test]
    fn card_writes_a_whole_review_as_one_message() {
        let hunk = |line: usize, text: &str| Hunk {
            path: "src/foo.rs".to_string(),
            line,
            row: 4,
            text: text.to_string(),
        };
        let first = hunk(12, "@@ -12,2 +12,3 @@\n context\n+added");
        let second = hunk(40, "@@ -40,1 +40,1 @@\n-gone\n+here");

        // One hunk with no opening is exactly the single-hunk comment.
        assert_eq!(
            on_hunks("", &[(&first, "why this row?")]),
            on_hunk(&first, "why this row?")
        );

        // The opening, then each hunk in order, separated by blank lines.
        assert_eq!(
            on_hunks(
                "two things",
                &[(&first, "why this row?"), (&second, "and this?")]
            ),
            format!(
                "two things\n\n{}\n\n{}",
                on_hunk(&first, "why this row?"),
                on_hunk(&second, "and this?")
            )
        );

        // A whitespace-only opening is dropped.
        assert_eq!(
            on_hunks(
                "  \n ",
                &[(&first, "why this row?"), (&second, "and this?")]
            ),
            format!(
                "{}\n\n{}",
                on_hunk(&first, "why this row?"),
                on_hunk(&second, "and this?")
            )
        );

        // An opening with no hunks is the whole message.
        assert_eq!(on_hunks("just this", &[]), "just this");
    }

    #[test]
    fn axis_reads_a_line_of_nothing_but_tokens_as_a_narrowing() {
        assert_eq!(
            narrowing("s:working"),
            Some(vec![Narrow::State(Some("working".to_string()))])
        );
        assert_eq!(
            narrowing("s:waiting  s:working"),
            Some(vec![
                Narrow::State(Some("waiting".to_string())),
                Narrow::State(Some("working".to_string())),
            ]),
            "as many as were typed, in the order they were typed"
        );
        assert_eq!(
            narrowing("a:port"),
            None,
            "`a:` narrowed by name once and does not now: `/` does that \
             untokenised, so a line beginning `a:` is a name with a colon in it"
        );
        assert_eq!(
            narrowing("s:"),
            Some(vec![Narrow::State(None)]),
            "and a token with nothing after it drops that one"
        );
    }

    #[test]
    fn axis_takes_a_line_with_a_colon_in_it_for_the_name_it_is() {
        for line in [
            "s:waiting is what to check",
            "port the importer",
            "fix s:waiting",
            "",
            "   ",
        ] {
            assert_eq!(narrowing(line), None, "{line:?} is a name, colons and all");
        }
    }

    #[test]
    fn axis_leaves_the_state_tokens_on_the_task_line_alone() {
        // Only the find line narrows. On a task line `s:waiting` is task text.
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        assert_eq!(composer.label(), "TASK");

        let (dials, task) = turned(&as_claude(), "s:waiting").unwrap();
        assert_eq!(dials, Turned::default());
        assert_eq!(task, "s:waiting", "and the whole of it is what is started");
    }

    /// A config using claude, which declares dials in the registry.
    fn as_claude() -> Config {
        Config {
            agent: "claude".to_string(),
            ..Config::default()
        }
    }

    #[test]
    fn composer_says_which_lines_are_too_slight_to_be_a_task() {
        let slight = |line: &str| slight(&as_claude(), line);

        assert_eq!(slight("fix"), Some("fix".to_string()));
        assert_eq!(
            slight("m:opus fix"),
            Some("fix".to_string()),
            "the task rather than the line: the dials are not the instruction"
        );
        assert_eq!(
            slight("  n\n "),
            Some("n".to_string()),
            "and a stray keystroke is the one this is about"
        );

        for line in ["port it", "fix the login bug", "", "   "] {
            assert_eq!(slight(line), None, "{line:?} stands on its own");
        }
        assert_eq!(
            slight("s:"),
            Some("s:".to_string()),
            "and the tokens that narrowed the wall from here once are two \
             characters of task like any other"
        );
    }

    #[test]
    fn composer_hands_the_line_to_an_editor_and_takes_back_what_was_written() {
        let here = TempDir::new().unwrap();
        let path = here.path().join("task.md");

        // The editor gets the line and what it leaves becomes the line.
        let Edited::Line(text) =
            edited_in("sed -i -e s/fix/port/", &path, "fix the importer").unwrap()
        else {
            panic!("the editor was not read back");
        };
        assert_eq!(text, "port the importer");
        assert!(
            !path.exists(),
            "and the file it was written in does not outlive the edit"
        );

        let Edited::Line(text) =
            edited_in("printf 'port it\\n' >", &path, "port the importer").unwrap()
        else {
            panic!("the editor was not read back");
        };
        assert_eq!(
            text, "port it",
            "the newline a file ends with is the file's rather than the task's"
        );
    }

    #[test]
    fn composer_keeps_the_line_an_editor_would_not_write() {
        let here = TempDir::new().unwrap();
        let path = here.path().join("task.md");

        // A non-zero exit (vi's `:cq`) keeps the line.
        let Edited::No(why) = edited_in("false", &path, "port the importer").unwrap() else {
            panic!("an editor that refused took the line with it");
        };
        assert!(why.contains("false"), "{why}");
        assert!(!path.exists());
    }

    #[test]
    fn composer_a_leading_bang_makes_the_line_a_command_row() {
        // A leading `!` makes the rest a command, as `amx new --exec` does.
        let mut composer = Composer::new(Asking::Task);
        composer.text = "!cargo test".to_string();
        assert_eq!(
            composer.label(),
            "COMMAND",
            "and the rule says so while the bang stands"
        );
        composer.text = "cargo test".to_string();
        assert_eq!(composer.label(), "TASK");

        let (dials, command) = turned(&as_claude(), "!cargo test").unwrap();
        assert_eq!(
            dials,
            Turned {
                exec: true,
                ..Turned::default()
            },
            "no vendor and no dials: a command row runs a shell"
        );
        assert_eq!(command, "cargo test");

        // `d:` is the only dial it takes.
        let (dials, command) = turned(&as_claude(), "!d:/srv/app  cargo test").unwrap();
        assert_eq!(
            dials,
            Turned {
                exec: true,
                dir: Some("/srv/app".to_string()),
                ..Turned::default()
            }
        );
        assert_eq!(command, "cargo test");
    }

    #[test]
    fn composer_refuses_the_dials_a_command_row_has_nothing_to_turn() {
        // Vendor dials and `agent:` have no vendor to apply to (`--exec`
        // refuses them too), and a command runs where it was typed, so the
        // worktree dials go as well.
        let refused = |line: &str| turned(&as_claude(), line).expect_err(line);

        for line in [
            "!m:opus cargo test",
            "!p:plan ls",
            "!agent:codex ls",
            "!w:on ls",
            "!b:main ls",
            "!pr:412 ls",
        ] {
            let said = refused(line);
            assert!(
                said.ends_with("a command row takes d: and no other dial"),
                "{line:?}: {said}"
            );
            assert!(
                line.contains(said.split(':').next().expect("the token it names")),
                "and names the token as it was typed: {said}"
            );
        }
        assert_eq!(refused("!d: cargo test"), "d: takes a directory");

        // Only leading tokens are dials.
        let (dials, command) = turned(&as_claude(), "!echo m:opus").unwrap();
        assert!(dials.exec);
        assert_eq!(command, "echo m:opus");
    }

    #[test]
    fn composer_never_asks_about_a_command_row_however_short_it_is() {
        // The prompt guards against a stray keystroke after `n`, and `!` is not
        // one.
        assert_eq!(slight(&as_claude(), "!ls"), None);
        assert_eq!(slight(&as_claude(), "!d:/srv/app ls"), None);
    }

    #[test]
    fn composer_offers_the_cards_line_the_vendors_words_and_none_of_the_dials() {
        // A reply goes to a running agent, so dial words are message text and
        // are offered nothing.
        for word in ["agent:cl", "m:", "p:", "w:", "d:/"] {
            let mut line = Composer::new(Asking::Reply);
            line.insert(word);
            assert!(
                suggest(&line, &as_claude(), a_project(), &[]).is_none(),
                "{word:?} is a word of the message"
            );
        }
        let mut task = Composer::new(Asking::Task);
        task.insert("agent:cl");
        assert!(
            suggest(&task, &as_claude(), a_project(), &[]).is_some(),
            "and a dial on the task line it still is"
        );

        // A reply has no `d:`, so paths are read in the directory it was
        // handed.
        let project = TempDir::new().unwrap();
        std::fs::write(project.path().join("importer.rs"), "").unwrap();
        let mut line = Composer::new(Asking::Reply);
        line.insert("see d:/nowhere @imp");
        let found = suggest(&line, &as_claude(), project.path(), &[]).expect("the file");
        assert_eq!(
            found
                .entries
                .iter()
                .map(|entry| entry.spelled.as_str())
                .collect::<Vec<_>>(),
            ["@importer.rs"]
        );
    }

    #[test]
    fn composer_offers_a_command_row_none_of_the_words_a_vendor_answers_to() {
        // In a shell, `/etc` is a path and `@src` is a word for `cat`.
        for line in ["!ls /", "!cat @src"] {
            let mut composer = Composer::new(Asking::Task);
            composer.insert(line);
            assert!(
                suggest(&composer, &as_claude(), a_project(), &[]).is_none(),
                "{line:?}"
            );
        }
    }

    #[test]
    fn composer_reads_the_dials_off_the_front_of_a_task_line() {
        let (dials, task) =
            turned(&as_claude(), "m:opus p:plan e:high w:off port the importer").unwrap();
        assert_eq!(
            dials,
            Turned {
                exec: false,
                agent: None,
                model: Some("opus".to_string()),
                permission: Some("plan".to_string()),
                effort: Some("high".to_string()),
                worktree: Some(false),
                dir: None,
                base: None,
                branch: None,
                pr: None,
                with_changes: false,
            }
        );
        assert_eq!(task, "port the importer");

        let (dials, task) = turned(&as_claude(), "agent:codex  w:on  fix the login bug").unwrap();
        assert_eq!(dials.agent.as_deref(), Some("codex"));
        assert_eq!(dials.worktree, Some(true));
        assert_eq!(task, "fix the login bug", "however they were spaced");
    }

    #[test]
    fn composer_keeps_the_newlines_of_the_task_the_dials_lead() {
        let (dials, task) =
            turned(&as_claude(), "m:opus port the importer\nand its tests").unwrap();
        assert_eq!(dials.model.as_deref(), Some("opus"));
        assert_eq!(
            task, "port the importer\nand its tests",
            "a pasted task is one task, dials or no dials"
        );
    }

    #[test]
    fn composer_takes_a_line_with_a_dial_word_anywhere_else_for_the_task_it_is() {
        // Only leading tokens are dials.
        for line in [
            "port the m:opus importer",
            "fix w:off",
            "port the importer",
            "make agent:s of them",
        ] {
            let (dials, task) = turned(&as_claude(), line).unwrap();
            assert_eq!(dials, Turned::default(), "{line:?}");
            assert_eq!(task, line, "{line:?} is a task, colons and all");
        }
    }

    #[test]
    fn composer_aims_one_spawn_at_the_directory_its_line_names() {
        let (dials, task) = turned(&as_claude(), "d:/srv/app port the importer").unwrap();
        assert_eq!(dials.dir.as_deref(), Some("/srv/app"));
        assert_eq!(task, "port the importer");

        let (dials, task) = turned(&as_claude(), "d:~/code/amx  m:opus  fix it").unwrap();
        assert_eq!(
            dials.dir.as_deref(),
            Some("~/code/amx"),
            "as it was typed: what a path means is worked out where it is used"
        );
        assert_eq!(dials.model.as_deref(), Some("opus"));
        assert_eq!(task, "fix it");

        assert_eq!(
            turned(&as_claude(), "d: port it").expect_err("refused"),
            "d: takes a directory"
        );
    }

    #[test]
    fn composer_cuts_one_spawn_from_the_ref_or_the_request_its_line_names() {
        let (dials, task) = turned(&as_claude(), "b:main port the importer").unwrap();
        assert_eq!(dials.base.as_deref(), Some("main"));
        assert_eq!(task, "port the importer");

        // Resolving the ref is the spawn's job; any word is accepted here.
        let (dials, task) = turned(&as_claude(), "b:v0.2.0  m:opus  port it").unwrap();
        assert_eq!(dials.base.as_deref(), Some("v0.2.0"));
        assert_eq!(dials.model.as_deref(), Some("opus"));
        assert_eq!(task, "port it");

        let (dials, task) = turned(&as_claude(), "pr:412 review it").unwrap();
        assert_eq!(dials.pr, Some(412));
        assert_eq!(task, "review it");

        assert_eq!(
            turned(&as_claude(), "b: port it").expect_err("refused"),
            "b: takes a ref"
        );
        for line in ["pr: review it", "pr:none review it", "pr:-1 review it"] {
            assert_eq!(
                turned(&as_claude(), line).expect_err("refused"),
                "pr: takes a number",
                "{line:?}"
            );
        }
    }

    #[test]
    fn composer_refuses_a_request_beside_the_words_that_answer_it() {
        // The pairs clap refuses, in either order.
        let refused = |line: &str| turned(&as_claude(), line).expect_err(line);

        for line in ["pr:412 b:main review it", "b:main pr:412 review it"] {
            assert_eq!(
                refused(line),
                "pr: and b: — a request says what it is cut from",
                "{line:?}"
            );
        }
        for line in [
            "pr:412 w:off review it",
            "w:changes pr:412 review it",
            "pr:412 w:changes review it",
        ] {
            assert_eq!(
                refused(line),
                "pr: and w: — a request is a tree of its own",
                "{line:?}"
            );
        }

        // `w:on` is redundant with a request and allowed, as is `b:` with
        // `w:changes`.
        let (dials, _) = turned(&as_claude(), "pr:412 w:on review it").unwrap();
        assert_eq!((dials.pr, dials.worktree), (Some(412), Some(true)));

        let (dials, _) = turned(&as_claude(), "b:main w:changes port it").unwrap();
        assert_eq!(dials.base.as_deref(), Some("main"));
        assert!(dials.with_changes);
    }

    #[test]
    fn composer_is_turned_onto_the_branch_its_line_names() {
        let (dials, task) = turned(&as_claude(), "on:spike carry on with it").unwrap();
        assert_eq!(dials.branch.as_deref(), Some("spike"));
        assert_eq!(task, "carry on with it");

        // Finding the branch is the spawn's job; any word is accepted here.
        let (dials, task) = turned(&as_claude(), "on:origin/spike  m:opus  carry on").unwrap();
        assert_eq!(dials.branch.as_deref(), Some("origin/spike"));
        assert_eq!(dials.model.as_deref(), Some("opus"));
        assert_eq!(task, "carry on");

        // `w:changes` is allowed with it.
        let (dials, _) = turned(&as_claude(), "on:spike w:changes carry on").unwrap();
        assert_eq!(dials.branch.as_deref(), Some("spike"));
        assert!(dials.with_changes);
    }

    #[test]
    fn composer_refuses_a_branch_beside_the_words_turned_against_it() {
        let refused = |line: &str| turned(&as_claude(), line).expect_err(line);

        assert_eq!(refused("on: carry on with it"), "on: takes a branch");

        // The pairs clap refuses, in either order.
        for line in ["on:spike b:main port it", "b:main on:spike port it"] {
            assert_eq!(
                refused(line),
                "on: and b: — a branch says what it is cut from",
                "{line:?}"
            );
        }
        for line in ["on:spike pr:412 review it", "pr:412 on:spike review it"] {
            assert_eq!(
                refused(line),
                "on: and pr: — a request is a branch of its own",
                "{line:?}"
            );
        }
        for line in ["on:spike w:off port it", "w:off on:spike port it"] {
            assert_eq!(
                refused(line),
                "on: and w:off — a branch is a tree of its own",
                "{line:?}"
            );
        }

        // A command row takes only `d:`.
        assert_eq!(
            refused("!on:spike ls"),
            "on:spike: a command row takes d: and no other dial"
        );
    }

    #[test]
    fn composer_carries_the_work_no_commit_holds_where_the_line_says_changes() {
        let (dials, task) = turned(&as_claude(), "w:changes fix the login bug").unwrap();
        assert_eq!(
            (dials.worktree, dials.with_changes),
            (Some(true), true),
            "the work moves into a tree, so the word asks for one"
        );
        assert_eq!(task, "fix the login bug");

        let (dials, _) = turned(&as_claude(), "w:on port it").unwrap();
        assert!(
            !dials.with_changes,
            "and a tree of its own is not the half hour you had already spent"
        );
    }

    #[test]
    fn composer_refuses_changes_where_there_is_nothing_to_move_before_anything_is_made() {
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();

        // The refusal names `w:changes`, not `new`'s flag.
        let line = format!("d:{} w:changes port the importer", here.path().display());
        let Started::No(why) =
            start(root.path(), &Config::default(), &line, None, here.path()).unwrap()
        else {
            panic!("a spawn was sent to carry work that is not there");
        };
        assert_eq!(
            why,
            format!("w:changes: nothing in {} to move", here.path().display())
        );
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and nothing was made on the way to finding out"
        );
    }

    #[test]
    fn composer_refuses_a_directory_nothing_is_at_before_anything_is_made() {
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();

        let Started::No(why) = start(
            root.path(),
            &Config::default(),
            "d:nowhere/at/all port the importer",
            None,
            here.path(),
        )
        .unwrap() else {
            panic!("a spawn was aimed at a directory that is not there");
        };
        assert!(why.contains("nowhere/at/all"), "{why}");
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and nothing was made on the way to finding out"
        );

        // A relative path is read against the view's directory.
        assert_eq!(
            aimed("app", here.path()).expect_err("no such directory"),
            format!("d:app: nothing is at {}/app", here.path().display())
        );
        std::fs::create_dir(here.path().join("app")).unwrap();
        assert_eq!(
            aimed("app", here.path()).unwrap(),
            here.path().join("app"),
            "and a directory that is there is where the agent runs"
        );
    }

    #[test]
    fn composer_refuses_a_value_the_vendor_would_not_take_and_says_what_it_does() {
        let refused = |line: &str| turned(&as_claude(), line).expect_err(line);

        let said = refused("p:nonsense port the importer");
        assert!(said.starts_with("p:nonsense: claude takes"), "{said}");
        assert!(said.contains("acceptEdits"), "every mode it has: {said}");
        assert_eq!(refused("w:maybe port it"), "w:maybe: on, off or changes");
        assert_eq!(refused("m: port it"), "m: takes a value");
        let said = refused("e:hard port it");
        assert!(said.starts_with("e:hard: claude takes"), "{said}");
        assert!(said.contains("xhigh"), "every level it has: {said}");
        assert_eq!(refused("agent: port it"), "agent: takes a command");

        // An open dial accepts values outside its cycle, as `--model` does.
        let (dials, _) = turned(&as_claude(), "m:claude-fable-5 port it").unwrap();
        assert_eq!(dials.model.as_deref(), Some("claude-fable-5"));
    }

    #[test]
    fn composer_refuses_a_dial_the_agent_on_the_same_line_does_not_declare() {
        // An unregistered agent still spawns, but dials it never declared are
        // refused by name.
        let said = turned(&as_claude(), "agent:mock-claude m:opus port it").expect_err("refused");
        assert_eq!(said, "m:opus: amx knows no such dial for mock-claude");

        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };
        assert_eq!(
            turned(&config, "p:plan port it").expect_err("refused"),
            "p:plan: amx knows no such dial for mock-claude"
        );
        let (dials, task) = turned(&config, "w:off port it").unwrap();
        assert_eq!(
            (dials.worktree, task.as_str()),
            (Some(false), "port it"),
            "the tree is amx's own dial, and every agent gets one"
        );
    }

    #[test]
    fn composer_hands_the_agent_a_line_is_led_with_to_the_vendor_and_not_the_task() {
        // A leading `@name` found in the project's agents becomes the vendor's
        // `--agent` flag.
        let project = TempDir::new().unwrap();
        let agents = project.path().join(".claude/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("scout.md"),
            "---\ndescription: Goes and looks.\n---\n",
        )
        .unwrap();

        assert_eq!(
            as_agent("claude", "@scout port the importer", project.path()),
            (
                vec!["--agent".to_string(), "scout".to_string()],
                "port the importer".to_string()
            )
        );

        // Anywhere else, or a name that is not one of its agents, stays in the
        // task.
        for line in [
            "port the importer @scout",
            "@notes.md port the importer",
            "@ port the importer",
            "port the importer",
        ] {
            assert_eq!(
                as_agent("claude", line, project.path()),
                (Vec::new(), line.to_string()),
                "{line:?}"
            );
        }

        // A vendor with no agent flag (pi, or one amx has no entry for) leaves
        // the word in the task.
        for agent in ["pi", "mock-claude"] {
            assert_eq!(
                as_agent(agent, "@scout port the importer", project.path()),
                (Vec::new(), "@scout port the importer".to_string()),
                "{agent}, which is the same answer as a command amx has no \
                 entry for at all"
            );
        }
    }

    /// The spellings a suggestion offers, in order.
    fn offered(suggest: &Suggest) -> Vec<&str> {
        suggest
            .entries
            .iter()
            .map(|entry| entry.spelled.as_str())
            .collect()
    }

    /// A project path for suggestions that never read the disk.
    fn a_project() -> &'static Path {
        Path::new("/srv/app")
    }

    #[test]
    fn composer_offers_the_vendors_a_line_can_be_aimed_at_by_name() {
        // `agent:` completes to every vendor in the registry.
        let mut line = Composer::new(Asking::Task);
        line.insert("agent:");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("the table");
        assert_eq!(
            offered(&found),
            ["agent:claude", "agent:pi", "agent:codex", "agent:opencode"]
        );
        assert_eq!(
            (found.word, found.chosen),
            (0..6, 0),
            "the word it is about, and the choice standing on the first of them"
        );

        line.insert("p");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("the one left");
        assert_eq!(offered(&found), ["agent:pi"]);

        line.insert("q");
        assert!(
            suggest(&line, &as_claude(), a_project(), &[]).is_none(),
            "and a word nothing answers to is the word somebody typed"
        );
    }

    #[test]
    fn composer_puts_the_suggestion_the_choice_is_on_where_the_word_was() {
        let mut line = Composer::new(Asking::Task);
        line.insert("m:opus agent:");
        line.suggest = suggest(&line, &as_claude(), a_project(), &[]);

        // The highlight wraps at both ends.
        line.choose(1);
        assert_eq!(line.suggest.as_ref().expect("the list").chosen, 1);
        line.choose(1);
        line.choose(1);
        line.choose(1);
        assert_eq!(
            line.suggest.as_ref().expect("the list").chosen,
            0,
            "past the last of them is the first"
        );
        line.choose(-1);
        assert_eq!(line.suggest.as_ref().expect("the list").chosen, 3);
        line.choose(-1);
        line.choose(-1);
        assert_eq!(line.suggest.as_ref().expect("the list").chosen, 1);

        line.complete();
        assert_eq!(line.text, "m:opus agent:pi ");
        assert_eq!(
            line.at, 16,
            "with the cursor after the space it left for the next word"
        );
        assert!(
            line.suggest.is_none(),
            "and the suggestions go with the word they were about"
        );
    }

    #[test]
    fn composer_completes_the_word_the_cursor_is_in_rather_than_the_line() {
        let mut line = Composer::new(Asking::Task);
        line.insert("agent:cl port it");
        // Put the cursor at the end of the word being corrected.
        for _ in 0.." port it".chars().count() {
            line.left();
        }

        line.suggest = suggest(&line, &as_claude(), a_project(), &[]);
        line.complete();
        assert_eq!(line.text, "agent:claude port it");
        assert_eq!(
            line.at, 12,
            "and a word mended in the middle of a sentence does not push the \
             next one along"
        );
    }

    #[test]
    fn composer_is_finishing_a_word_until_it_is_spelled_the_way_the_choice_is() {
        let mut line = Composer::new(Asking::Task);
        line.insert("agent:cl");
        line.suggest = suggest(&line, &as_claude(), a_project(), &[]);
        assert!(
            line.finishing(),
            "a word short of the one offered is a word still being written"
        );

        line.insert("aude");
        line.suggest = suggest(&line, &as_claude(), a_project(), &[]);
        assert_eq!(
            offered(line.suggest.as_ref().expect("itself")),
            ["agent:claude"]
        );
        assert!(
            !line.finishing(),
            "the same word spelled out is finished, however many suggestions \
             stand under it"
        );

        // Matching one entry while the highlight is on another means the
        // other.
        let mut line = Composer::new(Asking::Task);
        line.insert("/review");
        line.suggest = Some(Suggest {
            word: 0..7,
            entries: vec![
                worded("/review".to_string()),
                worded("/reviewer".to_string()),
            ],
            chosen: 0,
        });
        assert!(!line.finishing());
        line.choose(1);
        assert!(
            line.finishing(),
            "the word the choice is on is not the one typed"
        );

        line.suggest = None;
        assert!(
            !line.finishing(),
            "and a word with nothing under it is finished"
        );
    }

    #[test]
    fn composer_reads_the_catalog_once_for_the_life_of_the_line() {
        // Counts reads instead of touching the disk.
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            vec![worded("/review".to_string())]
        };
        let line = Composer::new(Asking::Task);

        assert_eq!(
            offered_by(&line.catalog("claude", a_project(), read)),
            ["/review"]
        );
        assert_eq!(
            offered_by(&line.catalog("claude", a_project(), read)),
            ["/review"]
        );
        assert_eq!(
            reads.get(),
            1,
            "the second keystroke on the same line reads nothing"
        );

        line.catalog("claude", Path::new("/srv/other"), read);
        assert_eq!(reads.get(), 2, "another project is another catalog");
        line.catalog("pi", Path::new("/srv/other"), read);
        assert_eq!(reads.get(), 3, "and so is another vendor");
        line.catalog("pi", Path::new("/srv/other"), read);
        assert_eq!(reads.get(), 3);

        assert_eq!(
            offered_by(&Composer::new(Asking::Task).catalog("pi", a_project(), Vec::new)),
            Vec::<&str>::new(),
            "a new line has read nothing yet"
        );
    }

    #[test]
    fn composer_narrows_the_next_keystroke_out_of_the_catalog_the_last_one_read() {
        // The catalog is set by hand, with names no disk has, so these
        // results can only come from the cache.
        let mut line = Composer::new(Asking::Task);
        *line.listed.borrow_mut() = Some(Listed {
            agent: "claude".to_string(),
            project: a_project().to_path_buf(),
            entries: vec![
                worded("/quenched".to_string()),
                worded("/quiet".to_string()),
            ],
        });

        line.insert("/qu");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("the reading");
        assert_eq!(offered(&found), ["/quenched", "/quiet"]);
        line.insert("e");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("narrowed");
        assert_eq!(
            offered(&found),
            ["/quenched"],
            "narrowed by what was typed, out of the same reading"
        );

        // Naming another vendor replaces the cached catalog.
        line.home();
        line.insert("agent:pi ");
        suggest(&line, &as_claude(), a_project(), &[]);
        assert_eq!(
            line.listed
                .borrow()
                .as_ref()
                .map(|listed| listed.agent.as_str()),
            Some("pi"),
            "the reading is the vendor the line names"
        );
    }

    /// The spellings in a catalog, in order.
    fn offered_by(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.spelled.as_str()).collect()
    }

    #[test]
    fn composer_opens_a_codex_lines_suggestions_on_its_own_sigil() {
        // codex runs a skill as `$name`, so only `$` opens its catalog and `/`
        // lists nothing. The catalog is set by hand.
        let as_codex = Config {
            agent: "codex".to_string(),
            ..Config::default()
        };
        let mut line = Composer::new(Asking::Task);
        *line.listed.borrow_mut() = Some(Listed {
            agent: "codex".to_string(),
            project: a_project().to_path_buf(),
            entries: vec![worded("$deploy".to_string())],
        });
        line.insert("$de");
        let found = suggest(&line, &as_codex, a_project(), &[]).expect("the skill");
        assert_eq!(offered(&found), ["$deploy"]);

        let mut line = Composer::new(Asking::Task);
        *line.listed.borrow_mut() = Some(Listed {
            agent: "codex".to_string(),
            project: a_project().to_path_buf(),
            entries: vec![worded("/deploy".to_string())],
        });
        line.insert("/de");
        assert!(
            suggest(&line, &as_codex, a_project(), &[]).is_none(),
            "a slash on a codex line asks its catalog for nothing"
        );
    }

    #[test]
    fn composer_suggests_nothing_for_a_word_that_asks_the_vendor_for_nothing() {
        let mut line = Composer::new(Asking::Task);
        line.insert("port the importer");
        assert!(
            suggest(&line, &as_claude(), a_project(), &[]).is_none(),
            "a sentence asks the vendor for nothing by name"
        );

        let mut line = Composer::new(Asking::Task);
        line.insert("agent:pi ");
        assert!(
            suggest(&line, &as_claude(), a_project(), &[]).is_none(),
            "and a cursor standing on whitespace is standing in no word"
        );
    }

    #[test]
    fn composer_offers_the_values_the_dial_under_the_cursor_takes() {
        // Expected values come from the registry, so a vendor renaming one
        // does not need this test changed.
        let claude = registry::entry("claude").expect("claude is in the table");
        let cycle = |dial: &str, spec: registry::DialSpec| -> Vec<String> {
            spec.cycle
                .iter()
                .map(|value| format!("{dial}{value}"))
                .collect()
        };

        let mut line = Composer::new(Asking::Task);
        line.insert("m:");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("claude's models");
        assert_eq!(
            offered(&found),
            cycle("m:", claude.model.expect("claude has a model dial"))
        );

        line.insert("o");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("the one left");
        assert_eq!(offered(&found), ["m:opus"]);

        let mut line = Composer::new(Asking::Task);
        line.insert("p:pl");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("claude's modes");
        assert_eq!(offered(&found), ["p:plan"]);

        let mut line = Composer::new(Asking::Task);
        line.insert("e:");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("claude's levels");
        assert_eq!(
            offered(&found),
            cycle("e:", claude.effort.expect("claude has an effort dial"))
        );

        // `w:` is amx's own dial, with amx's own values.
        let mut line = Composer::new(Asking::Task);
        line.insert("w:");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("on, off or changes");
        assert_eq!(offered(&found), ["w:on", "w:off", "w:changes"]);

        // A dial the vendor does not declare offers nothing.
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };
        let mut line = Composer::new(Asking::Task);
        line.insert("m:");
        assert!(suggest(&line, &config, a_project(), &[]).is_none());
    }

    #[test]
    fn composer_offers_the_models_the_model_key_walks() {
        // With models listed in the config, `m:` offers what the model key
        // walks: the default sentinel, then the list.
        let told = Config {
            agent: "pi".to_string(),
            harnesses: std::collections::BTreeMap::from([(
                "pi".to_string(),
                crate::config::HarnessConfig {
                    models: vec![
                        "openai/gpt-5".to_string(),
                        "anthropic/claude-opus-5".to_string(),
                    ],
                    ..Default::default()
                },
            )]),
            ..Config::default()
        };
        let mut line = Composer::new(Asking::Task);
        line.insert("m:");
        let found = suggest(&line, &told, a_project(), &[]).expect("pi's models");
        assert_eq!(
            offered(&found),
            ["m:default", "m:openai/gpt-5", "m:anthropic/claude-opus-5"],
            "the sentinel and then the file's list, which is what the key walks"
        );

        // A partial value narrows the list.
        line.insert("openai/");
        let found = suggest(&line, &told, a_project(), &[]).expect("the one left");
        assert_eq!(offered(&found), ["m:openai/gpt-5"]);
    }

    /// Initialises a git repository in `dir` with one commit and `branches`,
    /// isolated from the user's git config.
    fn a_repo_on(dir: &Path, branches: &[&str]) {
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "amx tests")
                .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
                .env("GIT_COMMITTER_NAME", "amx tests")
                .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
                .output()
                .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "before\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-m", "first"]);
        for branch in branches {
            git(&["branch", branch]);
        }
    }

    #[test]
    fn composer_is_answering_an_on_with_the_branches_the_checkout_has() {
        let project = TempDir::new().unwrap();
        a_repo_on(project.path(), &["release", "spike", "spike-two"]);

        let mut line = Composer::new(Asking::Task);
        line.insert("on:");
        let found = suggest(&line, &as_claude(), project.path(), &[]).expect("what git has");
        assert_eq!(
            offered(&found),
            ["on:main", "on:release", "on:spike", "on:spike-two"],
            "the branches this checkout already has, spelled as the word would \
             be"
        );

        // Narrowed by what has been typed.
        line.insert("spike-");
        let found = suggest(&line, &as_claude(), project.path(), &[]).expect("the one left");
        assert_eq!(offered(&found), ["on:spike-two"]);

        line.insert("x");
        assert!(
            suggest(&line, &as_claude(), project.path(), &[]).is_none(),
            "and a name no branch answers to is the name somebody typed"
        );

        // Read in the `d:` directory, where the agent will run.
        let here = TempDir::new().unwrap();
        let mut line = Composer::new(Asking::Task);
        line.insert(&format!("d:{} on:rel", project.path().display()));
        let found = suggest(&line, &as_claude(), here.path(), &[]).expect("what is over there");
        assert_eq!(offered(&found), ["on:release"]);

        // Outside a repository, nothing.
        let mut line = Composer::new(Asking::Task);
        line.insert("on:");
        assert!(suggest(&line, &as_claude(), here.path(), &[]).is_none());
    }

    #[test]
    fn composer_offers_the_files_under_a_word_the_vendor_answers_to_with_none() {
        // `@` that names no agent completes files in the project. A word with
        // a `/` is never an agent name.
        let project = TempDir::new().unwrap();
        std::fs::create_dir_all(project.path().join("src/tui")).unwrap();
        std::fs::write(project.path().join("src/main.rs"), "fn main() {}").unwrap();

        let mut line = Composer::new(Asking::Task);
        line.insert("read @src/");
        let found = suggest(&line, &as_claude(), project.path(), &[]).expect("what is in it");
        assert_eq!(
            offered(&found),
            ["@src/main.rs", "@src/tui/"],
            "spelled as the word would be, with the separator on the directory \
             that says the path may go on"
        );

        // Narrowed by what has been typed.
        line.insert("m");
        let found = suggest(&line, &as_claude(), project.path(), &[]).expect("the one left");
        assert_eq!(offered(&found), ["@src/main.rs"]);

        line.insert("x");
        assert!(
            suggest(&line, &as_claude(), project.path(), &[]).is_none(),
            "and a path nothing answers to is the path somebody typed"
        );
    }

    #[test]
    fn composer_leaves_a_path_that_has_reached_a_directory_open_for_the_rest() {
        let project = TempDir::new().unwrap();
        std::fs::create_dir_all(project.path().join("src/tui")).unwrap();

        let mut line = Composer::new(Asking::Task);
        line.insert("read @src/t");
        line.suggest = suggest(&line, &as_claude(), project.path(), &[]);
        line.complete();
        assert_eq!(
            (line.text.as_str(), line.at),
            ("read @src/tui/", 14),
            "a path that has reached a directory is a word with more of itself \
             to come, so nothing is put between it and what is typed next"
        );
    }

    #[test]
    fn composer_reads_a_path_against_the_directory_the_line_aims_at() {
        // Read in the `d:` directory, where the agent will run.
        let here = TempDir::new().unwrap();
        let there = TempDir::new().unwrap();
        std::fs::create_dir_all(there.path().join("crates/importer")).unwrap();

        let mut line = Composer::new(Asking::Task);
        line.insert(&format!("d:{} port @crates/", there.path().display()));
        let found = suggest(&line, &as_claude(), here.path(), &[]).expect("what is over there");
        assert_eq!(offered(&found), ["@crates/importer/"]);
    }

    #[test]
    fn composer_offers_a_d_the_directories_and_the_projects_on_the_wall() {
        let here = TempDir::new().unwrap();
        std::fs::create_dir(here.path().join("app")).unwrap();
        std::fs::create_dir(here.path().join(".git")).unwrap();
        std::fs::create_dir(here.path().join(".amx")).unwrap();
        std::fs::write(here.path().join("README.md"), "words").unwrap();

        let elsewhere = TempDir::new().unwrap();
        let wall = [
            elsewhere.path().join("importer"),
            elsewhere.path().join("api"),
        ];

        let mut line = Composer::new(Asking::Task);
        line.insert("d:");
        let found = suggest(&line, &as_claude(), here.path(), &wall).expect("somewhere to run");
        assert_eq!(
            offered(&found),
            [
                "d:app/".to_string(),
                format!("d:{}", wall[0].display()),
                format!("d:{}", wall[1].display()),
            ],
            "what is under the view's own directory, which is where a name on \
             this dial is read, and then the projects somebody already has \
             agents in; a file is nowhere to run, and git's own directory and \
             amx's are nobody's"
        );

        // Narrowed by what has been typed, for either source.
        let mut line = Composer::new(Asking::Task);
        line.insert(&format!("d:{}/i", elsewhere.path().display()));
        let found = suggest(&line, &as_claude(), here.path(), &wall).expect("the one project");
        assert_eq!(offered(&found), [format!("d:{}", wall[0].display())]);
    }

    #[test]
    fn a_line_that_is_not_a_task_is_the_words_somebody_typed() {
        // Only task-like lines reach a vendor, so no other line completes.
        for asking in [
            Asking::Find,
            Asking::Name {
                id: "fix-login-a1b".to_string(),
            },
            Asking::Reply,
        ] {
            let mut line = Composer::new(asking);
            line.insert("agent:");
            assert!(
                suggest(&line, &as_claude(), a_project(), &[]).is_none(),
                "{}",
                line.label()
            );
        }
    }

    #[test]
    fn what_a_verb_wrote_becomes_one_line() {
        assert_eq!(
            one_line(b"fix-login-a1b stopped\nremoved /srv/app/.amx/worktrees/fix-login-a1b\n"),
            "fix-login-a1b stopped · removed /srv/app/.amx/worktrees/fix-login-a1b"
        );
        assert_eq!(one_line(b""), "");
    }

    #[test]
    fn backlog_keeps_the_lines_sent_newest_first_and_only_what_was_sent() {
        let mut sent = Backlog::default();
        sent.remember_line(&Asking::Task, "port the importer");
        sent.remember_line(&Asking::Task, "fix the login");
        sent.remember_line(&Asking::Reply, "yes, go on");
        assert_eq!(
            sent.lines_for(&Asking::Task),
            ["fix the login", "port the importer"],
            "newest first, and a task is not a reply"
        );
        assert_eq!(sent.lines_for(&Asking::Reply), ["yes, go on"]);

        // An immediate repeat is stored once.
        sent.remember_line(&Asking::Task, "fix the login");
        assert_eq!(sent.lines_for(&Asking::Task).len(), 2, "not kept twice");
        // Sent again after another line, it is stored again as the newest.
        sent.remember_line(&Asking::Task, "port the importer");
        assert_eq!(
            sent.lines_for(&Asking::Task),
            ["port the importer", "fix the login", "port the importer"]
        );

        // Only tasks and replies are kept.
        sent.remember_line(&Asking::Find, "login");
        sent.remember_line(
            &Asking::Name {
                id: "fix-login-b2c".to_string(),
            },
            "auth",
        );
        sent.remember_line(&Asking::Task, "   ");
        assert_eq!(sent.lines_for(&Asking::Find), [] as [&str; 0]);
        assert_eq!(
            sent.lines_for(&Asking::Task).len(),
            3,
            "a blank line is nothing sent"
        );

        // At most fifty; the oldest goes.
        for n in 0..60 {
            sent.remember_line(&Asking::Reply, &format!("line {n}"));
        }
        let replies = sent.lines_for(&Asking::Reply);
        assert_eq!(replies.len(), REMEMBERED_LINES);
        assert_eq!(replies[0], "line 59");
        assert_eq!(replies[49], "line 10");
    }

    #[test]
    fn composer_walks_back_over_the_lines_sent_and_gives_the_draft_back() {
        let sent = ["fix the login".to_string(), "port the importer".to_string()];
        let mut composer = Composer::new(Asking::Task);
        composer.insert("half a");
        composer.left();

        // The first step saves the draft and shows the newest line, cursor at
        // its end.
        assert!(composer.recall(&sent, true));
        assert_eq!((composer.text.as_str(), composer.at), ("fix the login", 13));
        assert!(composer.recall(&sent, true));
        assert_eq!(composer.text, "port the importer");
        // The walk stops at the oldest.
        assert!(!composer.recall(&sent, true), "nothing older to bring back");
        assert_eq!(composer.text, "port the importer");

        // Stepping newer past the newest restores the draft and its cursor.
        assert!(composer.recall(&sent, false));
        assert_eq!(composer.text, "fix the login");
        assert!(composer.recall(&sent, false));
        assert_eq!((composer.text.as_str(), composer.at), ("half a", 5));
        assert!(
            !composer.recall(&sent, false),
            "there is nothing newer than the draft"
        );
        assert_eq!((composer.text.as_str(), composer.at), ("half a", 5));

        // A new walk starts again from the newest.
        assert!(composer.recall(&sent, true));
        assert_eq!(composer.text, "fix the login");
    }

    #[test]
    fn composer_recalls_a_line_whole_and_keeps_the_drafts_pastes_for_it() {
        let sent = ["ship it".to_string()];
        let mut composer = Composer::new(Asking::Reply);
        let long = "x\n".repeat(PASTED_ROWS + 1);
        composer.paste(&long);
        let folded = composer.text.clone();
        assert_eq!(composer.pastes.len(), 1, "the draft holds a paste");

        // A recalled line comes back as plain text, with no pastes.
        assert!(composer.recall(&sent, true));
        assert_eq!(composer.text, "ship it");
        assert!(composer.pastes.is_empty());
        assert_eq!(composer.whole(), "ship it");

        // The draft comes back with its paste still folded.
        assert!(composer.recall(&sent, false));
        assert_eq!(composer.text, folded);
        assert_eq!(composer.whole(), long);

        // With no history, the line is left alone.
        let mut empty = Composer::new(Asking::Task);
        empty.insert("typed");
        assert!(!empty.recall(&[], true));
        assert_eq!(empty.text, "typed");
        assert!(!empty.recall(&[], false));
    }

    /// Writes a fake forge script that records its directory and arguments.
    ///
    /// Passed to [`opened`] by path, never put on `PATH`, so the suite never
    /// runs the machine's real gh.
    fn a_fake_forge(under: &Path, name: &str, wrote: &Path) -> PathBuf {
        let script = under.join(name);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > {0}.new\nmv {0}.new {0}\n",
                wrote.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// Runs [`opened`] against fakes and returns what the forge recorded.
    ///
    /// The spawn is not waited on, so this polls for the file. A spawn is
    /// retried when it fails with `Text file busy`, which happens while another
    /// test thread's fork still holds the just-written script open.
    fn told(at: &Path, gh: &Path, glab: &Path, wrote: &Path) -> Vec<String> {
        for _ in 0..20 {
            if opened(at, gh, glab, 12).is_err() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            for _ in 0..25 {
                if let Ok(said) = std::fs::read_to_string(wrote) {
                    return said.lines().map(str::to_string).collect();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        panic!("the forge never ran");
    }

    #[test]
    fn open_hands_the_request_to_gh_in_the_agents_own_directory() {
        let home = TempDir::new().unwrap();
        let wrote = home.path().join("said");
        let gh = a_fake_forge(home.path(), "gh", &wrote);
        let tree = home.path().join("tree");
        std::fs::create_dir(&tree).unwrap();

        // Only the number: the forge knows the repository from the directory.
        let said = told(&tree, &gh, Path::new("/nowhere/glab"), &wrote);
        assert_eq!(
            said[1..],
            ["pr", "view", "12", "--web"],
            "gh is asked to open the request in the browser: {said:?}"
        );
        assert_eq!(
            std::fs::canonicalize(&said[0]).unwrap(),
            std::fs::canonicalize(&tree).unwrap(),
            "in the directory the agent works in: {said:?}"
        );
    }

    #[test]
    fn open_asks_glab_where_there_is_no_gh_and_says_so_where_there_is_neither() {
        let home = TempDir::new().unwrap();
        let wrote = home.path().join("said");
        let glab = a_fake_forge(home.path(), "glab", &wrote);
        let missing = home.path().join("nothing-here");

        // The cached look does not record the forge, so without gh the
        // request is taken to be GitLab's.
        let said = told(home.path(), &missing, &glab, &wrote);
        assert_eq!(
            said[1..],
            ["mr", "view", "12", "--web"],
            "glab opens a merge request: {said:?}"
        );

        // With neither installed, the error says so.
        let why = format!(
            "{:#}",
            opened(home.path(), &missing, &missing, 12).unwrap_err()
        );
        assert!(
            why.contains("gh") && why.contains("glab"),
            "it names what is missing: {why}"
        );
    }

    #[test]
    fn open_reaps_the_forge_it_spawned() {
        let home = TempDir::new().unwrap();
        let wrote = home.path().join("pid");
        let gh = home.path().join("gh");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\necho $$ > {0}.new\nmv {0}.new {0}\n",
                wrote.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let pid = told_pid(home.path(), &gh, &wrote);
        let proc = PathBuf::from(format!("/proc/{pid}"));
        for _ in 0..250 {
            if !proc.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let stat = std::fs::read_to_string(proc.join("stat")).unwrap_or_default();
        panic!("the forge was never waited on: {stat}");
    }

    /// The pid of a fake forge that writes only `$$`, retried like [`told`].
    fn told_pid(at: &Path, gh: &Path, wrote: &Path) -> u32 {
        for _ in 0..20 {
            if opened(at, gh, Path::new("/nowhere/glab"), 12).is_err() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            for _ in 0..25 {
                if let Ok(said) = std::fs::read_to_string(wrote) {
                    return said.trim().parse().unwrap();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        panic!("the forge never ran");
    }
}
