//! Doing something about what the view is showing.
//!
//! Each of these is the verb that does the same thing, run in the view's own
//! process rather than shelled out to: the same records, the same tmux, the
//! same laws.
//! Two things are deliberately not the verb itself. A verb says what it could
//! not do on stderr, which a terminal in raw mode is in no position to receive
//! — so what these answer with is the line the view puts where its keys are.
//! And a verb may wait: `send` gives the vendor five seconds to say the text
//! arrived. A view holding a screen open cannot spend five seconds anywhere,
//! and it does not have to, because the next reading is where that word shows
//! up anyway.
//!
//! Which reply an agent gets is decided by what it is doing at the moment the
//! line is entered, not at the moment it was opened: text typed at a
//! permission prompt answers the prompt, and a turn can end while somebody is
//! still typing.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, Ref, RefCell};
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::paint::Card;
use super::rows::{Narrow, shorten};
use crate::catalog::{self, Entry};
use crate::cli::{AgentArgs, AnswerArgs, NewArgs, StopArgs};
use crate::config::Config;
use crate::derive::View;
use crate::store::{Agent, Ask, Kind, Phase};
use crate::tmux::Server;
use crate::verbs::answer::Answered;
use crate::{derive, exit, registry, spawn, store, verbs, worktree};

/// A line somebody is typing, and what it is for.
pub struct Composer {
    pub asking: Asking,
    pub text: String,
    /// Where the next character lands, counted in characters of the line
    /// rather than bytes: what somebody sees the block standing on is a
    /// character, and a line takes whatever they can type into it.
    ///
    /// A line opens with it at the end — of nothing on a new line, and of the
    /// name a rename is opened on, because a name is edited rather than typed
    /// again from the start.
    pub at: usize,
    /// What the next agent may do without asking, for the rule over the line
    /// to carry at the far end of itself.
    ///
    /// It is a fact about the view rather than about the line — which
    /// permission the dial is resting on, and it moves under the line as
    /// shift+tab is pressed — and the band that draws the rule is handed the
    /// line and not the view. So the reading is taken once a frame where the
    /// view is in hand and left here, which is the errand the paint map's
    /// cells run in the other direction.
    pub allowed: Cell<Option<String>>,
    /// What the word under the cursor could be, where it could be something.
    pub suggest: Option<Suggest>,
    /// The vendor's catalog as this line last read it, and who it was read
    /// for. A cell for the reason `allowed` is one: the suggestions are taken
    /// off a reading of the line, and what the reading finds out along the way
    /// is left here for the next one.
    pub listed: RefCell<Option<Listed>>,
    /// Where the line will run: the project the wall was showing when it was
    /// opened, and nothing where the wall was not showing one.
    ///
    /// Taken when the line opens rather than read again when it is entered,
    /// because it is where somebody was looking as they typed. A `d:` on the
    /// line says it instead — a directory named in words is somebody saying
    /// where, and the cursor is only where they were.
    pub under: Option<PathBuf>,
    /// What each marker on the line stands for, in the order they were
    /// numbered: the first is `[Pasted text #1]`.
    ///
    /// Kept beside the line rather than in it, which is the whole of the fold:
    /// what is drawn is one row a person can read the rest of their task
    /// around, and what is sent is every character they pasted.
    pub pastes: Vec<String>,
    /// Where a walk back through the lines sent before is standing, while
    /// one is under way.
    pub walking: Option<Walking>,
}

/// A walk back through the lines sent before: which of them is on the line,
/// and what the line held when the walk began.
pub struct Walking {
    /// Which of the lines sent is on the line now, newest first.
    at: usize,
    /// The line as it was when the first step was taken — text, cursor and
    /// pastes — given back by the step past the newest, so a line half
    /// written is not lost to a look at the last one.
    draft: (String, usize, Vec<String>),
}

/// The lines the view has sent, for a line being typed to bring back.
///
/// Two lists, because what went to an agent is not what the next agent would
/// be started with: a task and a reply are different sentences to different
/// listeners. Newest first, a line equal to the newest not kept twice, and
/// fifty of each. A shell's history is longer, but a shell's history is the
/// whole of what somebody typed, and this is only what the view sent.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backlog {
    tasks: Vec<String>,
    replies: Vec<String>,
}

/// How many lines each list keeps.
const REMEMBERED_LINES: usize = 50;

impl Backlog {
    /// Keep a line the view has just sent, newest first.
    ///
    /// A rename and a find line are not sent, and a digit pressed at a
    /// question is a choice rather than a line, so only a task and a reply
    /// have a list to go on. The same line sent twice running is kept once:
    /// walking back over two copies of it would be a step that went nowhere.
    pub fn remember_line(&mut self, asking: &Asking, line: &str) {
        let lines = match asking {
            Asking::Task => &mut self.tasks,
            Asking::Reply => &mut self.replies,
            Asking::Name { .. } | Asking::Find => return,
        };
        if line.trim().is_empty() || lines.first().is_some_and(|newest| newest == line) {
            return;
        }
        lines.insert(0, line.to_string());
        lines.truncate(REMEMBERED_LINES);
    }

    /// The lines sent from this kind of line, newest first, and none from a
    /// kind that sends nothing.
    pub fn lines_for(&self, asking: &Asking) -> &[String] {
        match asking {
            Asking::Task => &self.tasks,
            Asking::Reply => &self.replies,
            Asking::Name { .. } | Asking::Find => &[],
        }
    }
}

/// The words a vendor would answer to where the cursor is standing, as they
/// stood at the last keystroke.
///
/// Taken again on every keystroke rather than held, so that the list under the
/// word is never a keystroke behind it. What it is narrowed from is the
/// catalog the line read on the first keystroke that asked for one, which is
/// [`Listed`]'s to keep.
pub struct Suggest {
    /// Where the word stands on the line, counted in characters the way the
    /// cursor is, because it is what taking a suggestion writes over.
    pub word: Range<usize>,
    /// What answers to it, narrowed by what has been typed of it. Never empty:
    /// a word nothing answers to has no suggestions rather than an empty list
    /// of them.
    pub entries: Vec<Entry>,
    /// Which of them the choice is standing on.
    pub chosen: usize,
}

/// Everything the vendor loads by name, as it stood when this line first
/// asked, and who it was read for.
///
/// Read once for the life of the line rather than on every keystroke. The
/// catalog is a walk of every place the vendor declares and a read of every
/// file's frontmatter under them, and a line is typed a character at a time:
/// thirty files is nothing, and a plugin cache of a few hundred is a line that
/// lags behind the fingers typing it. What is narrowed on the keystroke is
/// what was typed, and that costs nothing.
///
/// The vendor and the project because they are what the catalog is a function
/// of: `agent:pi` typed onto a line that has been offering claude's skills is
/// a different vendor's directories, and a `d:` aimed at another project is
/// that project's own places. Either one changing is the reading gone, and
/// the next keystroke takes it again.
pub struct Listed {
    agent: String,
    project: PathBuf,
    entries: Vec<Entry>,
}

/// What entering the line will do.
pub enum Asking {
    /// A task, for an agent that does not exist yet.
    Task,
    /// Something for the agent the card is a look at: a message, or the key
    /// its question is waiting for.
    ///
    /// It names no agent, because the card it stands at the foot of does: the
    /// line is the card's last row, and which agent that card is showing is
    /// the view's to say at the moment enter is pressed rather than the
    /// line's to have remembered from the moment it opened.
    Reply,
    /// A name for one of them, which goes nowhere near the agent itself.
    Name { id: String },
    /// Which agents to keep on the wall. Not a line that is sent: it is read
    /// on every keystroke, so the list under it is already narrowed by the
    /// time somebody has finished typing what they were looking for.
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

    /// Bring back a line sent before, or the one after it.
    ///
    /// `older` walks toward the oldest and stops there; the other way walks
    /// toward the newest, and the step past it puts back what was being typed
    /// when the walk began — text, cursor and pastes — so a line half written
    /// is not lost to a look at the last one. A recalled line arrives whole,
    /// the cursor at its end and no pastes standing beside it: it was sent as
    /// characters and it comes back as characters. Answers whether the line
    /// changed, which a step past either end does not.
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
        self.text = line.clone();
        self.at = line.chars().count();
        self.pastes.clear();
        true
    }

    /// Put text in where the cursor is, and leave the cursor after it.
    ///
    /// One character or a whole paste through the same door: both are text
    /// arriving at the one place on the line that takes text, and what the
    /// cursor was standing on is still in front of it afterwards.
    pub fn insert(&mut self, text: &str) {
        let at = self.byte();
        self.text.insert_str(at, text);
        self.at += text.chars().count();
    }

    /// Put a paste in where the cursor is, folded behind a marker where it is
    /// long enough to bury the line it landed on.
    ///
    /// A clipboard holds whole files, and a task typed around one of them is a
    /// task nobody can read: the log somebody pasted scrolls the sentence
    /// asking about it off the top of the composer. So a long paste stands as
    /// `[Pasted text #N]` — one row, in the place on the line where it landed,
    /// and the text itself waiting beside the line until it is sent.
    ///
    /// Anything shorter goes in as it is. A folded phrase would be a line
    /// somebody could not read back, and the marker is only worth its
    /// awkwardness where what it hides was going to be unreadable anyway.
    ///
    /// The same text pasted onto a line whose marker for it is still standing
    /// unfolds that marker instead of raising a second one beside it. Pressing
    /// paste again on a row that came back where a file was expected is
    /// somebody asking to see what they pasted, and this is the one press that
    /// says so — so the text goes back in the place the marker was holding for
    /// it, and the line reads as what will be sent. What is sent does not
    /// move: the marker went, and the paste it stood for is the characters
    /// now standing there.
    ///
    /// A marker taken back is not there to unfold, so the same text pasted
    /// again after that folds afresh, under the next number. The paste beside
    /// the line keeps the number it was given either way, because that number
    /// is how [`whole`](Self::whole) reads every other marker on the line.
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

    /// Where the line's marker for a paste it is already holding equal to this
    /// text stands, in bytes, and nothing where no such marker is on the line.
    ///
    /// The text and not the number, because a person pastes a clipboard rather
    /// than a marker: what says this is the same paste is that it is the same
    /// characters.
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

    /// The line as whatever it is sent to will be given it: every marker on it
    /// back to the paste it stands for.
    ///
    /// The line itself is what is drawn and what the cursor walks, so nothing
    /// here writes to it. A marker somebody has edited into something that is
    /// no longer one stands as the characters they left, which is what
    /// deleting half of it asked for.
    pub fn whole(&self) -> String {
        self.pastes
            .iter()
            .enumerate()
            .fold(self.text.clone(), |line, (at, pasted)| {
                line.replace(&marker(at + 1), pasted)
            })
    }

    /// Take the character behind the cursor, and the one under it.
    ///
    /// Neither reaches past the end it is standing at: a backspace at the front
    /// of the line and a delete at the back of it are one press more than
    /// somebody meant, not a character taken from the other end.
    ///
    /// A marker standing against the cursor goes whole. It is one row for one
    /// paste, and a press that took a character off it would leave a row that
    /// is no longer a marker and no longer the paste either: what `whole`
    /// sent then would be the broken bracket, and the text it stood for would
    /// go nowhere without anybody being told. Taking the character is what
    /// somebody meant by the press; taking the paste is what the marker means.
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

    /// Take the word behind the cursor, in one edit.
    ///
    /// The word the cursor would have walked back over, because a chord that
    /// deleted by one rule while the arrow beside it moved by another would be
    /// two words to keep in mind for one word on the line. A marker is the
    /// word it stands as, spaces and all, for the reason `delete_back` gives.
    pub fn delete_word_back(&mut self) {
        if self.marker_behind().is_some() {
            return self.delete_back();
        }
        let to = self.byte();
        self.word_left();
        let from = self.byte();
        self.text.replace_range(from..to, "");
    }

    /// The marker the cursor is standing at the end of, as the characters it
    /// spans, and the one it is standing at the front of. Only a marker for a
    /// paste this line holds: the bracket typed by hand is the characters it
    /// is.
    ///
    /// A cursor walked into the middle of one is left to the edit it makes;
    /// the two presses that mean "take that paste back" are the ones that
    /// land against its ends.
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

    /// One character back, and one on. Neither walks off the line: the ends of
    /// it are where a cursor stops.
    pub fn left(&mut self) {
        self.at = self.at.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.at = (self.at + 1).min(self.length());
    }

    /// Both ends of it, whatever it is holding. The whole line rather than the
    /// row the cursor is on: a task pasted over four rows is one line, and the
    /// end of it is where the line ends.
    pub fn home(&mut self) {
        self.at = 0;
    }

    pub fn end(&mut self) {
        self.at = self.length();
    }

    /// A word at a time: whatever whitespace is in the way, and then the run
    /// of characters behind or in front of it.
    ///
    /// Whitespace and not punctuation, because what is on this line is a
    /// sentence somebody is writing: `m:opus` is one word of it, and a chord
    /// that stopped inside the dial would be a chord nobody could aim.
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

    /// Move the choice through the suggestions, wrapping at both ends: a list
    /// walked with two keys has nowhere else for them to stop.
    ///
    /// Nothing where there are no suggestions, which is what leaves a line
    /// without an up and a down of its own.
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

    /// Put the suggestion the choice is on where the word under the cursor is,
    /// and a space after it.
    ///
    /// The space is what says the word is finished: what is being completed is
    /// one word of a sentence, and the next thing typed is the next word
    /// rather than more of this one. Never where the line already has one
    /// there, because a word mended in the middle of a sentence is not a word
    /// that pushes the next one along — and never after a directory, because a
    /// path that has reached one is a word with more of itself to come. The
    /// suggestions go with it either way, since the word they were about is now
    /// the word one of them named.
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

    /// Whether the word under the cursor is still being finished: there are
    /// suggestions under it, and it is not yet spelled the way the choice
    /// spells it.
    ///
    /// This is what tells enter which of its two jobs it has. A line with a
    /// list open under it is a line somebody is still writing a word of, and
    /// enter finishes the word — but a word already spelled the way the choice
    /// spells it is a finished word, and enter on a finished word is enter on
    /// the line. Without the distinction `/review` typed out to the end
    /// leaves one suggestion, itself, and takes two enters to send: one to
    /// put a space after a word that needed nothing, and one to mean it. Tab
    /// asks nothing of this, because tab has the one job.
    ///
    /// The choice rather than any of the entries: a word spelled like one of
    /// them while the choice was walked to another is somebody choosing the
    /// other, and enter gives them what they walked to.
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

    /// The vendor's catalog for this line: the one already read for this
    /// vendor and this project, and otherwise what `read` finds, kept for the
    /// next keystroke.
    ///
    /// `read` is handed in rather than called here so that what fills the
    /// reading and what decides whether to take it again are two things, and
    /// the second can be proved without a disk.
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

    /// Where the cursor stands as a byte of the line, which is what the string
    /// under it is cut by. Past the last character it is the end of the line,
    /// which is where a line being typed usually is.
    fn byte(&self) -> usize {
        self.byte_at(self.at)
    }

    /// The same reading for any character of the line, which is how a word
    /// somewhere else on it is cut out.
    fn byte_at(&self, at: usize) -> usize {
        self.text
            .char_indices()
            .nth(at)
            .map_or(self.text.len(), |(byte, _)| byte)
    }

    /// How many characters the line is, which is where its end is.
    fn length(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether this line runs a command rather than starting an agent, which
    /// is what the bang it opens with says.
    ///
    /// A task line only: every other line goes to an agent that is already
    /// running or narrows the wall, and a bang typed on one of those is the
    /// character it is.
    ///
    /// Read off the whole of it, because that is what enter is handed: a
    /// pasted script folded into a marker still opens with the bang it was
    /// copied with, and a rule saying TASK over a line about to run a shell
    /// would be the one thing the rule is there to say, said wrong.
    pub fn commanding(&self) -> bool {
        matches!(self.asking, Asking::Task) && self.whole().starts_with(BANG)
    }

    /// What the rule over the line calls the mode, in the one word a band's
    /// edge has room for.
    ///
    /// Uppercase, the way every heading on the wall is: a label on a border is
    /// read at a glance or not at all. The one thing a person needs to know
    /// before pressing enter is what enter is about to do, which is why the
    /// word changes under the bang as it is typed and as it is taken back:
    /// what a line starts is what the label is about.
    pub fn label(&self) -> &'static str {
        if self.commanding() {
            return "COMMAND";
        }
        match &self.asking {
            Asking::Task => "TASK",
            // Nothing draws a rule over a reply: it is the card's own last
            // row, under a rule the card has already written the agent's name
            // on.
            Asking::Reply => "REPLY",
            Asking::Name { .. } => "RENAME",
            // Nothing draws a rule over a find line: it is one row at the
            // foot, so the label has nowhere to be said and nothing to say.
            Asking::Find => "FIND",
        }
    }

    /// What the line is aimed at, where it is aimed at anything.
    ///
    /// The label alone does not say it, and it is what somebody about to press
    /// enter has to be sure of: a rename renames one agent and nothing else.
    /// A task is aimed at nobody yet, so what it says instead is
    /// the project it will run in — the one thing about a spawn that the rule
    /// can say before there is an agent to name — and nothing where that is
    /// the directory the view was opened in, which is where a task runs unless
    /// something says otherwise.
    ///
    /// It stands on the rule beside the label rather than in front of the
    /// line, so every line the band draws begins in the same column. The path
    /// is written the way the heading it was read off writes it, because it is
    /// the same place said twice on one screen.
    pub fn about(&self) -> Option<String> {
        match &self.asking {
            Asking::Task => self
                .under
                .as_deref()
                .map(|dir| format!("in {}", shorten(dir, std::env::home_dir().as_deref()))),
            // And a reply names nobody here, because the card's rule above it
            // already names the agent it is going to.
            Asking::Find | Asking::Reply => None,
            Asking::Name { id } => Some(id.clone()),
        }
    }
}

/// How much of a paste is too much to leave on the line: the characters a
/// composer can show at its widest, and the rows it can show at its tallest.
///
/// Either of them, because a paste is long in one of two ways and both of them
/// bury the line: a thousand characters of one paragraph wrap into rows, and
/// four short rows are four rows.
const PASTED_CHARACTERS: usize = 800;
const PASTED_ROWS: usize = 3;

/// What a folded paste stands as, numbered from one for the line it was
/// pasted onto.
///
/// Per line rather than across the view: the number is read on the row it is
/// drawn on, and a second paste onto a fresh line is that line's first.
fn marker(nth: usize) -> String {
    format!("[Pasted text #{nth}]")
}

/// A find line of nothing but `s:` tokens narrows the list by state; anything
/// else is the name to look for.
///
/// Nothing but: "s:waiting is what to check" is a sentence somebody may well
/// be looking for an agent by, and a surface that guessed otherwise would be
/// one nobody could type into.
///
/// The task line read these once, and an `a:` beside them that narrowed by
/// name. `/` does the whole of it now, on every keystroke and without a line
/// to open first — so the tokens live on the one line that narrows anything,
/// and the line a task is typed on starts an agent and nothing else.
pub fn narrowing(line: &str) -> Option<Vec<Narrow>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() || !tokens.iter().all(|token| token.starts_with(STATE)) {
        return None;
    }

    Some(
        tokens
            .iter()
            .map(|token| {
                // A token with nothing after it drops that narrowing.
                let want = &token[STATE.len()..];
                Narrow::State((!want.is_empty()).then(|| want.to_string()))
            })
            .collect(),
    )
}

/// The one token that narrows by something other than the name.
pub const STATE: &str = "s:";

/// What a find line narrows the list to, which is anything somebody types.
///
/// A line of nothing but `s:` tokens narrows by state. Anything else is the
/// name to look for, whole and untokenised: `/` is a search box before it is a
/// grammar, and somebody typing `port the` means an agent called that rather
/// than two filters.
///
/// An empty line narrows to nothing, which is what puts the fleet back as the
/// last character is deleted.
pub fn finding(line: &str) -> Vec<Narrow> {
    if let Some(narrowing) = narrowing(line) {
        return narrowing;
    }
    let want = line.trim();
    vec![Narrow::Name((!want.is_empty()).then(|| want.to_string()))]
}

/// The tokens a task line may be led with, and what each of them turns.
const DIALS: [&str; 5] = [MODEL, PERMISSION, WORKTREE, DIR, AGENT];

/// The one of them that says which vendor the line is for, which is the vendor
/// every other word on it is read against.
const AGENT: &str = "agent:";

/// The two the vendor declares, whose values are the vendor's own to name.
const MODEL: &str = "m:";
const PERMISSION: &str = "p:";

/// And the two that are amx's: whether this agent is given a tree of its own,
/// and where it runs.
const WORKTREE: &str = "w:";
const DIR: &str = "d:";

/// What `w:` takes, which is amx's own answer and in no vendor's table.
const TREE: [&str; 2] = ["on", "off"];

/// The mark a file is named by, which is the vendor's own and the same one an
/// agent is named by.
const AT: &str = "@";

/// The mark a command row is led with, which is the shell's own: a line that
/// opens with it runs what is after it instead of asking an agent to.
const BANG: char = '!';

/// What a line's leading tokens turn, for the one spawn they lead. Empty is
/// the ordinary line, which leaves every dial where the config put it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Turned {
    /// Whether the line runs a command rather than starting an agent, which is
    /// what the bang it opens with says.
    pub exec: bool,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub permission: Option<String>,
    /// Whether this agent is given a tree of its own, when the line said.
    pub worktree: Option<bool>,
    /// Where this one runs, as it was typed. Kept as the word on the line
    /// rather than a path: what `~` and a relative name mean is the running
    /// view's business, and this is only what somebody asked for.
    pub dir: Option<String>,
}

/// What starting an agent came to.
pub enum Started {
    /// It is running: which one, and the line the view says so on. Both,
    /// because the id is what a key that goes to the new agent addresses and
    /// the line is what a person reads.
    Yes { id: String, said: String },
    /// Nothing was made, and this says why.
    No(String),
}

/// Split a task line into the dial tokens at the front of it and the task
/// itself.
///
/// Leading only: `port the m:opus importer` is a task with a colon in it,
/// under the same law that keeps `s:waiting is what to check` one. A task that
/// has to *begin* with one of these words is what `amx new` at a shell prompt
/// is for.
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

/// The dials a task line turns and the task that is left, or the word that is
/// not a value for the dial it was typed at.
///
/// What a dial takes is the vendor's business, and the answer comes from the
/// table `new` resolves against: a value amx passed on and the vendor refused
/// would be an agent that died in its pane with the reason scrolled past.
///
/// `agent:` is the exception and takes any command, the way `--agent` does at
/// a shell prompt. The registry is launch metadata rather than a list of who
/// may be launched, and an agent it has never heard of has always been allowed
/// to spawn; what it costs is its dials, which `m:` and `p:` beside it say by
/// name.
///
/// A line led with the bang is not a task at all and is read by [`commanded`]:
/// what is left of it is a command, and the dials it may lead are its own.
pub fn turned(config: &Config, line: &str) -> Result<(Turned, String), String> {
    if let Some(rest) = line.strip_prefix(BANG) {
        return commanded(rest);
    }
    let (tokens, task) = tokens(line);
    let mut turned = Turned::default();

    // Which vendor first: which dials exist at all is its answer, and a line
    // may name one the config does not.
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
                    _ => return Err(format!("w:{value}: on or off")),
                });
            }
            DIR => {
                if value.is_empty() {
                    return Err("d: takes a directory".to_string());
                }
                turned.dir = Some((*value).to_string());
            }
            MODEL => {
                turned.model = Some(pointed(&agent, dial, entry.and_then(|e| e.model), value)?);
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
    Ok((turned, task.to_string()))
}

/// The one dial a command row takes and the command that is left, or the word
/// that is a dial it has nothing to turn.
///
/// `d:` alone, because it is the only one of the five that is about the row
/// rather than about an agent: where the command runs. A row that runs `sh -c`
/// launches no vendor, so the vendor's own two and the word that names one have
/// nothing here to be about — which is why `--exec` refuses those flags at a
/// shell prompt too. `w:` goes with them: a command is not a conversation to
/// keep apart from the next one, so it runs in the checkout it was typed in
/// whatever any line says.
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

/// A value for one of the vendor's own dials, or why it is not one.
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

/// What the word under the cursor could be, where it could be something.
///
/// A task line only. The other lines go to an agent that is already running,
/// and what a vendor loads by name is what a task line asks it for. A command
/// row asks it for nothing either: it runs a shell, where `/etc` is a directory
/// rather than the front of a skill's name.
///
/// Read on the keystroke, the way the find line narrows the wall on one: a
/// suggestion arriving after the word it was about has been finished is no use
/// to anybody. What it costs is a directory read for a path, and the vendor's
/// catalog once for the line — and only for a word that opens with one of the
/// marks that ask for one. An ordinary sentence asks nothing of the disk.
///
/// `wall` is the projects the view is showing agents in, which is what a `d:`
/// is offered besides the directories under it.
pub fn suggest(
    composer: &Composer,
    config: &Config,
    project: &Path,
    wall: &[PathBuf],
) -> Option<Suggest> {
    // A task line, and the line at the foot of the card: the vendor's words
    // are the same words said to an agent already running, and `config` and
    // `project` are that agent's own there rather than the header's dials.
    // The dials are the task line's alone — see [`answering`].
    if !matches!(composer.asking, Asking::Task | Asking::Reply) || composer.commanding() {
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

/// The word the cursor is standing in, in characters, and nothing where it is
/// standing on whitespace.
///
/// The whole word rather than the part in front of the cursor: what a
/// suggestion goes in the place of is the word somebody is mending, and a
/// letter put back into the middle of `/reveiw` is one word being written and
/// not two.
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

/// Which vendor's words these are: the `agent:` the line is led with, and the
/// one the view is holding otherwise.
///
/// The line's own first, because it is what the agent this line starts will
/// be: a word offered out of the dial's vendor would be a word the vendor
/// named beside it has never heard of.
fn asked_of(config: &Config, line: &str) -> String {
    let (tokens, _) = tokens(line);
    tokens
        .iter()
        .find(|(dial, value)| *dial == AGENT && !value.is_empty())
        .map_or_else(|| config.agent.clone(), |(_, value)| (*value).to_string())
}

/// What answers to the word being typed, narrowed to what is typed of it.
///
/// Three kinds of word, in the order a mark is read. The dials are amx's own
/// and are answered out of the table and off the disk: `agent:` by the vendors
/// there are entries for, `m:` and `p:` by the cycle the vendor declares, `w:`
/// by amx's two words, and `d:` by the directories a path names. The marks past
/// them are the vendor's own: `/` runs a skill, a command or something the
/// vendor answers out of itself, and `@` names one of its agents. And a word
/// naming none of those is the third kind — a path, which is the other thing
/// the mark a vendor reads a file by is for.
///
/// A vendor amx has measured no places for names nothing of its own, and so
/// does a machine with no home directory for its places to hang off; the files
/// under the cursor are still there, since they are the project's rather than
/// anybody's catalog.
fn answering(
    composer: &Composer,
    typed: &str,
    config: &Config,
    project: &Path,
    wall: &[PathBuf],
) -> Vec<Entry> {
    let line = composer.text.as_str();
    // The dials are the task line's alone: they say what an agent is started
    // with, and the line at the foot of the card goes to one already running
    // under whatever it was started with. There every one of them is a word
    // of the message, `agent:` included — and the vendor asked is the
    // agent's own, which is what the line was handed.
    let starting = matches!(composer.asking, Asking::Task);
    let agent = match starting {
        true => asked_of(config, line),
        false => config.agent.clone(),
    };
    if starting {
        if typed.starts_with(AGENT) {
            return vendors(typed);
        }
        if let Some(values) = dialled(&agent, typed) {
            return values;
        }
        // A `d:` being typed is the one path on the line that is not read
        // against the `d:`: it is what the rest of them will be read against.
        if typed.starts_with(DIR) {
            let mut found = paths(DIR, typed, project, true);
            found.extend(on_the_wall(wall, typed));
            return found;
        }
    }

    let kinds: &[catalog::Kind] = match typed.chars().next() {
        Some('/') => &[
            catalog::Kind::Skill,
            catalog::Kind::Command,
            catalog::Kind::Builtin,
        ],
        Some('@') => &[catalog::Kind::Agent],
        _ => return Vec::new(),
    };
    let listed = composer.catalog(&agent, project, || catalogued(&agent, project));
    let named = named(&listed, typed, kinds);
    // A path on a task line is read where its `d:` says the agent will run;
    // on the card's line, where the agent already runs.
    let here = match starting {
        true => running(line, project),
        false => project.to_path_buf(),
    };
    match named.is_empty() && typed.starts_with(AT) {
        true => paths(AT, typed, &here, false),
        false => named,
    }
}

/// What the vendor loads by name, out of the places its entry declares.
fn catalogued(agent: &str, project: &Path) -> Vec<Entry> {
    let places = registry::entry(agent).and_then(|vendor| vendor.catalog);
    let (Some(places), Some(home)) = (places, std::env::home_dir()) else {
        return Vec::new();
    };
    catalog::listing(&places, &home, project)
}

/// The entries of these kinds that answer to what has been typed of the word.
fn named(entries: &[Entry], typed: &str, kinds: &[catalog::Kind]) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| kinds.contains(&entry.kind) && entry.spelled.starts_with(typed))
        .cloned()
        .collect()
}

/// The vendors amx has an entry for, as the words that aim a line at one.
///
/// Out of the table: a vendor is in no file of anybody's for a sentence about
/// it to be read from.
fn vendors(typed: &str) -> Vec<Entry> {
    registry::entries()
        .iter()
        .map(|vendor| worded(format!("{AGENT}{}", vendor.name)))
        .filter(|entry| entry.spelled.starts_with(typed))
        .collect()
}

/// What the dial a word is typed at takes, and nothing where the word is typed
/// at no dial.
///
/// The vendor's own cycle for the vendor's own two, which is the list the key
/// under the header offers and the list `turned` reads a value against: a line
/// that suggested a word the spawn would refuse would be offering somebody a
/// refusal. A dial this vendor does not declare has no values to offer, which
/// is the same silence `turned` refuses the token in.
fn dialled(agent: &str, typed: &str) -> Option<Vec<Entry>> {
    let vendor = registry::entry(agent);
    let (dial, cycle): (&str, &[&str]) = match typed {
        _ if typed.starts_with(MODEL) => (MODEL, vendor?.model?.cycle),
        _ if typed.starts_with(PERMISSION) => (PERMISSION, vendor?.permission?.cycle),
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

/// What is in the directory a path names, as the words that would finish the
/// one being typed.
///
/// The path is read the way a shell prompt standing in `here` would read it: a
/// leading `~` is the home directory, and a name that is not absolute is under
/// `here`. What comes back is spelled as it was typed, mark and all, because a
/// suggestion goes in the place of the whole word.
///
/// A directory carries the separator that says the path may go on, which is
/// also what keeps a space off the end of it when the word is taken. `.git` and
/// `.amx` are left out: they are in every project a line is typed in and
/// neither is anybody's next word.
///
/// `folders` is whether only directories answer, which is what a `d:` takes.
fn paths(mark: &str, typed: &str, here: &Path, folders: bool) -> Vec<Entry> {
    let said = typed.strip_prefix(mark).unwrap_or(typed);
    // The directory it names and the part of a name that has been typed: what
    // is being narrowed is the last segment, and everything in front of it is
    // where to look.
    let (dir, leaf) = match said.rfind('/') {
        Some(at) => said.split_at(at + 1),
        None => ("", said),
    };
    // A directory nothing is at, or one nobody may read, offers nothing and
    // says nothing: somebody typing a task is owed suggestions or none.
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
    // By name, so what a machine offers does not depend on the order a
    // filesystem happens to hand its entries back.
    found.sort_by(|one, two| one.spelled.cmp(&two.spelled));
    found
}

/// The directories in every project that no line names: git's own, and amx's.
const KEPT_BACK: [&str; 2] = [".git", ".amx"];

/// Every project an agent on the wall runs in, as the words that aim a line at
/// one.
///
/// The wall's own answer rather than a walk of anybody's disk: where somebody
/// starts an agent is nearly always where they already have one, and those
/// directories are rarely under the one the view was opened in for a path to
/// reach in a word.
fn on_the_wall(wall: &[PathBuf], typed: &str) -> Vec<Entry> {
    wall.iter()
        .map(|project| worded(format!("{DIR}{}", project.display())))
        .filter(|entry| entry.spelled.starts_with(typed))
        .collect()
}

/// Where a path typed on this line is read from: the directory the line's own
/// `d:` names, and the one the view is running in otherwise.
///
/// The `d:` because that is where the agent this line starts will run, and a
/// file offered out of anywhere else is a file it would not find. A `d:`
/// nothing is at yet is a word somebody is still typing, and the view's own
/// directory is what a path is read against until it is a directory.
fn running(line: &str, project: &Path) -> PathBuf {
    let (tokens, _) = tokens(line);
    tokens
        .iter()
        .find(|(dial, value)| *dial == DIR && !value.is_empty())
        .and_then(|(_, value)| aimed(value, project).ok())
        .unwrap_or_else(|| project.to_path_buf())
}

/// One word amx offers out of itself: a vendor, a dial's value, a file.
///
/// Nothing to say about any of them, because there is no file to read a
/// sentence from — the word is the whole of what it says.
fn worded(spelled: String) -> Entry {
    Entry {
        spelled,
        kind: catalog::Kind::Builtin,
        about: String::new(),
    }
}

/// What editing a line in an editor came to.
pub enum Edited {
    /// The editor was closed on this, and it is the line now.
    Line(String),
    /// It said it wanted none of it, and this says how: the line stays where
    /// it was, with what was typed on it.
    No(String),
}

/// What a line is edited in: what somebody configured, and `vi` where they
/// configured nothing, which is the editor a unix box is obliged to have.
///
/// `$VISUAL` first, because that is the one that names a program for a terminal
/// somebody is sitting at, and this line is being edited on the terminal they
/// are sitting at.
fn editor() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|said| !said.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Give the line to an editor and take back whatever it was left holding.
///
/// A file rather than a pipe, because an editor is a program that opens a file:
/// `$EDITOR` is routinely a command line of its own, so the whole of it is
/// handed to a shell with the file behind it, exactly as `git commit` does it.
pub fn edited(text: &str) -> Result<Edited> {
    let path = std::env::temp_dir().join(format!("amx-task-{}.md", std::process::id()));
    edited_in(&editor(), &path, text)
}

/// The same, with the editor and the file it opens both named, because a test
/// has to be able to say what the person at the terminal would have done.
fn edited_in(editor: &str, path: &Path, text: &str) -> Result<Edited> {
    // Unlinked and then made new rather than truncated: the directory this
    // sits in is everybody's, and a name amx can work out is a name somebody
    // else can work out too. Making it new refuses a file already standing
    // there instead of writing through whatever it points at.
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
    // Whatever came of it, the task is not left lying in a directory everybody
    // can read.
    let _ = std::fs::remove_file(path);

    if !status.success() {
        return Ok(Edited::No(format!("{editor} left the line as it was")));
    }
    let written = written.with_context(|| format!("reading {} back", path.display()))?;
    // The newline a file ends with is the file's own. Everything above it is
    // the task, newlines and all.
    Ok(Edited::Line(
        written.trim_end_matches('\n').replace("\r\n", "\n"),
    ))
}

/// A path as one word to a shell, whatever is in it.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// How many characters a task is before the view takes it for one.
///
/// Four, measured against what a stray keystroke leaves behind: `n` opens the
/// line and the letter after it is a task nobody typed, and an agent started on
/// `w` is a minute of somebody's afternoon and a record to clear away. Nothing
/// this short is refused — it is asked about, once, because "wip" is a task
/// somebody means.
const ENOUGH: usize = 4;

/// The task on this line where it is too slight to start an agent on without
/// asking, and nothing where the line stands on its own.
///
/// The task rather than the whole line: `m:opus fix` is three characters of
/// instruction behind seven of dials, and the instruction is what the agent is
/// given.
///
/// A command row is never asked about. The bang is not a keystroke anybody
/// leans on by accident, and `ls` is a command somebody means every bit as
/// much as a longer one.
pub fn slight(config: &Config, line: &str) -> Option<String> {
    let (turned, task) = turned(config, line).ok()?;
    if turned.exec {
        return None;
    }
    // Said back on one row, whatever it was typed on: the question quotes it,
    // and a newline in a line of prose is a row the footer does not have.
    let task = task.split_whitespace().collect::<Vec<_>>().join(" ");
    (!task.is_empty() && task.chars().count() < ENOUGH).then_some(task)
}

/// Start an agent on what was typed, where the view is — or run it, where the
/// line is a command row.
///
/// `under` is the project the line was opened in, where the wall was showing
/// one. It stands in for the view's own directory and gives way to a `d:`: the
/// cursor says where somebody was looking and the line says where they mean.
pub fn start(root: &Path, config: &Config, line: &str, under: Option<&Path>) -> Result<Started> {
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

    let here = std::env::current_dir().context("no working directory")?;
    // Where this one runs: what the line named, then the project it was opened
    // in, then where the view is. A relative path is still read against the
    // view's own directory whichever of them it lands in — what a name means at
    // a prompt is where the prompt is standing, and this line was typed at one.
    //
    // Answered before anything is made: a directory nothing is at is a line
    // somebody is still writing, not a spawn to clean up after.
    let dir = match &turned.dir {
        Some(said) => match aimed(said, &here) {
            Ok(dir) => dir,
            Err(refusal) => return Ok(Started::No(refusal)),
        },
        None => under.map_or(here, Path::to_path_buf),
    };
    // Who the vendor is asked to be, read against the directory this one runs
    // in: an agent it loads out of the project is an agent of the project the
    // line names, not of the one the view was opened in. A command row asks for
    // nobody — it runs a shell, and the mark is the shell's own to read.
    let (vendor_args, task) = match turned.exec {
        true => (Vec::new(), task),
        false => {
            let agent = turned.agent.clone().unwrap_or_else(|| config.agent.clone());
            as_agent(&agent, &task, &dir)
        }
    };

    // A `w:` is a decision about this agent, so it is made where the config's
    // own answer is made rather than argued with downstream: `new` has a flag
    // for going without a tree and none for insisting on one.
    let mut config = config.clone();
    if let Some(worktree) = turned.worktree {
        config.worktrees = worktree;
    }

    let dials = AgentArgs {
        command: turned.agent,
        model: turned.model,
        permission: turned.permission,
        effort: None,
    };
    let named = dials.command.is_some() || dials.model.is_some() || dials.permission.is_some();
    let args = NewArgs {
        task,
        name: None,
        dir: None,
        no_worktree: false,
        exec: turned.exec,
        agent: named.then_some(dials),
        vendor_args,
    };

    spawned(root, &dir, &config, &args, "started")
}

/// The vendor's own agent a task line is led with, as the argv that asks for
/// it, and the task with the word taken off.
///
/// The front of the line and nowhere else, the way the dials are read: a task
/// is aimed at one agent, and `@scout` in the middle of a sentence is the file
/// or the word it was typed as. One of the agents in the vendor's own places
/// and no other name, because the flag is handed to the vendor: a name it has
/// never heard of is a spawn that dies in its pane.
///
/// Nothing from a vendor that cannot be told to be one of its agents, which is
/// the same silence a word naming none of them gets. Both leave the line
/// whole, and the mark keeps whatever the vendor reads it as.
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

/// Where a `d:` points, read the way a shell prompt in `here` would read it,
/// or why it points nowhere.
///
/// Both halves of that reading are amx's here, because there is no shell on
/// this line to do either: a leading `~` is the home directory, and a name that
/// is not absolute is under the directory the view is running in.
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

/// Hand the spawn to the verb, and say what came of it in the one line the
/// view has room for.
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
    // What the verb wrote is the id and nothing else, which is what a shell
    // prompt gets from it too.
    let id = one_line(&started);
    Ok(Started::Yes {
        said: format!("{said} {id}"),
        id,
    })
}

/// What a reply came to.
pub enum Replied {
    /// It reached the agent, and this says what was done with it.
    Yes(String),
    /// Nothing was sent, and this says why.
    No(String),
}

/// Say something to the agent under the cursor.
///
/// The grammar is `amx answer`'s, because it is the question's rather than
/// amx's: the choices are read as choices wherever they are typed, the boxes
/// of a question that takes several are checked by naming them, and words are
/// an answer only at the prompts that offer a row to put them in. Words typed
/// at a permission box would land on whatever is highlighted, which is an
/// answer nobody chose, so the verb refuses them before a byte of them reaches
/// the pane — and refuses them here in the same words, because it is the same
/// reading of the same record.
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
        phase if phase.is_terminal() => Ok(Replied::No(format!(
            "{id} is {phase}; nothing is listening"
        ))),
        _ => {
            verbs::send::deliver(&agent, &server, &view.meta.pane, text)?;
            Ok(Replied::Yes(format!("sent to {id}")))
        }
    }
}

/// The card's one line, as the command line the verb reads.
///
/// The card holds a line and not a command line, so everything typed on it is
/// the answer — a key, the choices to check, or words of your own — and the
/// verb tells the three apart exactly as it does at a shell prompt.
///
/// The note is the exception, and the shape it belongs to is what makes room
/// for it. Measured against claude 2.1.240, a question whose choices carry a
/// preview draws a notes field and no free-text row at all, so on that one
/// question words are not an answer and there is nothing else for them to be:
/// the key in front of them is the choice, and what follows it is the note it
/// rides beside. Everywhere else the whole line goes to the verb, whatever is
/// in it, so a line the question would refuse is quoted back whole rather than
/// by its first word.
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

/// Whether a digit on this card is the answer itself rather than a character
/// on the line.
///
/// The vendor's own question, with its choices read and one of them all it
/// takes. There the number pressed is the whole answer — it is what the
/// vendor's own screen submits the moment it is typed — and an enter after it
/// would be a card asking somebody to confirm a choice they had already made.
/// What it costs is the answer that opens with a digit, which is typed after
/// any other character of it.
///
/// The three prompts it is not true of keep the line. A question that takes
/// more than one choice is answered by checking boxes, so a digit there is one
/// box named and the rest of the line still to come. A question whose choices
/// carry a preview takes a note after the key, and every note there begins
/// with the key it rides beside, so a digit sent on the press would put the
/// note out of reach altogether. And a permission box carries no payload:
/// what its numbers stand for is amx's reading of a picture of a pane, an
/// allowed tool call cannot be taken back, and a card that sent one on a
/// keystroke would be answering a screen it guessed the shape of.
pub fn picks(kind: Option<Kind>, options: &[String], asked: Option<&Ask>) -> bool {
    kind == Some(Kind::Question)
        && !options.is_empty()
        && !asked.is_some_and(|ask| ask.multi || ask.takes_notes())
}

/// What this question will take, in the words the card invites it with — and
/// the words it is refused in, which are the same words for the same reason.
///
/// Only what is true of the prompt in front of somebody, which is why the
/// question the call is showing comes into it: a question that takes more than
/// one choice is answered by checking boxes, and a question whose choices
/// carry a preview has a field for a note and no row for words of your own at
/// all. A permission box has neither, so neither is ever written there; a
/// question whose choices amx has not read yet does not name numbers it cannot
/// stand behind, and with no numbers there is nothing to check or to hang a
/// note on.
///
/// Where the numbers answer on the press they are named as doing it, out of
/// the same [`picks`] the keystroke is read by: a row that said `press 1-2`
/// wherever the digits went straight to the pane would be inviting an enter
/// that is never wanted.
pub fn invitation(kind: Option<Kind>, options: &[String], asked: Option<&Ask>) -> String {
    let numbers = match options.len() {
        0 => None,
        1 => Some("1".to_string()),
        many => Some(format!("1-{}", many.min(9))),
    };
    let choices = numbers.map(|numbers| match picks(kind, options, asked) {
        true => format!("{numbers} picks"),
        false => format!("press {numbers}"),
    });
    let Some(choices) = choices else {
        return match kind {
            Some(Kind::Question) => "type an answer".to_string(),
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
        // A question of a call, under a record that calls the screen something
        // else. `AskUserQuestion` is the only thing that writes a question
        // down, so a pending one is the vendor's own menu whatever an older amx
        // wrote over it — and a menu has no y and no n to offer. The choices
        // are all that is offered, because they are all this amx will send: the
        // verb reads words against the same word for the kind, and a line that
        // invited them here would be inviting what it is about to refuse.
        (_, Some(_)) => several,
        _ => format!("{several}, y or n"),
    }
}

/// Calling the agent under the cursor something else, which is the verb's.
///
/// `ctrl+r` and `amx rename` are one reading of one line: the same limit, the
/// same refusals in the same words, and the same record written the same way.
/// What differs is where the sentence lands, which is this file's whole errand.
pub use crate::verbs::rename::{Renamed, rename};

/// Write down that somebody has looked at this agent.
///
/// A look, like a name, is a fact about the wall rather than something the
/// agent said, so it goes in through the same door: the record's own clock
/// stays where it was, and a look that changes nothing writes nothing.
pub fn looked(root: &Path, id: &str) -> Result<()> {
    let agent = Agent::open(root, id)?;
    agent.writer()?.observe(|state| state.seen = store::now())?;
    Ok(())
}

/// Stop the agent under the cursor: the pane goes and the record stays.
///
/// The dispositions a person gets asked about at a shell prompt are taken
/// here as the defaults they already are: the worktree goes, the branch
/// stays, the record stays. Nothing that could lose work is decided by a
/// keystroke.
/// `forget` below is this file's own `--delete`, and it is a second
/// keystroke rather than part of this one: ending an agent and clearing its
/// row away are two decisions on the wall as well as at a prompt.
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

/// What forgetting one agent came to.
enum Forgotten {
    /// The record is gone, and the tree with it.
    Yes,
    /// Both are still here, because the tree holds work no commit has.
    Kept(PathBuf),
}

/// Forget an agent whose command has ended: its record, and the tree it was
/// given with it.
///
/// A tree holding work no commit has keeps both. Its record is where the
/// branch and the commit that tree was cut from are named, and a tree nothing
/// names is work nobody will find again.
fn forgetting(root: &Path, view: &View) -> Result<Forgotten> {
    let agent = Agent::open(root, view.id())?;

    if let Some(tree) = &view.meta.worktree
        && tree.exists()
    {
        if worktree::is_dirty(tree).unwrap_or(true) {
            return Ok(Forgotten::Kept(tree.clone()));
        }
        let repo = worktree::main_repo(tree).unwrap_or_else(|_| tree.clone());
        worktree::remove(&repo, tree)?;
    }

    agent.remove()?;
    Ok(Forgotten::Yes)
}

/// The same, as the line the view puts where its keys are.
///
/// How many presses it took to get here is the view's own business, and the
/// answer there is two whatever the row was doing: this door opens on the
/// second press of a row the first one armed — stopping it if it was live —
/// because nothing brings a record and the tree under it back.
pub fn forget(root: &Path, view: &View) -> Result<String> {
    Ok(match forgetting(root, view)? {
        Forgotten::Yes => format!("{} forgotten", view.id()),
        Forgotten::Kept(tree) => format!(
            "keeping {}: {} holds work no commit has",
            view.id(),
            tree.display()
        ),
    })
}

/// Forget all of these, and say what became of them.
///
/// One at a time and through the door a single ctrl+x uses, so a tree holding
/// work no commit has keeps its agent here exactly as it does there. That is
/// the whole safety of a key that clears a group: a sweep may only do what
/// somebody could have done row by row.
///
/// Nothing stops for a record that will not go. Somebody asked for the group
/// to be cleared, and one agent amx could not deal with is a line at the end
/// rather than a reason to leave the rest of them standing.
pub fn forget_all(root: &Path, views: &[&View]) -> Result<String> {
    let (mut gone, mut kept) = (0, 0);
    let mut trouble = Vec::new();
    for view in views {
        match forgetting(root, view) {
            Ok(Forgotten::Yes) => gone += 1,
            Ok(Forgotten::Kept(_)) => kept += 1,
            Err(e) => trouble.push(format!("{}: {e:#}", view.id())),
        }
    }

    let mut said = format!("forgot {gone}");
    if kept > 0 {
        said.push_str(&format!(" · kept {kept} holding work no commit has"));
    }
    // Raised rather than said, because part of what was asked for did not
    // happen: the view draws what it could not do louder than what it did.
    if !trouble.is_empty() {
        bail!("{said} · {} would not go: {}", trouble.len(), trouble[0]);
    }
    Ok(said)
}

/// What the agent has changed, for the card.
///
/// Taken once, when somebody asks, and held: re-running `git diff` on every
/// reading would put a repository's whole worth of work behind a clock tick.
pub fn changes(root: &Path, view: &View) -> Result<Card> {
    // The whole patch, not the summary: the closer look is where somebody
    // reads what was written, and the wall already says how much of it there
    // is.
    let mut patch = Vec::new();
    verbs::diff::run(root, view.id(), false, &mut patch)?;

    let patch = String::from_utf8_lossy(&patch).into_owned();
    Ok(Card {
        id: view.id().to_string(),
        phase: view.phase(),
        question: None,
        options: Vec::new(),
        kind: None,
        body: match patch.trim().is_empty() {
            true => "nothing changed yet".to_string(),
            false => patch,
        },
        changes: true,
        answer: false,
    })
}

/// What a verb wrote, as the one line the view has room for.
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
    use crate::store::Choice;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn a_line_says_what_it_is_aimed_at_before_anybody_types_into_it() {
        // A task is aimed at nobody yet, and it was opened where a task runs
        // anyway, so the rule over it has only its own word to say.
        assert_eq!(Composer::new(Asking::Task).about(), None);

        // Opened on the project axis it says where it will run, in the words
        // the heading it was read off is written in.
        let mut under = Composer::new(Asking::Task);
        under.under = Some(PathBuf::from("/src/api"));
        assert_eq!(under.about().as_deref(), Some("in /src/api"));

        // A reply stands at the foot of the card, under a rule already
        // carrying the name of the agent it is going to, so it has nothing of
        // its own to say and nothing draws this.
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

        // Four characters back, which is the r the o belongs in front of.
        for _ in 0..4 {
            line.left();
        }
        line.insert("o");
        assert_eq!(line.text, "port the importer");
        assert_eq!(line.at, 13, "and the cursor is after what was typed");

        // Counted in characters and not in bytes, because a character is what
        // somebody sees the block standing on.
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
        // Short enough to read on the line, so it lands as the characters it
        // is and the line has nothing standing for anything.
        let mut line = Composer::new(Asking::Task);
        line.insert("port ");
        line.paste("the importer\nand its tests");
        assert_eq!(line.text, "port the importer\nand its tests");
        assert!(line.pastes.is_empty());
        assert_eq!(line.whole(), line.text);

        // A row more than the line will hold is a row on the line instead,
        // with the cursor after it and the paste itself waiting beside.
        let mut line = Composer::new(Asking::Task);
        line.insert("what went wrong here: ");
        let log = "one\ntwo\nthree\nfour";
        line.paste(log);
        assert_eq!(line.text, "what went wrong here: [Pasted text #1]");
        assert_eq!(line.at, line.text.chars().count());
        assert_eq!(line.whole(), format!("what went wrong here: {log}"));

        // And so is one paragraph of more characters than a composer can show.
        // The second paste on the line is that line's second, and what is
        // typed between them is typed between them.
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

        // The bang is read off the whole of it, so a script pasted onto the
        // line is the command row it opens with.
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

        // The same text pasted again goes in where its marker was standing,
        // with the cursor after it — and what is sent is what was going to be
        // sent either way.
        let mut line = folded();
        line.home();
        line.paste(log);
        assert_eq!(line.text, format!("why: {log} then"));
        assert_eq!(line.at, "why: ".chars().count() + log.chars().count());
        assert_eq!(line.whole(), format!("why: {log} then"));

        // A marker taken back is not there to unfold, so the same text folds
        // afresh — on the next number, since the first is still what the paste
        // beside the line is read by.
        let mut line = folded();
        line.at = "why: [Pasted text #1]".chars().count();
        line.delete_back();
        line.paste(log);
        assert_eq!(line.text, "why: [Pasted text #2] then");
        assert_eq!(line.whole(), format!("why: {log} then"));

        // And a paste the line is not already holding folds the way it always
        // did.
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

        // Backspace against the end of the marker takes the marker, so the
        // line never holds a bracket `whole` cannot read and the paste is not
        // sent behind somebody's back.
        let mut line = folded();
        line.at = "why: [Pasted text #1]".chars().count();
        line.delete_back();
        assert_eq!(line.text, "why:  then");
        assert_eq!(line.whole(), "why:  then");

        // Delete against its front does the same.
        let mut line = folded();
        line.at = "why: ".chars().count();
        line.delete_forward();
        assert_eq!(line.text, "why:  then");

        // And ctrl+w, which would otherwise take the `#1]` and leave the rest.
        let mut line = folded();
        line.at = "why: [Pasted text #1]".chars().count();
        line.delete_word_back();
        assert_eq!(line.text, "why:  then");

        // The same characters typed by hand are characters, on a line holding
        // no paste for them to stand for.
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

        // A word is whatever whitespace is in the way and the run of
        // characters behind or in front of it.
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

        // The character behind the cursor and the one under it, wherever on
        // the line the cursor is standing.
        line.word_left();
        line.delete_back();
        assert_eq!((line.text.as_str(), line.at), ("port theimporter", 8));
        line.delete_forward();
        assert_eq!(
            (line.text.as_str(), line.at),
            ("port themporter", 8),
            "the one under it goes and the cursor stays where it was"
        );

        // Neither end of the line loses a character to a key pressed at it.
        line.home();
        line.delete_back();
        assert_eq!((line.text.as_str(), line.at), ("port themporter", 0));
        line.end();
        line.delete_forward();
        assert_eq!((line.text.as_str(), line.at), ("port themporter", 15));

        // A word is the one the cursor walks over a word at a time: the
        // whitespace behind it and the run of characters behind that, taken in
        // one edit.
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

        // Characters and not bytes, the same as everything else the cursor
        // does.
        let mut line = Composer::new(Asking::Task);
        line.insert("a é c");
        line.left();
        line.left();
        line.delete_back();
        assert_eq!((line.text.as_str(), line.at), ("a  c", 2));
    }

    #[test]
    fn a_line_names_itself_in_one_word_on_the_rule_over_it() {
        // Which of the four this is, in one word, with the agent it is aimed
        // at said beside it rather than in it.
        assert_eq!(Composer::new(Asking::Task).label(), "TASK");
        assert_eq!(
            Composer::new(Asking::Name {
                id: "fix-login-b2c".to_string(),
            })
            .label(),
            "RENAME"
        );
    }

    /// One question of a call, as the payload records one: `multi` is whether
    /// it takes more than one choice, and a preview on a choice is what turns
    /// the notes field on.
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

        // A question of the vendor's own offers choices and a field, and the
        // choices answer it on the press.
        assert_eq!(
            invitation(Some(Kind::Question), &two, None),
            "1-2 picks, or type an answer"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &one, None),
            "1 picks, or type an answer",
            "and one choice is one number"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &[], None),
            "type an answer",
            "and a menu whose choices amx has not read yet names none"
        );

        // One that takes more than one choice is answered by checking boxes,
        // and the line says how they are named.
        assert_eq!(
            invitation(Some(Kind::Question), &two, Some(&asked(true, false))),
            "press 1-2, 1,3 for several, or type an answer"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &[], Some(&asked(true, false))),
            "type an answer",
            "with no choices read there is nothing to check"
        );

        // And one whose choices carry a preview has a field for a note and no
        // row for words of your own at all, so the words on the line are the
        // note rather than an answer.
        assert_eq!(
            invitation(Some(Kind::Question), &two, Some(&asked(false, true))),
            "press 1-2, and words after it are a note"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &two, Some(&asked(true, true))),
            "press 1-2, 1,3 for several, and words after it are a note",
            "and a checkbox question can carry one too"
        );

        // A permission box and the trust screen read one key, so the card
        // never invites words at either.
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert_eq!(
                invitation(kind, &two, None),
                "press 1-2, y or n",
                "{kind:?}"
            );
            assert_eq!(invitation(kind, &one, None), "press 1, y or n", "{kind:?}");
            assert_eq!(
                invitation(kind, &[], None),
                "press y, n or 1-9",
                "with nothing read off the screen, the grammar itself: {kind:?}"
            );
        }

        // And a question of a call under a record that calls the screen
        // something else — an older amx wrote `permission` over every menu it
        // saw, and records outlive the amx that wrote them. The screen is the
        // menu the call drew, which has no y and no n on it, and the words the
        // record's own word for the kind would have the verb refuse are not
        // offered either.
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert_eq!(
                invitation(kind, &two, Some(&asked(false, false))),
                "press 1-2",
                "{kind:?}"
            );
            assert_eq!(
                invitation(kind, &two, Some(&asked(true, false))),
                "press 1-2, 1,3 for several",
                "{kind:?}"
            );
        }
    }

    #[test]
    fn card_reads_a_digit_as_the_answer_only_where_one_choice_is_the_answer() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        let question = |asked: Option<&Ask>| picks(Some(Kind::Question), &two, asked);

        // The vendor's own question with its choices read, whether the payload
        // behind it was read or not: one of them is the whole answer.
        assert!(question(None));
        assert!(question(Some(&asked(false, false))));

        // A question that takes several is answered by checking boxes, and one
        // whose choices carry a preview takes a note after the key — a note
        // begins with that key, so a digit that went on the press would be a
        // note nobody could type.
        assert!(!question(Some(&asked(true, false))));
        assert!(!question(Some(&asked(false, true))));

        // A menu whose choices amx has not read has no number to stand behind,
        // and a permission box is answered in a grammar amx read off a pane.
        assert!(!picks(Some(Kind::Question), &[], None));
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert!(!picks(kind, &two, None), "{kind:?}");
            assert!(!picks(kind, &two, Some(&asked(false, false))), "{kind:?}");
        }
    }

    #[test]
    fn card_hands_the_verb_the_whole_line_wherever_words_are_an_answer() {
        let line = |text: &str, asked: Option<&Ask>| {
            let args = card_line(text, asked);
            (args.key, args.note)
        };
        let plain = asked(false, false);

        // A key, the boxes to check and words of your own are one thing on the
        // card, because the verb tells them apart at a shell prompt too.
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

        // The question that draws a notes field is the one that has no row for
        // words, so what follows the key on that line is the note.
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
        // The one line that narrows is `/`. What the tokens are here is a task
        // with a colon in it, which the rule over the line says in the one
        // word it says about every task.
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        assert_eq!(composer.label(), "TASK");

        let (dials, task) = turned(&as_claude(), "s:waiting").unwrap();
        assert_eq!(dials, Turned::default());
        assert_eq!(task, "s:waiting", "and the whole of it is what is started");
    }

    /// A config whose vendor is the one the registry declares dials for.
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

        // An editor that edits: what it is given is what was on the line, and
        // what it leaves behind is the line afterwards.
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

        // `:cq` is how a person says they meant none of it, and what it comes
        // back as is a status.
        let Edited::No(why) = edited_in("false", &path, "port the importer").unwrap() else {
            panic!("an editor that refused took the line with it");
        };
        assert!(why.contains("false"), "{why}");
        assert!(!path.exists());
    }

    #[test]
    fn composer_a_leading_bang_makes_the_line_a_command_row() {
        // The mark leads the line and the rest of it is the command: what
        // `amx new --exec` is at a shell prompt, typed where the wall is.
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

        // The one dial it takes is where it runs, and the command is what is
        // left of the line.
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
        // Said in the words of the line and naming the one it does take. The
        // vendor's two and the vendor itself have no vendor here to be read
        // against, which is why `--exec` refuses them at a shell prompt; the
        // tree goes with them, because a command runs where it was typed.
        let refused = |line: &str| turned(&as_claude(), line).expect_err(line);

        for line in [
            "!m:opus cargo test",
            "!p:plan ls",
            "!agent:codex ls",
            "!w:on ls",
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

        // A word that is one of those anywhere but the front is the command's
        // own, the same law that keeps `port the m:opus importer` a task.
        let (dials, command) = turned(&as_claude(), "!echo m:opus").unwrap();
        assert!(dials.exec);
        assert_eq!(command, "echo m:opus");
    }

    #[test]
    fn composer_never_asks_about_a_command_row_however_short_it_is() {
        // The question is about a stray keystroke behind the key that opens
        // the line, and a bang is not one. `ls` is a command somebody means.
        assert_eq!(slight(&as_claude(), "!ls"), None);
        assert_eq!(slight(&as_claude(), "!d:/srv/app ls"), None);
    }

    #[test]
    fn composer_offers_the_cards_line_the_vendors_words_and_none_of_the_dials() {
        // The line at the foot of the card goes to an agent already running
        // under whatever it was started with, so the words that start one are
        // words of the message there: a dial typed at it is offered nothing,
        // where the task line reads the same word as a dial.
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

        // A path is read against the directory the line was handed, which is
        // the agent's own: there is no `d:` to read it against instead.
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
        // A shell reads `/etc` as a directory rather than as the front of a
        // skill's name, and `@src` there is a word it hands to `cat` rather
        // than one of the vendor's agents.
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
        let (dials, task) = turned(&as_claude(), "m:opus p:plan w:off port the importer").unwrap();
        assert_eq!(
            dials,
            Turned {
                exec: false,
                agent: None,
                model: Some("opus".to_string()),
                permission: Some("plan".to_string()),
                worktree: Some(false),
                dir: None,
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
        // The same law that keeps `s:waiting is what to check` a task: leading
        // tokens only, and everything from the first word that is not one.
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
    fn composer_refuses_a_directory_nothing_is_at_before_anything_is_made() {
        let root = TempDir::new().unwrap();
        let here = TempDir::new().unwrap();

        let Started::No(why) = start(
            root.path(),
            &Config::default(),
            "d:nowhere/at/all port the importer",
            None,
        )
        .unwrap() else {
            panic!("a spawn was aimed at a directory that is not there");
        };
        assert!(why.contains("nowhere/at/all"), "{why}");
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and nothing was made on the way to finding out"
        );

        // A path is read against the directory the view is running in, the way
        // a shell would read it.
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
        assert_eq!(refused("w:maybe port it"), "w:maybe: on or off");
        assert_eq!(refused("m: port it"), "m: takes a value");
        assert_eq!(refused("agent: port it"), "agent: takes a command");

        // Open dials take what the cycle never names, because `--model` does.
        let (dials, _) = turned(&as_claude(), "m:claude-fable-5 port it").unwrap();
        assert_eq!(dials.model.as_deref(), Some("claude-fable-5"));
    }

    #[test]
    fn composer_refuses_a_dial_the_agent_on_the_same_line_does_not_declare() {
        // The unregistered rule, said where somebody is standing: an agent amx
        // has no table for spawns exactly as it always did, and the dials it
        // never declared are refused by name rather than injected at it.
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
        // One of the agents in the vendor's own places, read in the project
        // this line's agent will run in: the word comes off the task and goes
        // to the vendor under the flag it declares for one.
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

        // The front of the line and nowhere else, and one of the vendor's own
        // agents and no other name. Everything else is the sentence it was
        // typed in, mark and all.
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

        // A vendor that cannot be told to be one of its agents leaves the word
        // where it was typed, whatever is in anybody's directories: the flag is
        // the vendor's own, and pi has none.
        for agent in ["pi", "mock-claude"] {
            assert_eq!(
                as_agent(agent, "@scout port the importer", project.path()),
                (Vec::new(), "@scout port the importer".to_string()),
                "{agent}, which is the same answer as a command amx has no \
                 entry for at all"
            );
        }
    }

    /// The words a suggestion offers, in the order it offers them.
    fn offered(suggest: &Suggest) -> Vec<&str> {
        suggest
            .entries
            .iter()
            .map(|entry| entry.spelled.as_str())
            .collect()
    }

    /// Somewhere for a project's own files to be, for the words that are not
    /// read out of any.
    fn a_project() -> &'static Path {
        Path::new("/srv/app")
    }

    #[test]
    fn composer_offers_the_vendors_a_line_can_be_aimed_at_by_name() {
        // The dial's own token, completed out of the table: every vendor amx
        // has an entry for, narrowed as the word is typed.
        let mut line = Composer::new(Asking::Task);
        line.insert("agent:");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("the table");
        assert_eq!(offered(&found), ["agent:claude", "agent:pi"]);
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

        // The two keys walk the list, and the ends of it are each other's
        // neighbours.
        line.choose(1);
        assert_eq!(line.suggest.as_ref().expect("the list").chosen, 1);
        line.choose(1);
        assert_eq!(
            line.suggest.as_ref().expect("the list").chosen,
            0,
            "past the last of them is the first"
        );
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
        // Back to the end of the word being typed, which is where somebody
        // mending one stands.
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

        // Spelled like one of them while the choice stands on another is
        // somebody choosing the other.
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
        // What fills the reading is counted rather than walked: the disk is
        // not what this is about.
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
        // A catalog put into the line by hand, naming a word no disk has:
        // what the keystrokes after it offer is read out of the line, not the
        // vendor's directories.
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

        // The line aimed at another vendor is another vendor's catalog, and
        // the reading goes with the first one.
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

    /// The words a reading holds, in the order it holds them.
    fn offered_by(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.spelled.as_str()).collect()
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
        // The vendor's own cycle, read out of the table rather than named
        // here: what the line offers is what the spawn would take, and a value
        // named twice is a value that stops being offered the day the vendor
        // renames it.
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

        // The tree is amx's own dial, so its two words are amx's own answer
        // and in no table.
        let mut line = Composer::new(Asking::Task);
        line.insert("w:");
        let found = suggest(&line, &as_claude(), a_project(), &[]).expect("on or off");
        assert_eq!(offered(&found), ["w:on", "w:off"]);

        // A dial the agent on this line does not declare has no values to
        // offer, which is the answer `turned` refuses the token with.
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };
        let mut line = Composer::new(Asking::Task);
        line.insert("m:");
        assert!(suggest(&line, &config, a_project(), &[]).is_none());
    }

    #[test]
    fn composer_offers_the_files_under_a_word_the_vendor_answers_to_with_none() {
        // `@` names one of the vendor's agents, and where it names none of
        // them it is the other thing the mark is for: a file of the project
        // the agent will run in. A word with a separator in it is no agent's
        // name, so this is that word every time.
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

        // Narrowed by what has been typed of the name, the same as every other
        // word the line offers.
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
        // Where the agent will run rather than where the view is: a file
        // offered out of anywhere else is a file that agent would not find.
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

        // Narrowed by what is typed of it, whichever of the two a word came
        // from.
        let mut line = Composer::new(Asking::Task);
        line.insert(&format!("d:{}/i", elsewhere.path().display()));
        let found = suggest(&line, &as_claude(), here.path(), &wall).expect("the one project");
        assert_eq!(offered(&found), [format!("d:{}", wall[0].display())]);
    }

    #[test]
    fn a_line_that_is_not_a_task_is_the_words_somebody_typed() {
        // Every other line goes to an agent that is already running or
        // narrows the wall, and what a vendor loads by name is what a task
        // line asks it for.
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

        // The same line sent twice running is one line to walk back over.
        sent.remember_line(&Asking::Task, "fix the login");
        assert_eq!(sent.lines_for(&Asking::Task).len(), 2, "not kept twice");
        // Sent again later, it is the newest again rather than a second copy
        // somewhere down the list.
        sent.remember_line(&Asking::Task, "port the importer");
        assert_eq!(
            sent.lines_for(&Asking::Task),
            ["port the importer", "fix the login", "port the importer"]
        );

        // Nothing but a task and a reply is a line sent.
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

        // Fifty, and the oldest is the one that goes.
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

        // The first step sets the line aside and puts the newest in its place,
        // the cursor at its end.
        assert!(composer.recall(&sent, true));
        assert_eq!((composer.text.as_str(), composer.at), ("fix the login", 13));
        assert!(composer.recall(&sent, true));
        assert_eq!(composer.text, "port the importer");
        // The oldest is where the walk back stops.
        assert!(!composer.recall(&sent, true), "nothing older to bring back");
        assert_eq!(composer.text, "port the importer");

        // Forward again, and the step past the newest is the draft, cursor
        // and all.
        assert!(composer.recall(&sent, false));
        assert_eq!(composer.text, "fix the login");
        assert!(composer.recall(&sent, false));
        assert_eq!((composer.text.as_str(), composer.at), ("half a", 5));
        assert!(
            !composer.recall(&sent, false),
            "there is nothing newer than the draft"
        );
        assert_eq!((composer.text.as_str(), composer.at), ("half a", 5));

        // A walk that began again starts from the newest, not where the last
        // one left off.
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

        // A recalled line was sent as characters and comes back as characters,
        // with no marker standing beside it for anything.
        assert!(composer.recall(&sent, true));
        assert_eq!(composer.text, "ship it");
        assert!(composer.pastes.is_empty());
        assert_eq!(composer.whole(), "ship it");

        // And the draft comes back with its paste still behind the marker.
        assert!(composer.recall(&sent, false));
        assert_eq!(composer.text, folded);
        assert_eq!(composer.whole(), long);

        // Nothing sent is nothing to bring back, and the line is left alone.
        let mut empty = Composer::new(Asking::Task);
        empty.insert("typed");
        assert!(!empty.recall(&[], true));
        assert_eq!(empty.text, "typed");
        assert!(!empty.recall(&[], false));
    }
}
