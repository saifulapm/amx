//! The agent view.
//!
//! One screen, held open on a terminal, answering the question somebody opens
//! it for: is anything waiting on me? The agents are gathered under what they
//! need, the cursor walks them, space floats a card over one of them, and
//! enter puts it in front of the terminal.
//!
//! The card is where an agent that has stopped is dealt with. It carries what
//! the agent is asking, the choices it is offering and a line to answer on, so
//! the row that said something needed doing is one keypress from the thing
//! that does it.
//!
//! What can be done from here is what can be done from a shell prompt — start
//! one, say something to one, stop one, see what one has changed — because a
//! person watching a wall of agents should not have to leave the screen that
//! told them something needed doing. Every one of those is a key, and every
//! key that types text puts the view in a mode that says so: a list whose keys
//! are also letters cannot have a composer that swallows them.
//!
//! Nothing here is in the byte path. What a card shows is a `capture-pane`,
//! and reaching an agent is tmux putting the agent's own session in front of
//! whoever is looking — the view is never torn down to do it, so coming back
//! is coming back to the screen they left.
//!
//! The reading is taken from disk on a clock rather than pushed at the view by
//! anything: nothing amx runs stays resident, so there is nobody to push.

mod act;
mod clip;
mod grid;
mod keyname;
mod paint;
// The list is the view's own, and one thing in it is not: what somebody
// pinned outlives the view they pinned it in, and the verb that takes an idle
// agent's pane has to obey it. See [`rows::Arrangement::from_disk`].
pub(crate) mod rows;

use anyhow::{Context, Result};
use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{EnterAlternateScreen, SetTitle, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::Backend;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::config::Config;
use crate::derive::{self, View};
use crate::store::{Agent, Phase, now};
use crate::theme::{Theme, Watch};
use crate::tmux::{PaneId, Server, SessionId};
use crate::vendor::Transcript;
use crate::verbs::interrupt::{self, Cut};
use crate::verbs::ls::Scope;
use crate::verbs::resume::Comeback;
use crate::{exit, models, registry, spawn, verbs};
use act::{Asking, Composer, Renamed, Replied, Started};
/// The editor door, for `amx new --edit`: the view and the command line open
/// the same one, so a task written in either place is read the same way.
pub use act::{Edited, edited};
use keyname::Bound;
use paint::{Body, Card, Hunk, Keymap, Notice};
use rows::{Arrangement, List, Narrow};

/// How often the agents are read again.
const REFRESH: Duration = Duration::from_millis(1000);

/// How long the view waits for a key before it goes round again.
const TICK: Duration = Duration::from_millis(120);

/// How long a frame of the working pulse lasts, which is the vendor's own
/// interval at 2.1.237.
const FRAME: Duration = Duration::from_millis(120);

/// How long a press leaves a finished row armed: long enough to read what the
/// row has started saying and press the key again, short enough that a key
/// pressed after that is a fresh decision rather than the end of an old one.
///
/// Five rather than the two it was until 2026-09-15, because `c` arms every
/// finished row on the wall and a wall of reasons is more than two seconds of
/// reading. One window for both keys: what makes a press the end of an old
/// decision is how long ago the last one was, and that is not a different
/// question for `ctrl+x`.
const ARMED: Duration = Duration::from_secs(5);

/// What arrived from the terminal.
enum Typed {
    /// Nobody typed anything in the time given.
    Nothing,
    Key(KeyEvent),
    /// The mouse, which the view holds for as long as it holds the screen.
    Mouse(MouseEvent),
    /// Text that arrived in one piece rather than a key at a time, which is a
    /// paste.
    Paste(String),
    /// There is nobody at this terminal any more.
    Gone,
}

/// Where the keys come from.
trait Keys {
    fn next(&mut self, patience: Duration) -> Typed;
}

/// Where the view says what the terminal it is drawing on should be called.
///
/// A window title is the one part of this screen somebody can read with the
/// window behind something else, so what goes on it is the one thing they would
/// have come back to the screen for: how many agents are waiting on them.
trait Titles {
    fn say(&mut self, said: &str);
}

/// The terminal itself.
struct Keyboard;

/// And its title bar.
struct TitleBar;

impl Titles for TitleBar {
    fn say(&mut self, said: &str) {
        // A terminal that will not take a title is a terminal with no title
        // bar, which is nothing for a view to report.
        let _ = execute!(std::io::stdout(), SetTitle(said));
    }
}

impl Keys for Keyboard {
    fn next(&mut self, patience: Duration) -> Typed {
        // A terminal that cannot be read is a terminal with nobody at it,
        // which is the same answer as somebody closing the view. So is a
        // signal to end, looked at on both sides of the wait.
        if ENDED.load(Ordering::Relaxed) {
            return Typed::Gone;
        }
        match event::poll(patience) {
            _ if ENDED.load(Ordering::Relaxed) => Typed::Gone,
            Ok(false) => Typed::Nothing,
            Err(_) => Typed::Gone,
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => Typed::Key(key),
                Ok(Event::Mouse(mouse)) => Typed::Mouse(mouse),
                Ok(Event::Paste(text)) => Typed::Paste(text),
                Ok(_) => Typed::Nothing,
                Err(_) => Typed::Gone,
            },
        }
    }
}

/// The keys, less a shade answer that arrived after the probe stopped
/// waiting for it: see [`crate::shade::Late`].
struct Unanswered<K> {
    keys: K,
    late: crate::shade::Late,
    /// What has been read and is still to be handed on, in order.
    ready: std::collections::VecDeque<Typed>,
}

/// How long a run that could be an answer waits for its next key. An answer
/// is sent in one write, so the rest of it is already there to be read.
const HOLD: Duration = Duration::from_millis(50);

impl<K: Keys> Unanswered<K> {
    fn new(keys: K) -> Self {
        Unanswered {
            keys,
            late: crate::shade::Late::default(),
            ready: std::collections::VecDeque::new(),
        }
    }
}

impl<K: Keys> Keys for Unanswered<K> {
    fn next(&mut self, patience: Duration) -> Typed {
        while self.ready.is_empty() {
            let holding = self.late.holding();
            let wait = match holding {
                true => patience.min(HOLD),
                false => patience,
            };
            match self.keys.next(wait) {
                Typed::Key(key) => self
                    .ready
                    .extend(self.late.hear(key).into_iter().map(Typed::Key)),
                Typed::Nothing if !holding => return Typed::Nothing,
                // Whatever is not a key ends the run, and what was held is
                // somebody's typing that came before it.
                other => {
                    self.ready
                        .extend(self.late.let_go().into_iter().map(Typed::Key));
                    if !matches!(other, Typed::Nothing) {
                        self.ready.push_back(other);
                    }
                }
            }
        }
        self.ready.pop_front().unwrap_or(Typed::Nothing)
    }
}

/// Whether the view goes round again.
enum Doing {
    Carry,
    Close,
    /// Lend the terminal to a tmux client on this agent's session, and go
    /// round again when it is handed back.
    Lend {
        id: String,
        on: Server,
        session: SessionId,
    },
    /// Lend it to an editor for as long as somebody is writing the line in it.
    Edit,
    /// And to whatever reads patches, for as long as somebody is reading this
    /// agent's.
    View {
        id: String,
    },
    /// And to a command of somebody's own, bound to a key in the config file
    /// and run on the agent the cursor is on.
    Bound {
        id: String,
        spelling: String,
        command: String,
    },
}

/// What the keys are doing at the moment.
#[derive(Default)]
enum Mode {
    /// Walking the agents.
    #[default]
    List,
    /// Typing a line: a task for a new agent, or a reply to one of them.
    Typing(Composer),
    /// The keys themselves, on the screen.
    Keys,
    /// A question of the view's own, waiting for the one key that answers it.
    Confirming(Asked),
}

/// What the view is waiting to be told.
///
/// One key answers it and every other key does not, which is the way round a
/// question has to be when a yes is the expensive answer: this one starts a
/// program.
enum Asked {
    /// A task barely long enough to be one, and the line it was typed on.
    /// `follow` is whether the key that asked was the one that goes with the
    /// agent, which the answer has to carry for it.
    Slight {
        task: String,
        line: Composer,
        follow: bool,
    },
}

impl Asked {
    /// The question itself, in the words the answer is given in.
    fn question(&self) -> String {
        match self {
            Asked::Slight { task, .. } => {
                format!("start an agent on \"{task}\"? y starts it · anything else keeps the line")
            }
        }
    }
}

/// What the card is showing.
#[derive(Default, PartialEq, Eq)]
enum Look {
    /// Nothing: nobody asked for a card.
    #[default]
    Away,
    /// The agent's own screen, taken again with every reading, with whatever
    /// it has stopped to ask over the top of it.
    Screen,
    /// What the agent has changed, as it stood when somebody asked.
    Changes,
}

/// What the next agent will be started with: the vendor, the dials that
/// vendor declares, where it will run, and the cap the fleet on the screen is
/// counted against.
///
/// The dials are prospective. Nothing in them says anything about the agents
/// already running: a dial is about the agent that does not exist yet, so
/// turning one touches none of the ones that do. The profile starts at the
/// config file every time the view opens and dies with it — a launcher that
/// drifted from the file because of what somebody pressed last Tuesday would
/// leave the file saying one thing and the screen another.
struct Profile {
    /// The vendor command a spawn runs, which is a command line rather than a
    /// program name because that is what the config key holds.
    agent: String,
    /// The command the config file asked for, which is where the vendor dial
    /// starts and what it comes back round to.
    configured: String,
    /// Where each vendor dial stands. [`registry::DEFAULT`] is the vendor's
    /// own behaviour, which amx says by passing no flag at all.
    model: String,
    /// The config this view was opened under, kept whole because the model
    /// dial reads a harness's list when the key is pressed rather than when
    /// the view opens — a harness that prints its models costs a process, and
    /// only the harness somebody turns the dial on should pay it.
    config: Config,
    permission: String,
    effort: String,
    /// Whether the next agent is cut a worktree of its own.
    worktree: bool,
    /// Where the next agent will run, as a person writes it.
    dir: String,
    /// What the agents on this screen are counted against, where there is a
    /// number to count them against.
    ///
    /// A view about one project counts that project's agents, and what a
    /// project runs at once is `max_agents` in its own file — the gate on the
    /// screen before it refuses a spawn. A view about every agent on the
    /// machine is not about any one project, and counting a machine against
    /// one project's cap would be reading a number against a fleet it says
    /// nothing about: what stands over all of it is `max_total`, and until
    /// somebody sets one there is nothing to say.
    cap: Option<usize>,
}

impl Default for Profile {
    /// What a view about one project opens at, under a config nobody has
    /// written.
    fn default() -> Profile {
        let config = Config::default();
        let cap = Some(config.max_agents);
        Profile::open(&config, cap, None, None)
    }
}

impl Profile {
    /// The profile a view opens at: config's own answer for every dial, and
    /// the directory the view is being run from.
    ///
    /// A dial config asked for that this vendor would not take rests at the
    /// sentinel instead, which is the second half of the law the config loader
    /// keeps: no entry, no dial, and no value amx would have to invent.
    ///
    /// The cap comes from the door rather than from the file, because which
    /// key it is depends on what the view was opened about and only the door
    /// knows that.
    fn open(
        config: &Config,
        cap: Option<usize>,
        dir: Option<&Path>,
        home: Option<&Path>,
    ) -> Profile {
        let entry = registry::entry(&config.agent);
        Profile {
            agent: config.agent.clone(),
            configured: config.agent.clone(),
            model: effective(entry.and_then(|e| e.model), config.model.as_deref()),
            config: config.clone(),
            permission: effective(
                entry.and_then(|e| e.permission),
                config.permission.as_deref(),
            ),
            effort: effective(entry.and_then(|e| e.effort), config.effort.as_deref()),
            worktree: config.worktrees,
            dir: dir.map(|dir| rows::shorten(dir, home)).unwrap_or_default(),
            cap,
        }
    }

    /// This vendor's model dial, where it declares one.
    fn model_dial(&self) -> Option<registry::DialSpec> {
        registry::entry(&self.agent)?.model
    }

    /// This vendor's permission dial, under the same rule.
    fn permission_dial(&self) -> Option<registry::DialSpec> {
        registry::entry(&self.agent)?.permission
    }

    /// And the dial for how hard it thinks, where the vendor has one.
    fn effort_dial(&self) -> Option<registry::DialSpec> {
        registry::entry(&self.agent)?.effort
    }

    /// What the vendor dial offers: the command the config file asked for,
    /// and every vendor amx has an entry for beside it.
    ///
    /// The file's own command comes first and is never dropped, because it is
    /// the one value the dial could not work out for itself: `agent` is a
    /// command line, arguments and all, and a cycle that turned off it would
    /// leave nothing on the screen able to say what the file said.
    fn vendors(&self) -> Vec<&str> {
        let configured = registry::program(&self.configured);
        std::iter::once(self.configured.as_str())
            .chain(
                registry::entries()
                    .iter()
                    .map(|entry| entry.name)
                    .filter(|name| *name != configured),
            )
            .collect()
    }

    /// The next vendor, with the dials it declares under it.
    ///
    /// The model goes back to the sentinel, because a model belongs to the
    /// harness that runs it: somebody who has turned to another harness has
    /// asked for that harness, not for it to be handed a model chosen for the
    /// one before. The other dials rest at the sentinel where the new vendor
    /// will not take them, which is the law the profile opens on read a second
    /// time: the row must not name a value to a vendor that would refuse it.
    fn cycle_vendor(&mut self) {
        let Some(next) = next_in(&self.vendors(), &self.agent) else {
            return;
        };
        self.agent = next;
        self.model = registry::DEFAULT.to_string();
        self.permission = effective(self.permission_dial(), Some(&self.permission));
        self.effort = effective(self.effort_dial(), Some(&self.effort));
    }

    /// The next model the harness on the row runs, and nothing else.
    ///
    /// A model belongs to the harness that runs it, so this dial walks the
    /// list of the harness the row already names and never leaves it. Turning
    /// to another harness is the vendor key's job, which settles the dials
    /// under it the way a turned vendor settles them. The sentinel is nobody's
    /// model — the word for passing none — so it sits at both ends of the
    /// walk.
    ///
    /// The list is read here rather than when the view opened, so a harness
    /// that prints its models pays for that process only when somebody turns
    /// the dial on it; [`models::models_of`] keeps what it read for an hour.
    ///
    /// A harness amx has no entry for, or one that declares no model dial,
    /// has nothing to offer, so its key does nothing rather than inventing a
    /// value that vendor would refuse.
    fn cycle_model(&mut self) {
        let Some(vendor) = registry::entry(&self.agent) else {
            return;
        };
        if vendor.model.is_none() {
            return;
        }
        let list = models::models_of(vendor, &self.config);
        let at = list.iter().position(|model| *model == self.model);
        // Past the last model is the sentinel, and so is a model no list names:
        // the dial is what the key offers, and it always begins where it began.
        let next = at.map_or(list.first(), |at| list.get(at + 1));
        self.model = next
            .cloned()
            .unwrap_or_else(|| registry::DEFAULT.to_string());
    }

    fn cycle_permission(&mut self) {
        if let Some(next) = self
            .permission_dial()
            .and_then(|d| next_in(d.cycle, &self.permission))
        {
            self.permission = next;
        }
    }

    fn cycle_effort(&mut self) {
        if let Some(next) = self
            .effort_dial()
            .and_then(|d| next_in(d.cycle, &self.effort))
        {
            self.effort = next;
        }
    }

    fn toggle_worktree(&mut self) {
        self.worktree = !self.worktree;
    }

    /// The config a spawn from this view is made under: the file's own answer
    /// with every dial the header is showing written over it.
    ///
    /// A config rather than a set of flags, and that is what settles the
    /// precedence: `new` reads a flag first and the file second, and the
    /// tokens a task line is led with are the flags. So the header is what
    /// this view spawns at — the worktree dial included, whichever way the
    /// file had it — and a token on the line beats it for the one spawn it
    /// leads, without either of them having to know about the other.
    fn launching(&self, config: &Config) -> Config {
        Config {
            agent: self.agent.clone(),
            worktrees: self.worktree,
            model: turned_to(&self.model),
            permission: turned_to(&self.permission),
            effort: turned_to(&self.effort),
            ..config.clone()
        }
    }
}

/// A dial as the config states one: the sentinel is the vendor's own
/// behaviour, which is said by holding nothing rather than by holding a word.
fn turned_to(value: &str) -> Option<String> {
    (value != registry::DEFAULT).then(|| value.to_string())
}

/// Where a dial rests for a vendor: what config asked for if this vendor takes
/// it, and the sentinel — no flag at all — otherwise.
fn effective(dial: Option<registry::DialSpec>, configured: Option<&str>) -> String {
    match (dial, configured) {
        (Some(spec), Some(value)) if registry::accepts(&spec, value) => value.to_string(),
        _ => registry::DEFAULT.to_string(),
    }
}

/// Which of the two chords a key arrived under, if either.
///
/// Shift is not one of them. A terminal says shift by sending the character it
/// typed, so asking about it would be asking about the keyboard rather than
/// about the key. The one key it can be asked about is tab, which has no
/// character of its own to arrive as.
fn chord(key: KeyEvent) -> KeyModifiers {
    key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Whether the line at the foot of the card has a use for this key.
///
/// What it has a use for is what it takes text with, what moves along it and
/// what ends it. Everything else — the letters and arrows that walk the wall,
/// the keys that page the card, the chords that act on an agent — is the
/// list's while the line stands there, because a card open on an agent is a
/// card somebody is reading a list against.
///
/// The four control chords are the ones a terminal line has read since before
/// it had arrows, plus the one that takes the line to an editor and the one a
/// keyboard with no shift+enter breaks a line with. And the two arrows that
/// walk the words offered under the line, for exactly as long as there are
/// words offered: with none, they walk the wall. Held with alt they walk the
/// lines sent before, words offered or not.
fn the_lines(composer: &Composer, key: KeyEvent) -> bool {
    let plain = chord(key).is_empty();
    let ctrl = chord(key) == KeyModifiers::CONTROL;
    match key.code {
        KeyCode::Char('a' | 'e' | 'w' | 'g' | 'j') if ctrl => true,
        // A character typed plain or with shift held. One reached for with
        // control or alt is somebody reaching past the line.
        KeyCode::Char(_) => plain,
        KeyCode::Enter | KeyCode::Esc | KeyCode::Tab => true,
        KeyCode::Backspace => plain || chord(key) == KeyModifiers::ALT,
        KeyCode::Delete | KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => true,
        KeyCode::Up | KeyCode::Down => {
            (plain && composer.suggest.is_some()) || chord(key) == KeyModifiers::ALT
        }
        _ => false,
    }
}

/// And whether an empty one leaves this key to the list after all.
///
/// Two keys, read before the rule above: with nothing typed there is no line
/// for a space to stand in and nothing for enter to send, so both of them are
/// the wall's — space closes the card, which is the key that opened it, and
/// enter does what enter does on the line under the cursor. The first
/// character typed takes them back, because a message with two words in it is
/// a message somebody has to be able to write.
///
/// An enter carrying shift is not one of them: that is the newline every line
/// in the view breaks a paragraph with, and an empty line is exactly where
/// somebody writing one starts. The alt and control newlines never reach here
/// at all — a key held down with either is somebody reaching past the line.
fn the_lists_on_an_empty_line(key: KeyEvent) -> bool {
    if !chord(key).is_empty() {
        return false;
    }
    match key.code {
        KeyCode::Char(' ') => true,
        KeyCode::Enter => !key.modifiers.contains(KeyModifiers::SHIFT),
        _ => false,
    }
}

/// The next value a cycle offers. A value the cycle never names — a full model
/// name out of config, say — starts the cycle over rather than ending it: the
/// cycle is what the key offers, and it always begins at the sentinel.
fn next_in(cycle: &[&str], now: &str) -> Option<String> {
    match cycle.iter().position(|value| *value == now) {
        Some(at) => cycle.get((at + 1) % cycle.len()),
        None => cycle.first(),
    }
    .map(|value| value.to_string())
}

/// What a press has armed and a second one would forget: one row, or every
/// row under a heading, and the moment it was armed.
///
/// Held by id rather than by where the cursor was: the wall is read again
/// every second and the rows move under it, and a window that belonged to a
/// place on the screen would arm whatever had come to rest there. Whether the
/// press was on a heading is kept the same way, and not which heading:
/// stopping a group moves it to completed by the next reading, so the heading
/// that was pressed can be gone while its rows are still armed.
struct Arm {
    /// The rows the second press forgets.
    ids: Vec<String>,
    /// Whether the first press was on a heading, which is where the press
    /// that forgets them all has to land again.
    swept: bool,
    /// Which heading that was, in terms that outlive the next reading — or
    /// the heading standing over the rows once that one has dissolved, which
    /// [`Screen::keep_the_sweep`] moves it to. A press on any other heading
    /// is a first press of its own, whatever rows the two share.
    heading: Option<rows::Key>,
    /// Whether it was `c` that armed them, which is a press about everything
    /// on the wall that has finished rather than about the row the cursor is
    /// on. The two kinds do not answer each other's second press: a `ctrl+x`
    /// into this window is somebody reaching for the other key, not agreeing
    /// to this one.
    cleared: bool,
    /// Why each of those rows is on the list, in the order `ids` are in, where
    /// the press that armed them had a reason to give. Empty for the arm
    /// `ctrl+x` leaves: a row somebody put the cursor on needs no reason.
    why: Vec<String>,
    /// Which of `ids` have a tree holding work no commit has, asked once on the
    /// press that armed them. The second press passes those rows by, so the
    /// first press says so where a person is already reading: a row promised
    /// `c again clears` and then kept back is the view saying something that
    /// was not true.
    held: Vec<String>,
    at: Instant,
}

/// Where a card came from, and what would say it is out of date.
///
/// Taking a card is the most expensive thing the view does: an agent's whole
/// transcript read off disk and drawn into rows, or a `capture-pane` fork.
/// Most passes it comes to the same card, because nothing has written to the
/// files behind it since the last one — so what a card is read from is kept
/// with it, and two stats decide whether the reading is worth taking again.
enum Freshness {
    /// The files the card was read from, each with the length and the moment
    /// it was last written that it had when it was read. `None` for a file
    /// that was not there: a vendor announces its transcript before it writes
    /// one, and a file that appears is a file that moved.
    Files(Vec<(PathBuf, Option<(u64, SystemTime)>)>),
    /// A capture of a pane, or a question. Nothing on disk says whether a
    /// vendor has redrawn its pane, and a question is what the view exists to
    /// keep true, so both are taken again every time they are asked for.
    Pane,
}

impl Freshness {
    /// Whether anything the card was read from has been written since.
    fn moved(&self) -> bool {
        match self {
            Freshness::Pane => true,
            Freshness::Files(files) => files.iter().any(|(path, at)| stamped(path) != *at),
        }
    }
}

/// What a file was when a card read it: how long it is and when it was last
/// written. `None` where there is no file to ask about.
fn stamped(path: &Path) -> Option<(u64, SystemTime)> {
    let file = std::fs::metadata(path).ok()?;
    Some((file.len(), file.modified().ok()?))
}

/// A file about to be read, with what it stands at now.
///
/// Asked before the read rather than after it, because a file written in
/// between belongs to the reading that comes next: a card that recorded the
/// newer pair would stand on words the file no longer holds.
fn as_read(path: PathBuf) -> (PathBuf, Option<(u64, SystemTime)>) {
    let at = stamped(&path);
    (path, at)
}

/// What the card on the screen was taken of, which is what says whether taking
/// it again would come to the same card.
///
/// The agent it is about and what the record said it was doing, because a card
/// is a picture of one reading of one agent; the width it was wrapped for,
/// because a terminal somebody resized wraps the same words differently; and
/// where its body was read from.
struct Taken {
    id: String,
    phase: Phase,
    question: Option<String>,
    width: u16,
    fresh: Freshness,
}

/// The view as it stands: what was read, where the cursor is, what the keys
/// are doing, and what the view last had to say for itself.
#[derive(Default)]
struct Screen {
    /// Where every agent's record is kept, which is where a card reads what
    /// its agent is saying at this moment.
    root: PathBuf,
    list: List,
    /// Where the view stands: the directory it was opened about, or the one
    /// it was run from. A task typed at the view starts here unless its line
    /// or the cursor says otherwise.
    standing: PathBuf,
    /// What the next agent will be started with.
    profile: Profile,
    /// The colours it is all painted in, which the paint asks by role and
    /// never decides for itself.
    theme: Theme,
    /// What reading the theme last had to say for itself, so a file that has
    /// been put right takes its own complaint off the screen with it.
    complained: Option<String>,
    mode: Mode,
    look: Look,
    /// Whether the rows say what runs them — the vendor, the model and the
    /// effort — in a column of their own. Off until somebody asks for it: a
    /// fleet on one vendor and one model is a column of the same word, and the
    /// room it takes is the summary's.
    vendor: bool,
    card: Option<Card<Body>>,
    /// What that card was taken of, where the view took it. Kept beside the
    /// card rather than on it, so that a card built anywhere else — a patch is
    /// — is one the next pass takes again.
    taken: Option<Taken>,
    /// The transcript behind the card, as far as it has been read.
    heard: Heard,
    /// How far the card's body has been paged from its natural edge, and how
    /// far one page is. The paint owns the clamp: only it knows the rows the
    /// body was given.
    scroll: paint::Scroll,
    /// Where the window over the list stands, and whether it owes the cursor a
    /// move. The paint owns this clamp too, for the same reason: how far the
    /// window can be scrolled is a question about the band it is drawn in.
    wall: paint::WallScroll,
    /// How far down the keys the overlay stands, and what somebody is looking
    /// for on it. The paint owns the clamp here too, for the same reason: only
    /// it knows how many rows a screen this shape gave them.
    keymap: Keymap,
    /// The keys somebody bound in the config file, each against the command it
    /// runs. Read when the view opens and not again: the file is a person's
    /// standing answer, and a key that moved under their hands while they were
    /// looking at the screen would be a key they never pressed.
    bound: Vec<Bound>,
    notice: Option<Notice>,
    /// Where the last frame put things, which is what a mouse position is
    /// read against.
    map: paint::Map,
    /// The line of the list the pointer is resting on, when it is resting on
    /// an agent's.
    hover: Option<usize>,
    /// Where a left button went down, while it is still down: what a drag is
    /// measured from, and what tells a click apart from the start of one.
    pressed_at: Option<(u16, u16)>,
    /// The two ends of the cells a hand is dragging over, press first. Held
    /// only while the button is: the release copies them and lets them go,
    /// because a selection left on the screen would be the view keeping a
    /// mark for something already done with.
    selection: Option<((u16, u16), (u16, u16))>,
    /// The finished row a press has armed, where one is armed.
    arm: Option<Arm>,
    /// When the agents were last read.
    read: Option<Instant>,
    /// Where what somebody arranges is kept, where there is anywhere to keep
    /// it.
    remembering: Option<PathBuf>,
    /// What this view last knew the file to hold, and how the file looked
    /// when that was read or written. A change another view makes shows by
    /// the stamp moving; a write this view makes is merged against what is on
    /// disk rather than dropped over it.
    published: Arrangement,
    viewed_at: Option<(u64, SystemTime)>,
    /// Whether a `g` is standing there waiting for the second one that would
    /// make it a move to the top.
    ///
    /// Taken at the head of every press, so the key that is not that second
    /// `g` clears it by arriving and then does its own job whole. No window on
    /// it, unlike the arm a `ctrl+x` leaves: that one is timed because the
    /// press inside it destroys something, and this one only moves a cursor.
    going: bool,
    /// The agent somebody last went into from this screen, where they have
    /// gone into one: the terminal lent to it outside tmux, or the client
    /// switched to it inside, which is the same door from the other side.
    ///
    /// Kept for the frame rather than written down, like the arm and the
    /// pulse: it is about the person at this screen and what they were just
    /// looking at, and a mark that outlived the view would be telling them
    /// about a session they left yesterday.
    lent: Option<String>,
    /// The projects the agents on the wall run in, once each and in order.
    /// Kept with the reading rather than worked out where a line is typed: a
    /// `d:` is offered them on every keystroke, and the wall behind the line is
    /// a second old whatever the line is doing.
    projects: Vec<PathBuf>,
    /// The agent the line last started, until the cursor has been put on it.
    ///
    /// Kept for the one reading after the start, because that is the first
    /// frame the agent has a row on: the wall behind the line was read before
    /// it existed, and a cursor cannot be moved to a line nobody has drawn.
    started: Option<String>,
    /// Which frame of the working pulse the rows are on.
    beat: usize,
    /// When that frame came up.
    stepped: Option<Instant>,
    /// The lines this view has sent, tasks and replies apart, for a line
    /// being typed to walk back over.
    sent: act::Backlog,
}

/// Open the view on this terminal and hold it until somebody closes it.
///
/// The terminal is asked to bracket pastes for as long as the view holds it.
/// Without that a pasted task arrives as the keys it is made of, and the first
/// newline in it is an enter: a truncated task dispatched, and the rest of the
/// lines queued up to dispatch themselves after it. It is the same law amx has
/// always sent text to an agent under, facing the other way.
///
/// It is asked for one thing more: to tell apart the keys it otherwise sends
/// the same bytes for, which is what a terminal that speaks the kitty
/// keyboard protocol does when the disambiguation is pushed at it. That is
/// what makes shift+enter a key of its own rather than an enter. A terminal
/// that does not speak it ignores the asking and goes on sending enter, so
/// nothing is owed to it and nothing is lost.
///
/// `cap` is what the counts on the header are read against, which the front
/// door decides: see [`Profile::cap`].
pub fn run(root: &Path, config: &Config, scope: &Scope, cap: Option<usize>) -> Result<i32> {
    let mut terminal = ratatui::try_init().context("taking the terminal")?;
    // Made before anything else is asked of the terminal, so that whatever
    // the view goes on to ask for is given back however it ends.
    let held = Held;
    // A signal to end is a close like any other, heard by the loop between
    // keys, so the guard above is what gives the terminal back.
    hear_the_end();
    // A terminal that declines is one amx cannot tell a paste from typing on,
    // which is what the composer did before it asked at all.
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    // And the mouse, for as long as the view holds the screen: the list
    // takes it, and shift is the terminal's own selection the whole time.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);

    // And the shift on an enter, which a terminal has no way of sending until
    // it is asked to tell the modified keys apart.
    let _ = execute!(std::io::stdout(), TELL_THE_KEYS_APART);

    // The title the terminal came with, kept while the view has its own to
    // say, and put back below. A view that renamed somebody's window and left
    // it renamed would be leaving a count that stopped being true behind it.
    let _ = execute!(std::io::stdout(), Print(KEEP_THE_TITLE));

    let remembering = crate::paths::view_file(root);
    let outcome = watch(
        root,
        config,
        cap,
        scope,
        &mut terminal,
        &mut Unanswered::new(Keyboard),
        Here::read(),
        remembering.as_deref(),
        &mut TitleBar,
        Painting::of(&config.theme, crate::shade::asked, root),
    );

    drop(held);

    // Said onto the screen the view has just handed back, where there is
    // room for it and nothing to answer. Not over a view that failed: what
    // went wrong is the thing to read then.
    if outcome.is_ok()
        && let Some(offer) = remembering.and_then(|path| offer_the_statusline(&path))
    {
        println!("{offer}");
    }
    outcome
}

/// What gives the terminal back the way the view found it, on return, on a
/// panic unwinding through [`run`], and on a signal the loop hears.
///
/// Everything is given back whether or not asking for it took: a terminal
/// told to stop doing something it never started does nothing.
struct Held;

impl Drop for Held {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(std::io::stdout(), Print(PUT_THE_TITLE_BACK));
        // Then emptied, for a terminal with no title to put back: tmux keeps
        // none, and without this a pane goes on being called what the view
        // last counted.
        let _ = execute!(std::io::stdout(), SetTitle(""));
        // Not `restore`, which says its failure on stderr: with the terminal
        // gone that fails too, the saying panics, and the panic hook's own
        // restore panics again into an abort.
        let _ = ratatui::try_restore();
    }
}

/// Whether a SIGTERM or a SIGHUP has come for the view.
static ENDED: AtomicBool = AtomicBool::new(false);

extern "C" fn heard_the_end(_: nix::libc::c_int) {
    ENDED.store(true, Ordering::Relaxed);
}

/// Have SIGTERM and SIGHUP end the view the way closing it does, rather than
/// end the process with the terminal still in the view's modes.
///
/// A handler that only sets a flag, because nothing else is safe to do inside
/// one; the loop reads it between keys, at most a patience later.
fn hear_the_end() {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
    let action = SigAction::new(
        SigHandler::Handler(heard_the_end),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    for signal in [Signal::SIGTERM, Signal::SIGHUP] {
        // SAFETY: the handler touches nothing but an atomic.
        let _ = unsafe { sigaction(signal, &action) };
    }
    watch_the_terminal();
}

/// Have a terminal that goes end the view, wherever in the loop it is.
///
/// A hung-up terminal reads as an end of file or an error for ever after, and
/// crossterm reads it again at once on either without returning, so a view
/// waiting on a key when its pane went would spin and never hear the hangup.
/// This thread waits on nothing but the hangup, asks the loop to close, and
/// ends the process itself if the loop has not a moment later: there is no
/// terminal left to give anything back to.
fn watch_the_terminal() {
    use std::io::IsTerminal;
    use std::os::fd::IntoRawFd;
    // The terminal crossterm reads keys from, found the way it finds it.
    let fd = match std::io::stdin().is_terminal() {
        true => nix::libc::STDIN_FILENO,
        false => match std::fs::File::open("/dev/tty") {
            Ok(tty) => tty.into_raw_fd(),
            Err(_) => return,
        },
    };
    std::thread::spawn(move || {
        // Asking for no events still hears a hangup, an error or a closed
        // descriptor, which are always reported.
        let mut watched = nix::libc::pollfd {
            fd,
            events: 0,
            revents: 0,
        };
        // SAFETY: one pollfd, alive for the whole call.
        while unsafe { nix::libc::poll(&mut watched, 1, -1) } < 0 {
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return;
            }
        }
        ENDED.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(1));
        std::process::exit(exit::OK);
    });
}

/// What the view opens painted in: the palette the config named, whatever
/// reading it had to say for itself, and the file to watch for somebody
/// editing it.
///
/// Read where the view is opened rather than inside the loop, because a theme
/// lives beside the config file rather than under the root the view was
/// pointed at: everything the loop reads is somewhere it was handed, so a test
/// drives it with a palette of its own and never goes looking on the machine.
#[derive(Default)]
struct Painting {
    theme: Theme,
    /// What was wrong with the file, for the view to say once.
    warnings: Vec<String>,
    /// What the loop stats to notice the file being edited under it.
    watching: Watch,
}

impl Painting {
    /// The theme of that name, wherever this machine keeps its themes.
    ///
    /// `ask` is the terminal being asked its background, called here, which is
    /// after the terminal has been taken into raw mode and before the loop has
    /// read a key off it, because the answer arrives on stdin and anywhere
    /// else it would land in the middle of somebody's typing.
    fn of(named: &str, ask: impl FnOnce() -> Option<String>, state_root: &Path) -> Painting {
        let named = Painting::named(named, ask, state_root);
        // Stamped before the read rather than after it, so that an edit
        // landing between the two is one reread rather than a palette the
        // view holds until the next edit. `Watch` says why.
        let watching = Watch::of(named);
        let (theme, warnings) = crate::theme::load(named);
        Painting {
            theme,
            warnings,
            watching,
        }
    }

    /// The name of the palette to paint with, having asked the terminal its
    /// foreground and background once whatever the theme, and kept the
    /// colours it answered for the panes amx starts.
    ///
    /// `auto` is not a name on disk: it is that same answer read for a shade,
    /// and one of the two palettes amx ships — see [`crate::theme::AUTO`]. Any
    /// other name is itself. No answer keeps nothing, and leaves whatever an
    /// earlier view kept.
    fn named<'a>(
        named: &'a str,
        ask: impl FnOnce() -> Option<String>,
        state_root: &Path,
    ) -> &'a str {
        let answer = ask();
        let (foreground, background) = answer
            .as_deref()
            .map(crate::shade::colours_of)
            .unwrap_or_default();
        if let Some(background) = background {
            // A colour not kept is a pane painted the way it was before, which
            // is no reason to keep somebody from their view.
            let _ = crate::shade::remember(state_root, foreground, background);
        }
        crate::theme::chosen(named, || crate::shade::of_the_answer(answer.as_deref()))
    }
}

/// What a terminal is asked with to hold on to the title it is wearing, and to
/// put it back on again.
///
/// xterm's own pair for it, which the terminals amx is drawn on speak and the
/// ones that do not ignore: an escape a terminal has never heard of costs
/// nothing, and the worst of it is a window left called `amx`.
const KEEP_THE_TITLE: &str = "\x1b[22;2t";
const PUT_THE_TITLE_BACK: &str = "\x1b[23;2t";

/// What a terminal is asked with to tell apart the keys it otherwise sends the
/// same bytes for.
///
/// The least of the kitty keyboard protocol: enough for shift+enter to arrive
/// as an enter with a shift on it, and not the event kinds or the released
/// keys, which would be a second event for every press the view already reads
/// one of. Asked for by pushing it and given back by popping it, so a terminal
/// somebody had already set up for something else is left the way it was.
const TELL_THE_KEYS_APART: PushKeyboardEnhancementFlags =
    PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES);

/// The line somebody pastes, which is the verb amx already has with a clock
/// beside it.
const STATUSLINE: &str = "set -g status-right '#(amx statusline) | %H:%M'";

/// What the view has to say the first time somebody closes it, if anything.
///
/// An offer and not an install. Where the counts would be useful is a corner
/// of a terminal somebody was already using, which is a file of theirs, and
/// nothing in amx writes it: the line is printed for them to paste, once,
/// because a suggestion that comes back at every quit is an advertisement.
fn offer_the_statusline(path: &Path) -> Option<String> {
    let mut remembered = Remembered::read(path);
    if remembered.statusline {
        return None;
    }

    // A file that will not be written costs the offer again next time, which
    // is a better failure than an error over a screen that is already gone.
    remembered.statusline = true;
    let _ = remembered.write(path);

    Some(format!(
        "amx can keep the fleet in the corner of your tmux status line:\n\n    \
         {STATUSLINE}\n\nPaste that into your tmux config. amx will not write \
         it for you."
    ))
}

/// What the view remembers between runs. Every field defaults, so a file
/// written by an older amx still reads.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Remembered {
    /// Whether the status line has been offered.
    statusline: bool,
    /// How somebody arranged the list, as the list itself states it.
    arrangement: Arrangement,
    /// Whether the rows were left saying what runs them.
    vendor: bool,
    /// The lines the view has sent, for a later line to bring back.
    sent: act::Backlog,
}

impl Remembered {
    /// What the file says, and nothing where it says nothing: this is the
    /// view's own convenience, and a view that would not open because a
    /// half-written file could not be parsed would be a poor trade.
    fn read(path: &Path) -> Remembered {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// The whole document, so a second view reading it while this one writes
    /// sees what was there or all of this.
    fn write(&self, path: &Path) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self).context("writing what the view keeps")?;
        bytes.push(b'\n');
        crate::store::write_atomic(path, &bytes)
    }
}

/// The view itself: draw what is there, act on what is typed, read again.
///
/// `remembering` is where what somebody arranges is kept, where there is
/// anywhere to keep it. A view with nowhere still arranges itself; it just
/// does not outlive the run.
#[allow(clippy::too_many_arguments)]
fn watch<B>(
    root: &Path,
    config: &Config,
    cap: Option<usize>,
    scope: &Scope,
    terminal: &mut Terminal<B>,
    keys: &mut impl Keys,
    here: Option<Here>,
    remembering: Option<&Path>,
    titles: &mut impl Titles,
    painting: Painting,
) -> Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    // What the next agent will be started with is read once, from the config
    // this run was given and the directory the view stands in: neither moves
    // while somebody is looking at the screen, and a dial they turn is theirs
    // until they close it. A view opened about a directory stands there, the
    // way a view run from one does.
    let standing = match scope.under() {
        Some(under) => under.to_path_buf(),
        None => std::env::current_dir().context("no working directory")?,
    };
    // The keys the file binds, read here because this is where the config is:
    // a spelling is turned into a key once, and what the view holds afterwards
    // is keys rather than words.
    let (bound, refused) = keyname::bound_by(&config.keys);
    let mut screen = Screen {
        root: root.to_path_buf(),
        bound,
        profile: Profile::open(
            config,
            cap,
            Some(&standing),
            std::env::home_dir().as_deref(),
        ),
        standing,
        theme: painting.theme,
        remembering: remembering.map(Path::to_path_buf),
        ..Screen::default()
    };
    screen.say_of_the_theme(&painting.warnings);
    // A spelling nothing can press is said on the frame the view opens on and
    // not again: it is about the file rather than about anything happening,
    // and the next key somebody presses is them having read it. Said after the
    // theme has had its say, because a key that does nothing is the nearer of
    // the two to what they are about to do.
    if !refused.is_empty() {
        screen.notice = Some(Notice::Refused(refused.join(" · ")));
    }
    // And the file behind it, for the loop to notice somebody editing.
    let mut watching = painting.watching;
    // The list opens the way it was left, before anything is drawn on it: a
    // view that gathered itself one way and then jumped to the other would be
    // saying the arrangement is something it does rather than something
    // somebody said.
    if let Some(path) = remembering {
        let remembered = Remembered::read(path);
        screen.list.arrange(remembered.arrangement.clone());
        screen.published = remembered.arrangement;
        screen.viewed_at = stamped(path);
        screen.vendor = remembered.vendor;
        screen.sent = remembered.sent;
    }

    // What the terminal is called at the moment, so that it is said again when
    // it stops being true and not on every frame that did not change it.
    let mut called = String::new();

    // What was taken off the queue to find the end of a run of pointer
    // movements, and is waiting for its own pass to be acted on.
    let mut held: Option<Typed> = None;

    // Whether the reading behind the frame about to be drawn is still to be
    // taken: the view opens on the records and nothing else, and the pass
    // after that is the one that asks tmux.
    let mut records_only = true;

    // Declared once, here, rather than anywhere a record is merely read: this
    // loop is the one caller that stays for an answer that comes back after
    // the read that asked for it has returned, so it is the only one
    // `have_a_line_written` should ever claim a turn on behalf of.
    derive::will_stay_for_the_answer();

    loop {
        let opening = std::mem::take(&mut records_only);
        let refreshing = screen.read.is_none_or(|at| at.elapsed() >= REFRESH);
        match refreshing {
            true if opening => screen.recall(root, scope)?,
            true => screen.reread(root, scope)?,
            // The reread has just taken the card; between rereads a question
            // card is taken again anyway, so it never holds a question the
            // record has moved past.
            false => screen.freshen(),
        }
        // A palette is a file, and a person choosing one edits it with the
        // view open beside them. So it is read again on the wall's own
        // cadence: the colours on the screen are the ones the file says now,
        // and nothing has to be closed and reopened to see them. Only where
        // the reading clock came round — a stat every 120ms would be the view
        // asking the filesystem about a file that changes once a fortnight.
        if refreshing && let Some((theme, warnings)) = watching.reread() {
            screen.repaint(theme);
            screen.say_of_the_theme(&warnings);
        }
        // Another view arranges the same wall through the same file, so what
        // one of them pinned, slept or put in order should land on this one
        // within the tick. A stamp that has not moved is nobody's change.
        if refreshing {
            screen.adopt_the_view();
        }
        screen.step();
        terminal.draw(|frame| paint::draw(frame, &screen))?;

        let title = paint::title(&screen.list);
        if title != called {
            titles.say(&title);
            called = title;
        }

        // A mouse press can do everything a key can — a click on a row is
        // enter on it — so both are read into the same doing.
        let arrived = match held.take() {
            Some(typed) => typed,
            // The opening frame is the records' own account and the reading
            // that checks it against the panes is the next thing this loop
            // does. Nothing is waited for in between: a wall that stood as the
            // records left it until somebody pressed a key would be a wall
            // saying an agent is working a second after its pane went.
            None if opening => Typed::Nothing,
            None => keys.next(TICK),
        };
        let doing = match arrived {
            Typed::Nothing => Doing::Carry,
            Typed::Gone => return Ok(exit::OK),
            Typed::Mouse(mouse) => {
                // A run of movements is one thing that happened, and this
                // frame is the one it is drawn on.
                let (rest, ended) = at_rest(keys, mouse);
                held = ended;
                screen.moused(rest, root, config, here.as_ref())?
            }
            Typed::Paste(text) => {
                screen.pasted(&text, config);
                Doing::Carry
            }
            Typed::Key(key) => screen.act(key, root, config, here.as_ref())?,
        };
        match doing {
            Doing::Carry => {}
            Doing::Close => return Ok(exit::OK),
            Doing::Lend { id, on, session } => {
                screen.notice = lend(terminal, &id, &on, &session)?;
                // Where the terminal has just come back from, for the wall to
                // mark the row with. Only where it went: a lend tmux refused
                // is a row nobody has been to, and the last one somebody did
                // come back from is still the answer to where they were. The
                // pointer is retired either way — the terminal was handed over
                // before tmux said anything.
                match screen.notice.is_none() {
                    true => screen.went_into(id),
                    false => screen.hover = None,
                }
                // Whoever had the terminal had the title with it, so the
                // view says what it is called again rather than trusting a
                // name it did not put there.
                called.clear();
            }
            Doing::Edit => {
                edit_the_line(terminal, &mut screen, config)?;
                called.clear();
            }
            Doing::View { id } => {
                view_the_patch(terminal, &mut screen, root, &id, config)?;
                called.clear();
            }
            Doing::Bound {
                id,
                spelling,
                command,
            } => {
                run_the_key(terminal, &mut screen, root, &id, &spelling, &command)?;
                called.clear();
            }
        }
    }
}

/// Where a run of pointer movements came to rest, and whatever ended the run.
///
/// A terminal reports the pointer a cell at a time, so a hand crossing the
/// screen queues dozens of movements before the view has drawn one frame.
/// They all say the same kind of thing and only the last of them is true: what
/// the pointer is over now. So the queue is read to the end of the run and the
/// frame after it is drawn once, for where the hand stopped.
///
/// What ended the run comes back with it rather than being acted on here. A
/// key or a click behind a hundred movements is the next thing somebody meant
/// to do, and a view that read it off the queue and dropped it would be losing
/// keystrokes to a mouse.
fn at_rest(keys: &mut impl Keys, moved: MouseEvent) -> (MouseEvent, Option<Typed>) {
    let mut rest = moved;
    if rest.kind != MouseEventKind::Moved {
        return (rest, None);
    }
    loop {
        // Nothing is waited for: what is queued at this moment is the run,
        // and an empty queue is the pointer at rest.
        match keys.next(Duration::ZERO) {
            Typed::Mouse(next) if next.kind == MouseEventKind::Moved => rest = next,
            Typed::Nothing => return (rest, None),
            ended => return (rest, Some(ended)),
        }
    }
}

/// Lend the terminal to tmux for as long as somebody is looking at the agent.
///
/// The view and a tmux client both want a whole terminal, and outside tmux
/// there is one terminal: so the view puts the screen back the way it found
/// it, waits, and takes it again. That wait is the whole point — detaching
/// lands back on the list rather than at a shell prompt, which is what
/// `switch-client` gives somebody who was inside tmux to begin with.
fn lend<B>(
    terminal: &mut Terminal<B>,
    id: &str,
    on: &Server,
    session: &SessionId,
) -> Result<Option<Notice>>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let handed = borrowed(terminal, || on.attach_command(session).status())?;

    Ok(match handed {
        Ok(status) if status.success() => None,
        // tmux said why on a terminal the view has just taken back, so it says
        // it again where there is room for it.
        Ok(_) => Some(Notice::Failed(format!("tmux would not open {id}"))),
        Err(e) => Some(Notice::Failed(format!("reaching {id}: {e}"))),
    })
}

/// Give the terminal to an editor, and put what somebody wrote in it on the
/// line the view was holding.
///
/// A line that was being typed and a line the editor filled are the same line:
/// what comes back is the text and nothing else, so enter still does what the
/// prompt in front of it says it will do.
///
/// The editor is opened on the whole of it, pastes and all: a marker is a row
/// for reading a line around, and an editor is where somebody has gone to read
/// the whole thing. What they close it on is the line, so the markers are
/// spent — the text they stood for is in front of the cursor now.
fn edit_the_line<B>(terminal: &mut Terminal<B>, screen: &mut Screen, config: &Config) -> Result<()>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let Mode::Typing(composer) = &screen.mode else {
        return Ok(());
    };
    let text = composer.whole();
    let written = borrowed(terminal, || act::edited(&text))?;

    screen.notice = match written {
        Ok(Edited::Line(text)) => {
            if let Mode::Typing(composer) = &mut screen.mode {
                // At the end of what was written, which is where an editor
                // leaves somebody who has just closed one.
                composer.at = text.chars().count();
                composer.text = text;
                composer.pastes.clear();
            }
            // The line is a line somebody else wrote, so what could stand
            // under its cursor is looked up again rather than carried over.
            screen.suggesting(config);
            None
        }
        Ok(Edited::No(why)) => Some(Notice::Refused(why)),
        Err(e) => Some(Notice::Failed(format!("{e:#}"))),
    };
    Ok(())
}

/// Give the terminal to whatever the config reads patches with, on this
/// agent's work.
///
/// The same borrow the editor gets, and for the same reason: a pager or a
/// differ wants a whole terminal, and what it draws on one is between it and
/// the person reading it. `amx diff` at a shell hands its patch over exactly
/// this way, so the key and the verb put the same thing in front of somebody.
///
/// What the verb will not read — a row with no tree of its own, a tree
/// somebody has removed — it refuses in its own words, which are the words a
/// shell would have got.
fn view_the_patch<B>(
    terminal: &mut Terminal<B>,
    screen: &mut Screen,
    root: &Path,
    id: &str,
    config: &Config,
) -> Result<()>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    // The key that asked read this first: a view that gave the terminal up for
    // a command it does not have would be a screen going away and coming back
    // for nothing.
    let Some(viewer) = &config.diff else {
        return Ok(());
    };
    let read = borrowed(terminal, || verbs::diff::in_viewer(root, id, viewer))?;

    // A viewer that ran and ended badly said so on the terminal it was given,
    // which is between it and whoever was reading; what never got that far is
    // the view's to say.
    screen.notice = read.err().map(|e| Notice::Failed(format!("{e:#}")));
    Ok(())
}

/// Give the terminal to a command somebody bound a key to, in the tree of the
/// agent the cursor was on.
///
/// The same borrow the patch viewer gets, for the same reason and on the same
/// terms: what a person binds a key to is a program they mean to sit in front
/// of — lazygit, a test run under a pager — and the view is not drawing while
/// they are.
///
/// The command said whatever it had to say on the terminal it was handed, so
/// the notice carries only what a person could not have read there, under the
/// spelling they pressed: a wall can hold several bound keys, and which one
/// went wrong is the first thing to say about it.
fn run_the_key<B>(
    terminal: &mut Terminal<B>,
    screen: &mut Screen,
    root: &Path,
    id: &str,
    spelling: &str,
    command: &str,
) -> Result<()>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let ran = borrowed(terminal, || act::run_bound(root, id, command))?;

    screen.notice = ran
        .err()
        .map(|e| Notice::Failed(format!("{spelling}: {e:#}")));
    Ok(())
}

/// Give the terminal up for as long as something else needs the whole of it,
/// and take it back when that is done.
///
/// Both the things that borrow it are whole-screen programs of somebody else's
/// — a tmux client, an editor — and neither can share a terminal with a view
/// drawing on it. What comes back is the screen the view left, drawn again from
/// nothing, because what was on it in the meantime was not the view's.
fn borrowed<B, T>(terminal: &mut Terminal<B>, doing: impl FnOnce() -> T) -> Result<T>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    // The mouse goes back with the screen: whatever borrows the terminal
    // decides for itself whether it wants one.
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    // The disambiguation goes back with them, for the same reason: whatever
    // borrows the terminal reads keys off it and says for itself how it wants
    // them. An editor left inside a protocol the view asked for is an editor
    // reading keys in a shape it never agreed to.
    let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    // And so does the cursor. The view draws its own and keeps the terminal's
    // put away, but an editor is a program that expects to find one there: it
    // never asks for a cursor, it just writes where the cursor is. Nothing
    // undoes this on the way back because every frame the view draws hides
    // the cursor again.
    let _ = execute!(std::io::stdout(), Show);
    ratatui::try_restore().context("giving the terminal up")?;

    let outcome = doing();

    enable_raw_mode().context("taking the terminal back")?;
    execute!(std::io::stdout(), EnterAlternateScreen).context("taking the terminal back")?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let _ = execute!(std::io::stdout(), TELL_THE_KEYS_APART);
    terminal.clear()?;
    Ok(outcome)
}

impl Screen {
    /// Show what the records say, with nothing asked of tmux.
    ///
    /// The frame the view opens on. A reading costs a pane list per server and
    /// a capture for every agent that has gone quiet, and until it comes back
    /// there is nothing to draw: somebody who typed `amx` is looking at the
    /// terminal they typed it in. The records are on disk, they are the
    /// vendor's own account of what each agent was last doing, and a wall of
    /// them is on the screen in the time it takes to read a file each.
    ///
    /// What they cannot say is what has happened since — a pane that went, a
    /// turn that ended, the question behind one the record could not name.
    /// That is the reading's, and the reading is the next thing the view does:
    /// this leaves the clock unset, so the pass straight after takes one.
    fn recall(&mut self, root: &Path, scope: &Scope) -> Result<()> {
        self.showing(scope.narrow(derive::recorded(root, now())?));
        Ok(())
    }

    /// Show a reading, and keep the projects its agents run in.
    ///
    /// The projects are read off the records the wall is drawn from, which is
    /// the same string work the wall's own headings are gathered by and no
    /// second walk of a disk. One entry each, in the order the agents were
    /// read: a word offered twice is a choice between two things that look
    /// identical.
    ///
    /// A reading that has lost the row the cursor was on moves the cursor, and
    /// the window follows it there the way it follows a keypress. Only that
    /// reading: an agent that came or went above the cursor leaves it on its
    /// own row, and a window that came back to the cursor every time the clock
    /// read the records would be a wall nobody could scroll away from for a
    /// second.
    fn showing(&mut self, views: Vec<View>) {
        self.projects.clear();
        for view in &views {
            let project = crate::spawn::project_dir(&view.meta);
            if !self.projects.contains(&project) {
                self.projects.push(project);
            }
        }
        let on = self.list.selected().map(|view| view.id().to_string());
        let pointed = self.hover.and_then(|at| self.line_named(at));
        self.list.show(views);
        let still = self.list.selected().map(|view| view.id());
        if on.is_some() && on.as_deref() != still {
            self.wall.follow.set(true);
        }
        // An agent the cursor was on that has gone from the list takes its arm
        // with it: the rows have moved, and a second press is about a row the
        // person can no longer see.
        if let Some(on) = &on
            && self.list.agent_by_id(on).is_none()
        {
            self.arm = None;
        }
        // The pointer stays only on the line it was resting on. Rows that moved
        // under it carry another agent to that place, and a key read where the
        // pointer is would reach that agent instead.
        if self.hover.is_some_and(|at| self.line_named(at) != pointed) {
            self.hover = None;
        }
    }

    /// What the line at `at` stands for, in terms that outlive a reading: an
    /// agent's id, or a heading's key.
    fn line_named(&self, at: usize) -> Option<String> {
        match self.list.items().get(at)? {
            rows::Item::Agent(_) => self
                .list
                .agent(self.list.items()[at])
                .map(|view| format!("agent:{}", view.id())),
            rows::Item::Heading(under, _) => {
                self.list.key(*under).map(|key| format!("heading:{key:?}"))
            }
            _ => None,
        }
    }

    /// Paint in what the palette file says now.
    ///
    /// The card the view is holding goes with the old colours: its rows were
    /// drawn in them, and every file it was read from still stands, so nothing
    /// else would say the card is out of date.
    fn repaint(&mut self, theme: Theme) {
        self.theme = theme;
        self.taken = None;
    }

    /// Say what reading the theme had to say for itself, where the view says
    /// everything else.
    ///
    /// Somebody who named a palette and got the built-in one is owed the
    /// reason; the alternative is a screen that is quietly not the one they
    /// asked for. A file that was wrong and has been put right takes its own
    /// complaint down with it — but only its own, because a notice somebody's
    /// keypress put there is not the theme's to clear.
    fn say_of_the_theme(&mut self, warnings: &[String]) {
        let said = (!warnings.is_empty()).then(|| warnings.join(" · "));
        let showing = matches!(
            &self.notice,
            Some(Notice::Advice(shown)) if Some(shown) == self.complained.as_ref()
        );
        if said.is_some() || showing {
            self.notice = said.clone().map(Notice::Advice);
        }
        self.complained = said;
    }

    /// Read the agents again, the ones the view was opened about.
    ///
    /// The whole reading, before the narrowing, is what the background fetch is
    /// asked about: what somebody is looking at this minute is no reason to let
    /// another repository's upstream go stale under `c`.
    fn reread(&mut self, root: &Path, scope: &Scope) -> Result<()> {
        // Over the agents this view is about and no others: a view opened on
        // one project brings that project's origin up to date, not every
        // repository an agent on the machine has ever been cut in.
        let views = scope.narrow(derive::views(root, now())?);
        verbs::sweep::fetch_origins_again(&views);
        self.showing(views);
        self.read = Some(Instant::now());
        self.keep_the_sweep();
        self.land_on_what_was_started();
        self.follow_the_cursor();
        Ok(())
    }

    /// Put the cursor on the agent the line just started, the moment the wall
    /// has a row for it.
    ///
    /// Here rather than where it was started: what somebody typed a task for
    /// is what they are about to watch, and the card follows the cursor onto
    /// it as it would after any move.
    ///
    /// The agent is let go of whether or not the cursor landed. A narrowing
    /// that hides it is somebody saying they are looking at something else,
    /// and an agent held onto would take the cursor away from them on
    /// whatever reading widened the wall again.
    fn land_on_what_was_started(&mut self) {
        if let Some(id) = self.started.take() {
            self.list.land_on(&id);
        }
    }

    /// Keep the cursor with a swept group whose heading dissolved under it.
    ///
    /// A group is however its rows read at this moment, and an agent that ends
    /// while the window is open moves to completed by the very next reading:
    /// the heading that was pressed can be gone before the second press,
    /// leaving the cursor's index to whichever heading drifted into it — a
    /// different, live group, one keystroke from being stopped by the press
    /// meant to finish the sweep. So while a sweep's window is open and the
    /// cursor stands on a heading over none of its rows, it is moved to the
    /// heading standing over them: the second press lands on what the first
    /// one was about. A cursor anywhere else is left alone — a row reads its own
    /// presses, and only a heading is a keystroke from sweeping.
    ///
    /// The arm follows too. While the heading that was pressed is still on the
    /// wall, its own second press is the one that finishes the sweep and a
    /// press on any other heading arms that one instead. Once it has gone, the
    /// heading standing over the armed rows is the arm's heading from then on.
    fn keep_the_sweep(&mut self) {
        let Some(arm) = self.arming().filter(|arm| arm.swept) else {
            return;
        };
        if arm
            .heading
            .as_ref()
            .is_some_and(|key| self.list.heading_at(key).is_some())
        {
            return;
        }
        let covers = |under: rows::Under| {
            self.list
                .members(under)
                .iter()
                .any(|view| arm.ids.iter().any(|id| id == view.id()))
        };
        let landing = self
            .list
            .items()
            .iter()
            .enumerate()
            .find_map(|(at, item)| match item {
                rows::Item::Heading(under, _) if covers(*under) => Some((at, *under)),
                _ => None,
            });
        let Some((at, under)) = landing else {
            return;
        };
        let key = self.list.key(under);
        if self.list.heading().is_some() {
            self.list.land(at);
        }
        if let Some(arm) = self.arm.as_mut() {
            arm.heading = key;
        }
    }

    /// Move the working pulse on, if a frame has gone by.
    ///
    /// By the clock rather than by the pass: the loop goes round again the
    /// moment a key arrives, and a row that breathed faster while somebody was
    /// typing would be saying something about the typing.
    fn step(&mut self) {
        if self.stepped.is_none_or(|at| at.elapsed() >= FRAME) {
            self.beat = self.beat.wrapping_add(1);
            self.stepped = Some(Instant::now());
        }
    }

    /// Take the question card again on this pass, between rereads.
    ///
    /// The card is a snapshot, and for an agent that is only working the
    /// wall's own cadence keeps it fresh enough. A question is different:
    /// answering one tab of a call moves the record to the next question at
    /// once, while the vendor is still redrawing the pane behind the card —
    /// a snapshot cut on that moment pairs the new question with the old
    /// tab's capture, and held to the reread cadence the pair stands mixed
    /// on the screen for a second. Taken again every pass, the card settles
    /// the moment the pane does.
    ///
    /// Only the card the record has the question for, which is the card this
    /// is about: its body is the question block, so retaking it costs a
    /// struct built out of a reading already in hand. The other waiting
    /// agent — the one amx read no question for — has the pane for a body,
    /// and taking that is a `capture-pane` fork. Nothing on that card moves
    /// faster than the reading does, so it keeps the reading's cadence like
    /// every other capture on the screen.
    fn freshen(&mut self) {
        let asked = self
            .card
            .as_ref()
            .is_some_and(|card| card.asks() && card.question.is_some());
        if asked {
            self.follow_the_cursor();
        }
    }

    /// How wide a card's body is this frame, which is the band the last frame
    /// drew the list in: what the card says stands in the band's own columns.
    /// Eighty columns before anything has been drawn, which is a card wrapped
    /// for a terminal nobody has measured yet and redrawn to the real one at
    /// the next reading.
    fn body_width(&self) -> u16 {
        self.map.width().unwrap_or(80)
    }

    /// Whether the card on the screen is the card this pass would take anyway:
    /// the same agent, saying what the record still says it is saying, wrapped
    /// for the width the frame has, and read from files nothing has written to
    /// since.
    ///
    /// A card is asked for on every pass, and the reading behind one is a
    /// transcript off disk and a walk of every turn in it. A card nothing
    /// about would change is one the view already has.
    fn stands(&self) -> bool {
        let (Some(taken), Some(view)) = (&self.taken, self.list.selected()) else {
            return false;
        };
        taken.id == view.id()
            && taken.phase == view.phase()
            && taken.question == view.state.question
            && taken.width == self.body_width()
            && !taken.fresh.moved()
    }

    /// Take the card again, when it is the kind that follows the cursor: what
    /// an agent is doing is what its pane is showing now.
    ///
    /// A card paged away from its natural edge is being read, and retaking it
    /// would move the text under somebody's eyes — so it holds still until it
    /// is paged back or the cursor moves off its agent. A question is never
    /// behind the hold: an agent that has stopped to ask takes the card back,
    /// because surfacing that is what the view is for — and a card already
    /// asking is never held either, since the record moves under it while the
    /// vendor redraws, and freshness is what keeps the question and its tab
    /// paired.
    ///
    /// A card that would come back the same is not taken again either — see
    /// [`Screen::stands`]. That hold is about the cost rather than about
    /// somebody's eyes, and it is the one hold a question falls under too: a
    /// question card is read from no file, so nothing about it ever stands.
    ///
    /// And the line at the foot of the card comes and goes with the card
    /// itself, because it is the card's last row: every card has one, and the
    /// keys are the list's again only once there is no card at all.
    fn follow_the_cursor(&mut self) {
        match self.look {
            Look::Away => {
                self.card = None;
                self.taken = None;
                self.heard = Heard::default();
            }
            Look::Screen => {
                // A cursor on a heading or on the fold is not a cursor on
                // another agent: the card holds still on the one it is
                // showing, so walking past a heading does not take the card,
                // its line and the keys that go with it away and give them
                // back a press later.
                let between =
                    self.card.is_some() && (self.list.on_heading() || self.list.on_fold());
                let held = between
                    || self.scroll.paged()
                        && match (&self.card, self.list.selected()) {
                            (Some(card), Some(view)) => {
                                !card.asks()
                                    && view.phase() != Phase::Waiting
                                    && card.id == view.id()
                            }
                            _ => false,
                        };
                if !held && !self.stands() {
                    let width = self.body_width();
                    let taken = self.list.selected().map(|view| {
                        let (card, fresh) =
                            card_of(view, &self.root, width, self.theme, &mut self.heard);
                        let taken = Taken {
                            id: card.id.clone(),
                            phase: card.phase,
                            question: card.question.clone(),
                            width,
                            fresh,
                        };
                        (card, taken)
                    });
                    (self.card, self.taken) = taken.unzip();
                    self.scroll
                        .open_at(self.card.as_ref().map_or(0, Card::opens_at));
                }
            }
            // A diff was taken when somebody asked for it, and stays as it was
            // until they ask again.
            Look::Changes => {}
        }

        // The line comes with the card for as long as the card is up, not
        // just with the press that opened it: an answer that advanced a call
        // to its next tab spent the line it was typed on, and the tab now
        // showing is as much a question in front of somebody as the one
        // before it was. Only where the keys are the list's — a line already
        // being typed is somebody's, whatever it is for — and it goes when
        // the card does, because the two are one thing.
        let replying =
            matches!(&self.mode, Mode::Typing(line) if matches!(line.asking, Asking::Reply));
        match (self.card.is_some(), &self.mode) {
            (true, Mode::List) => self.mode = Mode::Typing(Composer::new(Asking::Reply)),
            (false, _) if replying => self.mode = Mode::List,
            _ => {}
        }
    }

    /// The line being typed on the card, when that is where it is going.
    ///
    /// The card holds one line and only one: what is being said to the agent
    /// it is a look at. Anything else being typed — a task, a name — is a
    /// band of its own under it.
    fn answering(&self) -> Option<&Composer> {
        match &self.mode {
            Mode::Typing(composer) if self.on_the_card(composer) => Some(composer),
            _ => None,
        }
    }

    /// And every other line, which is the one the band under the card draws.
    ///
    /// A task and a rename, which is the whole of it. A reply is the card's
    /// own last row wherever it is being typed, and a find line is drawn on
    /// the row the keys have: a band would cost the list two rows and dim what
    /// was left of it, and the list is exactly what somebody typing there is
    /// watching.
    fn banded(&self) -> Option<&Composer> {
        match &self.mode {
            Mode::Typing(composer) if !matches!(composer.asking, Asking::Reply | Asking::Find) => {
                Some(composer)
            }
            _ => None,
        }
    }

    /// Whether this line is the one at the foot of the card.
    ///
    /// Every card has one, so the question is only whether there is a card:
    /// the line goes to whichever agent the card is showing when enter is
    /// pressed, which is why it names none itself.
    ///
    /// Taken as an argument rather than read off the mode, because the one
    /// place it matters most is the keypress that has the composer out of the
    /// mode in its hand.
    fn on_the_card(&self, composer: &Composer) -> bool {
        matches!(composer.asking, Asking::Reply) && self.card.is_some()
    }

    /// The agent to answer and the choice to answer it with, where the key
    /// just pressed on the card is that choice rather than a character.
    ///
    /// A digit at a question whose numbered choices are the whole of what it
    /// takes, which [`act::picks`] is the reading of. The line has to be
    /// empty: a digit in the middle of words somebody is writing is a
    /// character of them, and the answers that open with one are still typed
    /// after any other character. And the number has to be a choice the card
    /// is showing — a 7 at a question offering two is nothing to send, so it
    /// falls through to the line like any other character.
    ///
    /// The question's own payload comes off the list rather than the card: the
    /// card is a picture of one agent, and whether this question takes more
    /// than one choice is on the record behind it.
    fn picking(&self, composer: &Composer, key: KeyEvent) -> Option<(String, String)> {
        let KeyCode::Char(digit @ '1'..='9') = key.code else {
            return None;
        };
        if !chord(key).is_empty() || !composer.text.is_empty() || !self.on_the_card(composer) {
            return None;
        }
        let card = self.card.as_ref()?;
        let asked = self
            .list
            .agent_by_id(&card.id)
            .and_then(rows::showing)
            .map(|showing| showing.ask);
        let at = digit.to_digit(10)? as usize;
        (act::picks(card.kind, &card.options, asked, card.walked) && at <= card.options.len())
            .then(|| (card.id.clone(), digit.to_string()))
    }

    /// Open the card on the agent under the cursor, with the line to answer it
    /// on where it is asking something.
    ///
    /// The line comes with the card because that is what the card is for: an
    /// agent that has stopped is the reason somebody came to the screen, and
    /// making them press a second key to say so would be a screen that knows
    /// what they want and waits to be asked. Opening it here is the same law
    /// [`Screen::follow_the_cursor`] holds continuously, so this only takes
    /// the look.
    fn look_closer(&mut self, root: &Path) {
        // A card is a look at one agent, and a heading or the fold is not one:
        // the key does nothing there rather than leaving the view looking at
        // an agent it would find the next time the cursor moved.
        let Some(id) = self.list.selected().map(|view| view.id().to_string()) else {
            return;
        };
        self.look = Look::Screen;
        self.follow_the_cursor();

        // Opening the card is the whole of what the mark means, so the record
        // learns it here. A record that will not take the look costs a mark
        // that stays on a row somebody has read, which is not worth spending
        // the one line the view has to say things on.
        let _ = act::looked(root, &id);
        self.acted();
    }

    /// Put the card away, and the line it was holding with it.
    ///
    /// The review goes too, which is what the row under the line says this key
    /// does: a review is written about the patch in front of somebody, and the
    /// key that puts the patch away is them saying they are done with it. One
    /// key out is worth more than notes kept for a card that may never come
    /// back, and the hint names the cost before it is paid.
    fn look_away(&mut self) {
        self.scroll.open_at(0);
        self.look = Look::Away;
        self.follow_the_cursor();
    }

    /// The cursor put on the row the pointer is resting on, for a key that is
    /// read where the person is looking rather than where the cursor was left.
    ///
    /// Answers whether it moved, which is what tells a key aimed at another
    /// row from one aimed at the row it was already on — off the list, and on
    /// the blank, there is nothing to land on and the answer is no.
    fn land_on_the_pointer(&mut self) -> bool {
        let Some(at) = self.hover.filter(|at| *at != self.list.cursor()) else {
            return false;
        };
        if !self.list.land(at) {
            return false;
        }
        self.moved();
        true
    }

    /// Where somebody has just gone: the row the wall marks as the one they
    /// were in, and the end of what the view knows about the pointer.
    ///
    /// Mouse capture is off for as long as another client has the terminal, so
    /// the line the view was holding is about a screen the pointer has crossed
    /// unwatched — and a space after a `ctrl+z` back would open the row that
    /// screen's last session happened to sit on.
    ///
    /// The card goes too, for the same reason the pointer does. A card is a
    /// look at a pane from outside it, and somebody who has just gone into that
    /// pane is not outside it any more: the whole of what the card was standing
    /// in for is now the screen in front of them. Left up, it is what they come
    /// back out to — a stale photograph of the screen they were just on, in
    /// front of the wall they pressed `ctrl+z` to get back to.
    fn went_into(&mut self, id: String) {
        self.lent = Some(id);
        self.hover = None;
        self.look_away();
    }

    /// What one key does.
    ///
    /// The pointer is retired on the way out, whatever the key was: hands on
    /// the keyboard are hands off the mouse, and a pointer parked on a row
    /// while somebody walks the list with `j` is not where they are looking.
    /// Otherwise a keyboard user previews or stops the row their hand happened
    /// to leave the mouse over. The next movement brings the pointer back.
    fn act(
        &mut self,
        key: KeyEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let doing = self.acting(key, root, config, here);
        self.hover = None;
        doing
    }

    /// The key itself, read by whatever the view is in the middle of.
    fn acting(
        &mut self,
        key: KeyEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        // Whatever the view had to say, it was about the last key.
        self.notice = None;

        // Before the modes, whatever the view is in the middle of: a terminal
        // in raw mode has no interrupt of its own, and a view nobody can get
        // out of the usual way is a trap.
        if key.code == KeyCode::Char('c') && chord(key) == KeyModifiers::CONTROL {
            return Ok(Doing::Close);
        }

        // The dials are about the agent that does not exist yet, so they turn
        // wherever somebody might be about to start one: walking the list, and
        // typing the task itself. Not over the keys, where every key is asked
        // to put the agents back, and not under a question about deleting
        // things, where the only key that means anything is the answer.
        if !matches!(self.mode, Mode::Keys | Mode::Confirming(_)) && self.turned(key) {
            return Ok(Doing::Carry);
        }

        match self.mode {
            Mode::Typing(_) => self.typed(key, root, config, here),
            Mode::Keys => Ok(self.reading_the_keys(key)),
            Mode::Confirming(_) => self.answered(key, root, config, here),
            Mode::List => self.pressed(key, root, config, here),
        }
    }

    /// A dial, if this is the key that turns one.
    ///
    /// The shape of them is alt with the initial of the dial the header
    /// names, and shift+tab for the permission mode, which is the chord
    /// claude's own screens cycle it with. The vendor is the `agent` dial
    /// everywhere else a person names it, on the row and in the `agent:`
    /// prefix, so its key is alt+a. Each one changes what the *next*
    /// agent will be started with and nothing about the ones already running.
    fn turned(&mut self, key: KeyEvent) -> bool {
        let alt = chord(key) == KeyModifiers::ALT;
        let plain = chord(key).is_empty();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('a') if alt => self.profile.cycle_vendor(),
            KeyCode::Char('m') if alt => self.profile.cycle_model(),
            KeyCode::Char('e') if alt => self.profile.cycle_effort(),
            KeyCode::Char('w') if alt => self.profile.toggle_worktree(),
            // Shift+tab is a key of its own where a terminal has one, and tab
            // with shift held where it does not.
            KeyCode::BackTab if plain => self.profile.cycle_permission(),
            KeyCode::Tab if plain && shift => self.profile.cycle_permission(),
            _ => return false,
        }
        true
    }

    /// A key on the list.
    ///
    /// Every key here is the one chord it is written under and no other, the
    /// same law a line being typed reads its characters by: a key held down
    /// with control or alt is somebody reaching for something else, and a list
    /// whose plain keys answered to every chord that carried them would close
    /// itself on the alt+q of somebody arranging their windows.
    fn pressed(
        &mut self,
        key: KeyEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let plain = chord(key).is_empty();
        let ctrl = chord(key) == KeyModifiers::CONTROL;
        let alt = chord(key) == KeyModifiers::ALT;
        // The one chord an arrow key arrives under, which is the only place
        // shift is a key of its own rather than the character it typed.
        let shift = plain && key.modifiers.contains(KeyModifiers::SHIFT);
        // Whether the press before this one was the first half of a `gg`.
        // Taken rather than read, so every arm below is a key that cancelled
        // it, and only the one that wants it has to say so.
        let going = std::mem::take(&mut self.going);
        match key.code {
            KeyCode::Char('q') if plain => return Ok(Doing::Close),
            // The cursor with shift held moves the agent it is on instead of
            // moving off it, which is the shape every list that reorders uses.
            KeyCode::Down if shift => {
                let moved = self.list.move_by(1);
                self.keep(moved);
            }
            KeyCode::Up if shift => {
                let moved = self.list.move_by(-1);
                self.keep(moved);
            }
            // The letters vim walks with, beside the arrows they stand for.
            // The view is already modal — every letter is text the moment a
            // line is open, and the rule over that line says so — so these
            // cost nothing but the two arms.
            KeyCode::Down | KeyCode::Char('j') if plain => {
                self.list.down();
                self.moved();
            }
            KeyCode::Up | KeyCode::Char('k') if plain => {
                self.list.up();
                self.moved();
            }
            // Both ends of the list, one press away. `G` is one key; `gg` is
            // two, because that is the pair a person's hands already know, and
            // a single `g` that jumped would fire under every hand reaching
            // for the second one.
            KeyCode::Char('G') if plain => {
                self.list.bottom();
                self.moved();
            }
            KeyCode::Char('g') if plain => match going {
                true => {
                    self.list.top();
                    self.moved();
                }
                false => self.going = true,
            },
            // The cursor keys walk the list even while a card is open; paging
            // inside the card's body is these two — and the two chords under
            // them, which are the same pages for a keyboard that has no page
            // keys to press.
            KeyCode::PageUp if plain => self.paged(true),
            KeyCode::PageDown if plain => self.paged(false),
            KeyCode::Char('b') if ctrl => self.paged(true),
            KeyCode::Char('f') if ctrl => self.paged(false),
            // And half of one, which is the pair a person reading rather than
            // skimming reaches for: enough of the last screen stays to say
            // where the new one came from.
            KeyCode::Char('u') if ctrl => self.paged_by(true, self.half()),
            KeyCode::Char('d') if ctrl => self.paged_by(false, self.half()),
            // And the patch by what it is made of. The pages are how far the
            // card moves; these are what it moves to, which is the unit a
            // patch is read and answered in.
            KeyCode::Char('n') if ctrl => self.to_hunk(true),
            KeyCode::Char('p') if ctrl => self.to_hunk(false),
            // The card, read where the pointer is the way `ctrl+x` is: a hand
            // resting on a row is the row somebody means, so the cursor goes
            // there and the card opens on it. Landing somewhere new opens that
            // row even with a card already up — closing what is open is what
            // the press means only where the cursor already was, which is
            // every press with no pointer on the list.
            KeyCode::Char(' ') if plain => {
                let onto = self.land_on_the_pointer();
                match onto || matches!(self.look, Look::Away) {
                    true => self.look_closer(root),
                    false => self.look_away(),
                }
            }
            // One layer a press, innermost first: the card is in front of the
            // list, so it goes before the list changes under it. A narrowing
            // outlives the line it was typed on, so the key that drops one has
            // to reach it here as well as there — otherwise a wall somebody
            // narrowed an hour ago is a wall with no way back.
            KeyCode::Esc if plain => match (self.card.is_some(), self.list.narrowing()) {
                (false, Some(_)) => self.widen(),
                _ => self.look_away(),
            },
            // The same key, read where the cursor is: a heading opens and
            // shuts the group under it, the fold gives back what it is holding,
            // and a row brings its agent forward.
            //
            // `l` is the third spelling of it, and it is the letter vim walks
            // right with: a person who reaches for `l` on a row is reaching for
            // the thing to the right of it, which is the agent itself. The card
            // is what space is for, and space is the only key that opens one.
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if plain => {
                if self.list.on_heading() {
                    self.list.shut_or_open();
                    self.follow_the_cursor();
                } else if self.list.on_fold() {
                    self.list.unfold();
                } else {
                    return self.bring_forward(root, config, here);
                }
            }
            // The agents by where they are on the wall, which is the number a
            // person watching one has already counted off the screen. Nine of
            // them, because there is no tenth key.
            KeyCode::Char(digit @ '1'..='9') if alt => {
                let at = digit.to_digit(10).unwrap_or_default() as usize;
                return self.reach_the_nth(at, root, config, here);
            }
            // And the one agent nobody has to count to: whichever of them is
            // waiting on the person at the keyboard.
            KeyCode::Char('w') if plain => self.land_on_what_needs_you(),
            // Back the way somebody came, which is the one thing the wall
            // cannot say and the trail can.
            KeyCode::Backspace if plain => self.land_on_where_you_were(root),
            // Opened at the top, wherever the last person to ask left it: the
            // question is what the keys are, not where somebody stopped
            // reading them.
            KeyCode::Char('?') if plain => {
                self.keymap.opened();
                self.mode = Mode::Keys;
            }
            KeyCode::Char('n') if plain => self.mode = Mode::Typing(self.task_line()),
            // The list narrowed by what somebody is looking for, read on every
            // keystroke rather than on the enter at the end of them. Narrowing
            // was reachable before this only by opening a task line and
            // knowing its grammar, which is a thing to be told rather than a
            // thing to find.
            KeyCode::Char('/') if plain => {
                self.mode = Mode::Typing(Composer::new(Asking::Find));
            }
            // The same line, opened in the editor somebody already has their
            // fingers in. A task worth a paragraph is a task worth writing
            // where writing is what the keys are for.
            KeyCode::Char('g') if ctrl => {
                self.mode = Mode::Typing(self.task_line());
                return Ok(Doing::Edit);
            }
            // A name for the agent under the cursor, opened on the one it is
            // going by: what somebody wants is usually a word of the current
            // name, and a line that started empty would have them type the
            // part they were keeping.
            KeyCode::Char('r') if ctrl => {
                if let Some(view) = self.list.selected() {
                    let mut composer = Composer::new(Asking::Name {
                        id: view.id().to_string(),
                    });
                    composer.insert(rows::called(view));
                    self.mode = Mode::Typing(composer);
                }
            }
            KeyCode::Char('d') if plain => {
                if let Some(view) = self.list.selected() {
                    match act::changes(root, view) {
                        Ok(card) => {
                            self.card = Some(card.read());
                            // Not a card the view took of a reading, so there
                            // is nothing about it that could stand: whatever
                            // puts the look back on the agent takes its card.
                            self.taken = None;
                            self.look = Look::Changes;
                            // A patch just taken is read from its top.
                            self.scroll.open_at(0);
                            // And it is a card like any other, so it opens
                            // with the line at its foot. Nothing here is
                            // taken again: a diff stands as it was read.
                            self.follow_the_cursor();
                        }
                        Err(e) => self.notice = Some(Notice::Failed(format!("{e:#}"))),
                    }
                }
            }
            // The same patch, read in whatever the person already reads
            // patches in: the card is where it is answered, and a pager with
            // colours and a word diff is where it is read. The terminal goes
            // to the viewer for as long as that takes.
            KeyCode::Char('d') if alt => {
                if let Some(view) = self.list.selected() {
                    match config.diff.is_some() {
                        true => {
                            return Ok(Doing::View {
                                id: view.id().to_string(),
                            });
                        }
                        // Nothing to read it with is nothing the view can go
                        // and find, so what it says is the key that would
                        // have named one.
                        false => {
                            self.notice = Some(Notice::Refused(
                                "no diff key in the config to read the patch with".to_string(),
                            ));
                        }
                    }
                }
            }
            // The request the row is already carrying, in the browser it is
            // reviewed in. The number is the column's own: what the row shows
            // is what opens, so nothing here asks a forge anything.
            KeyCode::Char('o') if plain => {
                if let Some(view) = self.list.selected() {
                    self.notice = match self.list.requests(view).first() {
                        Some(pr) => act::open(view, pr.number)
                            .err()
                            .map(|e| Notice::Failed(format!("{e:#}"))),
                        // A row amx cut no branch for is a row with nothing to
                        // open, and saying which row that was is the whole of
                        // what a wall of them needs.
                        None => Some(Notice::Refused(format!(
                            "no pull request on {}",
                            rows::called(view)
                        ))),
                    };
                }
            }
            // The turn the row is in the middle of, cut short where it stands.
            // What is at the pane is the verb's own reading of the record, so
            // the key and `amx interrupt` keep a person away from the same
            // three panes and say why in the same words.
            KeyCode::Char('i') if plain => {
                if let Some(view) = self.list.selected() {
                    let name = rows::called(view);
                    self.notice = Some(match interrupt::cut_the_turn(root, view.id()) {
                        Ok(Cut::Turn) => Notice::Advice(format!("interrupted {name}")),
                        // Escape at a question dismisses it, which is an answer
                        // nobody can take back, so the key that means to answer
                        // is named instead.
                        Ok(Cut::Question) => Notice::Refused(format!(
                            "{name} is waiting on a question, not working; esc on \
                             its card dismisses it"
                        )),
                        Ok(Cut::Command) => Notice::Refused(format!(
                            "{name} is a command, not an agent; ctrl+x stops it"
                        )),
                        Ok(Cut::Nothing) => Notice::Refused(format!(
                            "{name} is {}; nothing is running to interrupt",
                            interrupt::doing(view)
                        )),
                        Err(e) => Notice::Failed(format!("{e:#}")),
                    });
                }
            }
            // A copy of the agent under the cursor, on a task typed at the
            // line the key opens. Nothing typed is a copy with no first turn,
            // which is a conversation somebody means to take somewhere else
            // and has not said where yet.
            //
            // A command amx ran is turned away here rather than at the line:
            // no vendor was started for it, so there is no conversation to
            // copy, and a line asking for a task nothing could be given would
            // be a keystroke to take back.
            KeyCode::Char('f') if plain => {
                if let Some(view) = self.list.selected() {
                    match view.meta.agent {
                        Some(_) => {
                            self.mode = Mode::Typing(Composer::new(Asking::Fork {
                                id: view.id().to_string(),
                            }));
                        }
                        None => {
                            self.notice = Some(Notice::Refused(format!(
                                "{} is a command, not an agent; there is no \
                                 conversation to copy",
                                rows::called(view)
                            )));
                        }
                    }
                }
            }
            // The same key, read where the cursor is: on a row it is that
            // agent's ending, and on a heading it is the finished agents under
            // it, which is the one place a person is looking at a group rather
            // than at an agent. A pointer resting on a row or a heading is
            // where the person is looking, so the cursor goes there first and
            // the press is read on it — the way a click lands before it acts.
            KeyCode::Char('x') if ctrl => {
                self.land_on_the_pointer();
                match self.list.heading() {
                    Some(under) => self.sweep_or_arm(root, under),
                    None => self.end_or_arm(root),
                }
            }
            // And the whole wall's worth of finished rows, which is the one
            // press here that is about no row in particular.
            KeyCode::Char('c') if plain => self.clear_or_arm(root),
            // What each row runs, which is off the wall until it is asked for:
            // the answer is the same every time on a fleet running one vendor,
            // and the room is the summary's.
            KeyCode::Char('v') if plain => {
                self.vendor = !self.vendor;
                self.keep(true);
            }
            // The same agents, gathered the other way.
            KeyCode::Char('s') if ctrl => {
                self.list.turn();
                self.follow_the_cursor();
                self.keep(true);
            }
            // The agent under the cursor pinned over the wall, so that the
            // one somebody is watching stays where they are looking.
            KeyCode::Char('t') if ctrl => {
                let held = self.list.hold_or_let_go();
                self.keep(held);
            }
            // And the other direction: the agent under the cursor put under
            // the whole wall, so that the ones somebody is not looking at are
            // out of the way of the ones they are.
            KeyCode::Char('z') if plain => {
                let slept = self.list.sleep_or_wake();
                self.keep(slept);
            }
            // And last of all, a key of somebody's own. Here rather than
            // anywhere above, so every key amx binds keeps the meaning the
            // keys screen gives it and a table entry naming one of them never
            // runs: what a person may rebind is what amx left unbound.
            //
            // The command runs on the agent under the cursor, so a heading and
            // an empty wall do nothing and say nothing, the way alt+d does —
            // there is no tree to run it in and nothing was asked for.
            _ => {
                if let Some(view) = self.list.selected()
                    && let Some(bound) = self
                        .bound
                        .iter()
                        .find(|bound| key.code == bound.key.code && chord(key) == chord(bound.key))
                {
                    return Ok(Doing::Bound {
                        id: view.id().to_string(),
                        spelling: bound.spelling.clone(),
                        command: bound.command.clone(),
                    });
                }
            }
        }
        Ok(Doing::Carry)
    }

    /// Text arriving in one piece, which is a paste.
    ///
    /// It waits on the line, every newline in it included: a paste is one
    /// edit, and what dispatches it is the enter pressed afterwards. Its own
    /// trailing newline is text like any other.
    ///
    /// A long one waits behind a marker instead, on the two lines long enough
    /// to be pasted into — the task and the reply, which are what somebody
    /// writes a paragraph around. A name is a name and a find line is a word,
    /// so a paste at either of those is the characters it is: a marker on a
    /// line with nowhere to send it would be a line nobody could read back.
    ///
    /// Pasted at the list it opens a task line rather than being read as keys.
    /// A wall of agents whose keys stop things and forget things is no place to
    /// replay somebody's clipboard, and a person who pasted a task at the view
    /// meant it for the one thing here that takes text.
    fn pasted(&mut self, text: &str, config: &Config) {
        // Whatever the view had to say, it was about the last thing that
        // happened, and this is another one.
        self.notice = None;

        // A terminal that ends its lines the other way is still ending lines.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match &mut self.mode {
            Mode::Typing(composer) => match composer.asking {
                Asking::Task | Asking::Reply | Asking::Fork { .. } => composer.paste(&text),
                Asking::Name { .. } | Asking::Find => composer.insert(&text),
            },
            _ => {
                let mut composer = self.task_line();
                composer.paste(&text);
                self.mode = Mode::Typing(composer);
            }
        }
        // A paste is an edit like any other, so the word it left the cursor in
        // is looked up like any other.
        self.suggesting(config);
    }

    /// A key while somebody is typing a line.
    ///
    /// The composer is taken out of the mode for the length of the keypress and
    /// put back unless the key was the end of it, which is what makes entering
    /// and cancelling the same one move: the line is gone either way. A line
    /// that was entered and refused was not the end of it, so it comes back.
    fn typed(
        &mut self,
        key: KeyEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let Mode::Typing(mut composer) = std::mem::take(&mut self.mode) else {
            return Ok(Doing::Carry);
        };

        // The one rule while the card is holding a line: a key the line has a
        // use for is the line's, and every other key is the list's, as if the
        // line were not there. The line goes back into the mode before the
        // list acts, because it is still standing there afterwards.
        //
        // Two of them are read on an empty line before that rule, which is the
        // whole of the exception to it: space and enter have nothing to do to
        // a line with nothing on it, so down there they are the wall's.
        //
        // Unless a review is waiting to go. Then the line is empty and the card
        // is not: the words were typed a hunk ago and enter is what sends them,
        // which is worth more down here than the row the cursor is standing on.
        let sending = key.code == KeyCode::Enter && !self.noted("").is_empty();
        let empty = composer.text.is_empty() && the_lists_on_an_empty_line(key) && !sending;
        if self.on_the_card(&composer) && (empty || !the_lines(&composer, key)) {
            self.mode = Mode::Typing(composer);
            return self.pressed(key, root, config, here);
        }

        // Before the line takes the key at all: at a question whose numbered
        // choices are the whole of what it takes, the number pressed is the
        // answer and goes as it is pressed.
        if let Some((id, choice)) = self.picking(&composer, key) {
            let said = act::reply(root, &id, &choice);
            self.replied(said, composer);
            return Ok(Doing::Carry);
        }

        match key.code {
            // The suggestions go before the line does: what somebody is
            // looking at when they press this is the list under the word they
            // are typing, and one key back from a list is the list gone.
            KeyCode::Esc if composer.suggest.is_some() => {
                composer.suggest = None;
                self.mode = Mode::Typing(composer);
                return Ok(Doing::Carry);
            }
            // Cancelled: the line goes, and nothing was done with it. The card
            // that was holding it goes too — it and the line are one thing, so
            // one key is what closes them.
            KeyCode::Esc => {
                if self.on_the_card(&composer) {
                    self.look_away();
                }
                // A find line has already done its work by the time esc is
                // pressed, so cancelling it means putting the fleet back.
                if let Asking::Find = composer.asking {
                    self.widen();
                }
                return Ok(Doing::Carry);
            }
            // A newline in the line rather than the end of it. The enters that
            // grow the composer by hand rather than dispatching: a composer
            // where the plain one did not would be a composer nobody could
            // send from.
            //
            // Shift is the one everybody reaches for, and it only arrives at
            // all on a terminal that took [`TELL_THE_KEYS_APART`]: everywhere
            // else shift+enter is an enter, and the line it was pressed on is
            // sent. Alt is what the terminals that cannot say the shift have,
            // and is read whether or not the shift came with it.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                composer.insert("\n");
            }
            // And the same newline under the chord a terminal that will not
            // send either of those has instead. It arrives as 0x0A, which raw
            // mode no longer turns into a carriage return, so what crossterm
            // hands over is the letter that byte is the control code of.
            KeyCode::Char('j') if chord(key) == KeyModifiers::CONTROL => composer.insert("\n"),
            // The word the choice is standing on, put where the word being
            // typed is. Enter as well as tab, because a line with a list open
            // under it is a line somebody is still writing a word of: sending
            // it on the key that finishes the word would start an agent on a
            // spelling they were in the middle of correcting.
            //
            // Enter only while the word is still short of the choice, though.
            // A word already spelled the way the choice spells it is finished,
            // and enter on a finished word is enter on the line: `/review`
            // typed out leaves one suggestion, itself, and a line that took
            // two enters to send would be a line that punished spelling the
            // word. Tab keeps the one job whatever the word says.
            KeyCode::Tab if composer.suggest.is_some() && chord(key).is_empty() => {
                composer.complete();
            }
            // The same key on a line with nothing on it, where there is no
            // word to take: it writes the mark that asks for one. What stands
            // under it is what a typed `@` brings — the vendor's own agents,
            // and the project's files where it has none — so somebody who
            // does not know what to type is one press from what there is.
            //
            // The task line alone, because the band is: a reply, a name and a
            // find line have nothing to open under them, and a mark written
            // onto one of those would be a character to take back.
            KeyCode::Tab
                if composer.text.is_empty()
                    && matches!(composer.asking, Asking::Task)
                    && chord(key).is_empty() =>
            {
                composer.insert("@");
            }
            KeyCode::Enter if composer.finishing() && chord(key).is_empty() => {
                composer.complete();
            }
            KeyCode::Enter => {
                // A find line narrowed the list as it was typed, so enter has
                // nothing left to do but close it and leave the narrowing
                // standing.
                if let Asking::Find = composer.asking {
                    return Ok(Doing::Carry);
                }
                // A name is refused rather than dropped, empty or not: the
                // line was opened on a name somebody meant to edit, and a
                // keystroke that quietly threw it away would look like a
                // rename that happened.
                if let Asking::Name { id } = &composer.asking {
                    let id = id.clone();
                    match act::rename(root, &id, &composer.text) {
                        Ok(Renamed::Yes(said)) => {
                            self.notice = Some(Notice::Advice(said));
                            self.acted();
                        }
                        Ok(Renamed::No(why)) => {
                            self.notice = Some(Notice::Refused(why));
                            self.mode = Mode::Typing(composer);
                        }
                        Err(e) => {
                            self.notice = Some(Notice::Failed(format!("{e:#}")));
                            self.acted();
                        }
                    }
                    return Ok(Doing::Carry);
                }
                // A fork line is entered empty as well as written on: what
                // the copy is given is the task typed at it, and a line with
                // nothing on it is a copy sitting at the conversation it was
                // made from with no turn put to it yet.
                if let Asking::Fork { id } = &composer.asking {
                    let id = id.clone();
                    let whole = composer.whole();
                    let task = (!whole.trim().is_empty()).then_some(whole);
                    let made = act::spawn_copy(root, &id, task.as_deref());
                    self.forked(made, composer);
                    return Ok(Doing::Carry);
                }
                // The card's line goes to the card's agent, read at the press
                // rather than kept from the moment the line opened: the card
                // follows the cursor, and what somebody is looking at when
                // they press enter is what they are answering.
                if matches!(composer.asking, Asking::Reply) {
                    if let Some(id) = self.card.as_ref().map(|card| card.id.clone()) {
                        // The whole review as one message: what was written at
                        // the top of the patch, every hunk somebody left words
                        // on, and the line itself as the note on the hunk under
                        // the cursor. A review is one turn because that is how
                        // it is read — sent a hunk at a time the agent answers
                        // the first note before the second has arrived. A card
                        // with nothing written about it sends the line alone,
                        // which is every other card's message.
                        let review = self.review(&composer.whole());
                        let said = self.written(&review);
                        if said.trim().is_empty() {
                            return Ok(Doing::Carry);
                        }
                        // Before the send, because the send is what spends
                        // them: a review the agent took is behind whoever
                        // wrote it.
                        let kept = !self.scroll.remarks().is_empty();
                        let sent = act::reply(root, &id, &said);
                        let took = matches!(sent, Ok(Replied::Yes(_)));
                        self.replied(sent, composer);
                        // And the card goes back to the top of the patch it
                        // was a review of, with nothing kept and no hunk under
                        // the cursor. A refusal keeps both, because the review
                        // is still to send.
                        if took && kept {
                            self.scroll.open_at(0);
                        }
                    }
                    return Ok(Doing::Carry);
                }
                if composer.text.trim().is_empty() {
                    return Ok(Doing::Carry);
                }

                return self.entering(root, config, composer, false, here);
            }
            // The same line entered, with whoever pressed it going along: the
            // agent is started and the terminal is put in front of it.
            //
            // On a task and nothing else. A reply goes to an agent already on
            // the wall, which is a row away from a key that reaches one, and a
            // find line has nothing to go to.
            KeyCode::Char('n')
                if chord(key) == KeyModifiers::ALT
                    && matches!(composer.asking, Asking::Task)
                    && !composer.text.trim().is_empty() =>
            {
                return self.entering(root, config, composer, true, here);
            }
            // The line goes to the editor, and the mode is put back before it
            // does: what the editor writes lands on the line it was opened on.
            KeyCode::Char('g') if chord(key) == KeyModifiers::CONTROL => {
                self.mode = Mode::Typing(composer);
                return Ok(Doing::Edit);
            }
            // What is taken back is taken from where the cursor is: the
            // character behind it, the one under it, and the word behind it by
            // either of the two chords a terminal has for that.
            KeyCode::Backspace if chord(key) == KeyModifiers::ALT => composer.delete_word_back(),
            KeyCode::Char('w') if chord(key) == KeyModifiers::CONTROL => {
                composer.delete_word_back()
            }
            // The mark that opened the find taken back: on a find line with
            // nothing left on it, backspace is the second way out of the
            // filter, the way it is in vim. Esc does the same, and a line
            // somebody has emptied a character at a time ends on the key
            // their fingers would reach for next anyway.
            KeyCode::Backspace
                if chord(key).is_empty()
                    && composer.text.is_empty()
                    && matches!(composer.asking, Asking::Find) =>
            {
                self.widen();
                return Ok(Doing::Carry);
            }
            KeyCode::Backspace => composer.delete_back(),
            KeyCode::Delete => composer.delete_forward(),
            // Where the next character lands, moved by hand: one character
            // with an arrow, a word with control held, and both ends of the
            // line by the keys a terminal has had for them since before it had
            // arrows.
            KeyCode::Left if chord(key) == KeyModifiers::CONTROL => composer.word_left(),
            KeyCode::Right if chord(key) == KeyModifiers::CONTROL => composer.word_right(),
            KeyCode::Left => composer.left(),
            KeyCode::Right => composer.right(),
            KeyCode::Home => composer.home(),
            KeyCode::End => composer.end(),
            KeyCode::Char('a') if chord(key) == KeyModifiers::CONTROL => composer.home(),
            KeyCode::Char('e') if chord(key) == KeyModifiers::CONTROL => composer.end(),
            // The choice, where there are suggestions under the word to choose
            // between. A line is one thing to walk along and the list under it
            // is another, so the keys that walk each of them are different
            // keys.
            KeyCode::Up if chord(key).is_empty() && composer.suggest.is_some() => {
                composer.choose(-1)
            }
            KeyCode::Down if chord(key).is_empty() && composer.suggest.is_some() => {
                composer.choose(1)
            }
            // The lines sent before, walked on the alt arrows on any line and
            // on the plain ones on a task line, where nothing else has a use
            // for them: a task line stands over a dimmed wall with no card for
            // the arrows to move. The card's line keeps its plain arrows for
            // the card.
            KeyCode::Up | KeyCode::Down
                if chord(key) == KeyModifiers::ALT
                    || (chord(key).is_empty() && matches!(composer.asking, Asking::Task)) =>
            {
                let sent = self.sent.lines_for(&composer.asking);
                composer.recall(sent, key.code == KeyCode::Up);
            }
            // A key held down with control or alt is somebody reaching for
            // something else, not a character they meant to type.
            KeyCode::Char(typed)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                composer.insert(&typed.to_string());
            }
            _ => {}
        }

        // A find line is answered by the wall rather than sent anywhere, so
        // whatever the press did to it, the narrowing is taken again here: the
        // list under the line is what somebody is reading, and it is no use to
        // them a keystroke behind.
        if let Asking::Find = composer.asking {
            self.list.narrow(act::finding(&composer.text));
            self.follow_the_cursor();
        }

        self.mode = Mode::Typing(composer);
        // And a task line by the vendor, on the same cadence and for the same
        // reason. Not after the two keys that move the choice: those leave the
        // line exactly as it was, and looking it up again would put the choice
        // back on the first word every time somebody walked past it.
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.suggesting(config);
        }
        Ok(Doing::Carry)
    }

    /// What became of what the card sent, said out loud.
    ///
    /// A line the agent would not take is a line somebody is still writing,
    /// the same as a task a dial refused: an answer retyped is an answer, and
    /// one thrown away is somebody typing it again from the start. A digit
    /// that was refused leaves the empty line it was pressed on, which is
    /// where it was.
    fn replied(&mut self, said: Result<Replied>, composer: Composer) {
        match said {
            Ok(Replied::Yes(said)) => {
                self.remember_line(&Asking::Reply, &composer.whole());
                self.notice = Some(Notice::Advice(said));
                self.acted();
            }
            Ok(Replied::No(why)) => {
                self.notice = Some(Notice::Refused(why));
                self.mode = Mode::Typing(composer);
            }
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                self.acted();
            }
        }
    }

    /// What became of the copy the line asked for, said out loud.
    ///
    /// What [`Screen::starting`] does with a spawn, because a fork is one: the
    /// line is kept among the tasks, the cursor goes to meet the agent it
    /// made, and a line nothing was made from stays where it was typed with
    /// the reason under it. Nobody goes with a copy — the key that starts one
    /// is pressed at the wall, where the agent it was copied from is still
    /// running and still worth watching.
    fn forked(&mut self, made: Result<Started>, composer: Composer) {
        match made {
            Ok(Started::Yes { id, said }) => {
                self.remember_line(&composer.asking, &composer.whole());
                self.notice = Some(Notice::Advice(said));
                self.acted();
                self.started = Some(id);
            }
            Ok(Started::No(why)) => {
                self.notice = Some(Notice::Refused(why));
                self.mode = Mode::Typing(composer);
            }
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                self.acted();
            }
        }
    }

    /// A task line, opened where the cursor is standing.
    ///
    /// The wall gathered by project is somebody reading one project, so a line
    /// opened there is a line about it: the agent it starts runs where the rows
    /// around the cursor are running. Anywhere else the line carries nothing
    /// and the agent starts where the view was opened.
    fn task_line(&self) -> Composer {
        let mut composer = Composer::new(Asking::Task);
        composer.under = self.list.project_under_cursor();
        composer
    }

    /// Look the word under the cursor up again, and hold what could stand
    /// there.
    ///
    /// Against the vendor the header is showing, because that is what this
    /// view says the next agent will be started with, and against the
    /// directory the agent will run in, because a file offered out of
    /// anywhere else is a file it would not find: the project the line was
    /// opened under where the wall was showing one, and the directory the
    /// view was opened in otherwise, which is the same answer entering the
    /// line gives. A `d:` on the line still says it instead. The projects go
    /// with them, so that a `d:` can be aimed at one of them without a path
    /// being typed out.
    fn suggesting(&mut self, config: &Config) {
        let Mode::Typing(composer) = &self.mode else {
            return;
        };
        // A task line's words are the words of the vendor the header's dial
        // names, read under the project the line was opened under. The card's
        // line goes to an agent already running: its words are that agent's
        // own vendor's, read under the directory it runs in, because a word
        // offered out of anywhere else is a word that agent would not find.
        // A fork line is the same reading of the agent it copies, since the
        // copy runs that agent's vendor where that agent ran.
        let (launching, project) = match &composer.asking {
            Asking::Reply | Asking::Fork { .. } => {
                // The card's line is aimed at whichever agent the card is
                // open on, read at the keystroke; a fork line is aimed at the
                // row the key was pressed on, whatever the cursor has done
                // since.
                let whose = match &composer.asking {
                    Asking::Fork { id } => Some(id.clone()),
                    _ => self.card.as_ref().map(|card| card.id.clone()),
                };
                let Some(view) = whose.and_then(|id| self.list.agent_by_id(&id)) else {
                    return;
                };
                let vendor = view
                    .meta
                    .agent
                    .clone()
                    .unwrap_or_else(|| config.agent.clone());
                (
                    Config {
                        agent: vendor,
                        ..config.clone()
                    },
                    view.meta.dir.clone(),
                )
            }
            _ => (
                self.profile.launching(config),
                composer
                    .under
                    .clone()
                    .unwrap_or_else(|| self.standing.clone()),
            ),
        };
        let found = act::suggest(composer, &launching, &project, &self.projects);
        if let Mode::Typing(composer) = &mut self.mode {
            composer.suggest = found;
        }
    }

    /// Put the agent that has just been started in front of whoever started it.
    ///
    /// Read from its record rather than looked for in the list: the wall is a
    /// reading a second old, and this agent is younger than that.
    fn landing(
        &mut self,
        root: &Path,
        config: &Config,
        id: &str,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let view = match derive::view(root, id, now()) {
            Ok(view) => view,
            // Started and unreachable is worth saying and not worth closing the
            // view over: the agent is running either way, and `amx attach` is
            // still a thing somebody can type.
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                return Ok(Doing::Carry);
            }
        };

        let reached = reach(root, config, here, &view)?;
        Ok(self.arrived(id.to_string(), reached))
    }

    /// Act on what reaching the agent `id` came to.
    fn arrived(&mut self, id: String, reached: Reach) -> Doing {
        match reached {
            // Inside tmux the client has gone to the agent and this view is
            // still drawing behind it, so where somebody went is known now
            // rather than when a lend comes back.
            Reach::There => self.went_into(id),
            Reach::Say(notice) => self.notice = Some(notice),
            Reach::Lend(on, session) => return Doing::Lend { id, on, session },
        }
        Doing::Carry
    }

    /// Bring the agent under the cursor's window forward, which is what enter
    /// does on a row — and a click, which is enter by another hand.
    fn bring_forward(
        &mut self,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let Some(view) = self.list.selected() else {
            return Ok(Doing::Carry);
        };
        let id = view.id().to_string();
        let reached = reach(root, config, here, view)?;
        // An agent that came back is in a pane this reading knows nothing
        // about.
        self.acted();
        Ok(self.arrived(id, reached))
    }

    /// Bring forward the agent standing at `at` on the wall, counted from the
    /// top of the list as it is drawn.
    ///
    /// Agents rather than lines: a heading and the fold are not things a person
    /// counts when they are looking for the third agent, and a group somebody
    /// has shut holds none that can be counted to at all.
    fn reach_the_nth(
        &mut self,
        at: usize,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let nth = self
            .list
            .items()
            .iter()
            .filter_map(|item| self.list.agent(*item))
            .nth(at.saturating_sub(1));
        let Some(view) = nth else {
            self.notice = Some(Notice::Refused(format!(
                "the wall has fewer than {at} agents"
            )));
            return Ok(Doing::Carry);
        };

        let id = view.id().to_string();
        let reached = reach(root, config, here, view)?;
        self.acted();
        Ok(self.arrived(id, reached))
    }

    /// `w` on the list: the cursor onto the first agent with something on it
    /// for whoever is reading, which is what `amx attach --waiting` answers at
    /// a shell, in the same words where the answer is nobody.
    ///
    /// The cursor and nothing else. What to do about the row it lands on is
    /// the press after this one — the card, an answer, the window itself — so
    /// nothing here opens anything or takes the terminal anywhere.
    ///
    /// Read wherever the cursor is standing, a heading included: the question
    /// is about the wall rather than about the line somebody stopped on.
    fn land_on_what_needs_you(&mut self) {
        match self.list.first_needing() {
            Some(id) => {
                self.list.land_on(&id);
            }
            None => {
                self.notice = Some(Notice::Advice(
                    "nothing on the wall is waiting on you".to_string(),
                ));
            }
        }
    }

    /// `backspace` on the list: the cursor onto the agent this terminal was
    /// last handed to, which is what `amx attach --last` goes back to at a
    /// shell, in the same words where there is nobody.
    ///
    /// The trail rather than the wall, because where somebody was before they
    /// came here is the one thing the wall cannot say. It is read past the row
    /// the cursor is already on, so two presses go between two agents rather
    /// than standing still on one, and past a name the list is not drawing:
    /// an agent that has been forgotten, or that a narrowing left off the
    /// screen, is nowhere the cursor can go.
    ///
    /// The cursor and nothing else, as `w` is: what to do about the row it
    /// lands on is the press after this one.
    fn land_on_where_you_were(&mut self, root: &Path) {
        let standing = self.list.selected().map(|view| view.id().to_string());
        let back = verbs::attach::visited(root)
            .into_iter()
            .find(|id| Some(id) != standing.as_ref() && self.list.land_on(id));
        if back.is_none() {
            self.notice = Some(Notice::Advice("no agent to go back to".to_string()));
        }
    }

    /// Enter the line, which starts an agent on it — asking first where the
    /// task is barely one, and only the once: the answer starts it, so nothing
    /// comes back round to here.
    ///
    /// `follow` is whether whoever pressed the key is going with the agent.
    fn entering(
        &mut self,
        root: &Path,
        config: &Config,
        composer: Composer,
        follow: bool,
        here: Option<&Here>,
    ) -> Result<Doing> {
        if let Some(task) = act::slight(config, &composer.whole()) {
            self.mode = Mode::Confirming(Asked::Slight {
                task,
                line: composer,
                follow,
            });
            return Ok(Doing::Carry);
        }
        self.starting(root, config, composer, follow, here)
    }

    /// Start an agent on the line, under the dials the header is showing,
    /// which are what this view says the next agent will be started with.
    fn starting(
        &mut self,
        root: &Path,
        config: &Config,
        composer: Composer,
        follow: bool,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let launching = self.profile.launching(config);
        match act::start(
            root,
            &launching,
            &composer.whole(),
            composer.under.as_deref(),
            &self.standing,
        ) {
            Ok(Started::Yes { id, said }) => {
                self.remember_line(&Asking::Task, &composer.whole());
                self.notice = Some(Notice::Advice(said));
                self.acted();
                if follow {
                    return self.landing(root, config, &id, here);
                }
                // Whoever stayed here is watching the wall for the agent they
                // just started, so the cursor goes to meet it. Not now: the
                // rows on the screen were read before it existed.
                self.started = Some(id);
            }
            // A line nothing was made from is a line somebody is still
            // writing, so it stays where they typed it with the reason under
            // it. A task retyped because a dial was misspelt is a task
            // somebody types shorter the second time.
            Ok(Started::No(why)) => {
                self.notice = Some(Notice::Refused(why));
                self.mode = Mode::Typing(composer);
            }
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                self.acted();
            }
        }
        Ok(Doing::Carry)
    }

    /// The arm, while its window is still open.
    ///
    /// Worked out from the clock every time it is asked for rather than
    /// cleared when it falls due: what closes the window is time passing, and
    /// there is nothing running in this view to do the clearing at the moment
    /// it happens.
    fn arming(&self) -> Option<&Arm> {
        self.arm.as_ref().filter(|arm| arm.at.elapsed() < ARMED)
    }

    /// The rows a press has armed, while its window is still open.
    fn armed(&self) -> &[String] {
        self.arming().map_or(&[], |arm| arm.ids.as_slice())
    }

    /// The rows a `ctrl+x` armed, which is the only arm that key finishes.
    ///
    /// The arm `c` leaves is about everything that has finished, and the rows
    /// it marks were never chosen by anybody's cursor. A `ctrl+x` pressed into
    /// that window is somebody reaching for the other key a beat late, so it
    /// starts its own arm rather than forgetting a wall of agents nobody
    /// pointed at.
    fn forgetting(&self) -> &[String] {
        self.arming()
            .filter(|arm| !arm.cleared)
            .map_or(&[], |arm| arm.ids.as_slice())
    }

    /// Why each armed row is on the list, where the press that armed them had
    /// a reason to give — which is `c`'s press and no other.
    ///
    /// Parallel to [`armed`](Self::armed), so a row finds its own reason by
    /// where its id stands. Empty where `ctrl+x` left the arm: those rows are
    /// the ones somebody pointed at, and the key itself is the whole reason.
    fn why(&self) -> &[String] {
        self.arming()
            .filter(|arm| arm.cleared)
            .map_or(&[], |arm| arm.why.as_slice())
    }

    /// Which armed rows the second press will leave where they are, because
    /// the tree behind them holds work no commit has.
    ///
    /// A handful of ids rather than a parallel array: most of the time it is
    /// empty, and a row asks whether it is in it.
    fn held(&self) -> &[String] {
        self.arming()
            .filter(|arm| arm.cleared)
            .map_or(&[], |arm| arm.held.as_slice())
    }

    /// Whether the press that armed them was on a heading, which is what
    /// decides how much the rows say the press after it would do: a group's
    /// second press stops the live ones under it before it forgets them all,
    /// and a row's own second press only forgets.
    fn swept(&self) -> bool {
        self.arming().is_some_and(|arm| arm.swept)
    }

    /// ctrl+x on an agent's row: one rule, whatever the row is doing. The
    /// first press stops a live agent — idle included — and arms the row,
    /// live or finished; the press inside the window is the one that forgets;
    /// a window left to lapse disarms with nothing removed.
    ///
    /// Stopping is what the key has always done to a running agent, and it
    /// costs nothing that is not on a branch: the pane goes and the record
    /// stays. Forgetting is the other kind of thing — the record goes, and the
    /// tree it was cut goes with it — so it is never what a single keystroke
    /// does, whatever state the row was in when the first one landed.
    ///
    /// The row says it rather than the line under the keys, because the row is
    /// what the cursor is on and what the second press would take away. A
    /// warning at the foot of a screen is about the view; this one is about
    /// one agent.
    fn end_or_arm(&mut self, root: &Path) {
        let Some(view) = self.list.selected() else {
            return;
        };
        // Only a row's own arm: rows a heading armed are finished by the
        // heading's second press, which stops the live ones first. A press on
        // one of them is a first press on that row.
        if !self.swept() && self.forgetting().iter().any(|id| id == view.id()) {
            self.arm = None;
            self.notice = kept_a_tree(act::forget(root, view));
            self.acted();
            return;
        }

        let id = view.id().to_string();
        match view.phase().is_terminal() {
            // The row is the whole of what the view has to say about this, so
            // whatever it was saying before makes way for it.
            true => self.notice = None,
            false => {
                let stopped = act::stop(root, view);
                let held = stopped.is_ok();
                self.notice = said(stopped);
                self.acted();
                // A row the stop could not end is not one a second press may
                // clear away: the failure is on the screen instead.
                if !held {
                    return;
                }
            }
        }
        self.arm = Some(Arm {
            ids: vec![id],
            swept: false,
            heading: None,
            cleared: false,
            why: Vec::new(),
            held: Vec::new(),
            at: Instant::now(),
        });
    }

    /// ctrl+x on a heading: two presses over the whole group, and the first of
    /// them costs nothing.
    ///
    /// Every row under the heading is armed in place, whatever their states,
    /// each saying so where its summary was, and not one agent is touched. The
    /// rows are what the second press would act on, so the rows are where the
    /// warning is, and the footer asks nothing. The press inside the window is
    /// the one that does it: every live agent under the heading — idle
    /// included — is stopped the way its own row would stop it, and then they
    /// are all forgotten, each under the same worktree safety a single row
    /// gets. A window left to lapse disarms with nothing stopped and nothing
    /// removed.
    ///
    /// A row that stops itself has cost somebody one pane; a heading that
    /// stops itself can cost them every pane on the screen. So the group is
    /// held back to the press that has been warned about, which is the one
    /// that was going to be irreversible anyway.
    ///
    /// The second press lands on the heading standing over the armed rows
    /// *now*, which is not always the one that was pressed: a group dissolves
    /// as its rows change state, and an agent that ends while the window is
    /// open leaves the heading it was under with nothing to stand over. The
    /// rows are what the press was about, so the rows are what it is matched
    /// by.
    ///
    /// A row whose stop failed is not one this forgets, exactly as it would
    /// not be on its own: the failure is on the screen instead, and the rest
    /// of the group goes.
    fn sweep_or_arm(&mut self, root: &Path, under: rows::Under) {
        let pressed = self.list.key(under);
        let again = self
            .arming()
            .filter(|arm| arm.swept && !arm.cleared)
            .is_some_and(|arm| arm.heading.is_some() && arm.heading == pressed);
        if again {
            let arm = self.arm.take().expect("the arm that was just read");
            // What the first press armed, as the list has it now: an agent
            // whose record has gone in the meantime is not one this can
            // stop or forget.
            let mut trouble: Vec<String> = Vec::new();
            let mut views: Vec<&View> = Vec::new();
            for view in arm.ids.iter().filter_map(|id| self.list.agent_by_id(id)) {
                if !view.phase().is_terminal()
                    && let Err(e) = act::stop(root, view)
                {
                    trouble.push(format!("{}: {e:#}", view.id()));
                    continue;
                }
                views.push(view);
            }

            let forgotten = act::forget_all(root, &views);
            // What did happen and what would not, on the one line the view
            // has, the way `forget_all` puts its own two together. Raised
            // where a stop failed, because part of what was asked for did not
            // happen.
            self.notice = match trouble.len() {
                0 => kept_a_tree(forgotten),
                stuck => Some(Notice::Failed(format!(
                    "{} · {stuck} would not stop: {}",
                    match &forgotten {
                        Ok((said, _)) => said.clone(),
                        Err(e) => format!("{e:#}"),
                    },
                    trouble[0]
                ))),
            };
            self.acted();
            return;
        }

        let ids: Vec<String> = self
            .list
            .members(under)
            .iter()
            .map(|view| view.id().to_string())
            .collect();
        if ids.is_empty() {
            return;
        }
        // The rows are the whole of what the view has to say about this, so
        // whatever it was saying before makes way for them.
        self.notice = None;
        self.arm = Some(Arm {
            ids,
            swept: true,
            heading: self.list.key(under),
            cleared: false,
            why: Vec::new(),
            held: Vec::new(),
            at: Instant::now(),
        });
    }

    /// `c` on the list: two presses over everything that has finished,
    /// wherever the cursor is standing, and the first of them costs nothing.
    ///
    /// What `amx clear` does at a shell, under the view's own law for a press
    /// that cannot be taken back. The first press asks the verb which rows are
    /// over — done, failed or stopped — and why each one is on the list, and
    /// marks them, each saying its own reason where its summary was. The press
    /// inside the window takes them the way the verb takes them: the sweep's
    /// way where the work landed, and otherwise the record and the tree amx
    /// cut, under the one law `stop` keeps about a tree holding work no commit
    /// has.
    ///
    /// Every row the wall stands for rather than every row it is drawing. A
    /// group folded to thirty and a heading somebody shut are about how much
    /// screen there is, and this press is about the fleet. A narrowing is the
    /// other way about: what it put out of reach is off this list for the same
    /// reason it is off the wall.
    ///
    /// Asked on the press and never on a reading. The question is two git
    /// calls per finished row on a branch — is it merged, has its upstream
    /// gone — and the wall is read again every second.
    fn clear_or_arm(&mut self, root: &Path) {
        let again = self.arming().is_some_and(|arm| arm.cleared);
        if again {
            let arm = self.arm.take().expect("the arm that was just read");
            self.clear(root, &arm.ids, &arm.held);
            return;
        }

        let wall: Vec<View> = self
            .list
            .items()
            .iter()
            .filter_map(|item| match item {
                rows::Item::Heading(under, _) => Some(*under),
                _ => None,
            })
            .flat_map(|under| self.list.members(under))
            .cloned()
            .collect();
        let finished = verbs::clear::finished_rows(&wall);
        if finished.is_empty() {
            self.notice = Some(Notice::Advice("nothing has finished".to_string()));
            return;
        }

        // The rows are the whole of what the view has to say about this, so
        // whatever it was saying before makes way for them.
        self.notice = None;
        let (mut ids, mut why, mut held) = (Vec::new(), Vec::new(), Vec::new());
        for (at, reason) in finished {
            let view = &wall[at];
            if verbs::clear::holding(&view.meta).is_some() {
                held.push(view.id().to_string());
            }
            ids.push(view.id().to_string());
            why.push(reason);
        }
        self.arm = Some(Arm {
            ids,
            swept: false,
            heading: None,
            cleared: true,
            why,
            held,
            at: Instant::now(),
        });
        self.acted();
    }

    /// Take what the first press marked, as the list has it now: an agent
    /// whose record has gone in the meantime is not one this can take.
    ///
    /// A row the first press found holding work no commit has is not handed to
    /// the taker at all. The taker would keep it for the same reason, but the
    /// row has been saying so for as long as the window has been open by now,
    /// and asking git to say it again is a second answer to a question already
    /// answered.
    ///
    /// What was kept is counted rather than named, in the channel for a thing
    /// that did not happen — the sentence `ctrl+x` on a heading already says
    /// about the same rows for the same reason. It named them while the press
    /// was about the handful of rows a sweep had found; a press that covers a
    /// wall can keep twenty, and twenty ids on the one line the view has is a
    /// line nobody reads to the end of.
    fn clear(&mut self, root: &Path, ids: &[String], held: &[String]) {
        let (mut cleared, mut kept) = (0, 0);
        let mut trouble = None;
        for id in ids {
            let Some(view) = self.list.agent_by_id(id) else {
                continue;
            };
            if held.iter().any(|marked| marked == id) {
                kept += 1;
                continue;
            }
            match verbs::clear::take_row(root, view) {
                Ok(verbs::clear::Taken::Gone) => cleared += 1,
                // A tree that took work on between the two presses, which the
                // taker caught and this did not, or one git would not remove.
                Ok(verbs::clear::Taken::Holding(_)) => kept += 1,
                // Said, and passed by: the rest of the list is still what the
                // second press asked for.
                Err(e) => {
                    trouble.get_or_insert_with(|| format!("{id}: {e:#}"));
                }
            }
        }

        self.notice = Some(match trouble {
            Some(e) => Notice::Failed(e),
            None => match kept {
                0 => Notice::Advice(format!("cleared {cleared}")),
                kept => Notice::Refused(format!(
                    "cleared {cleared} · kept {kept} holding work no commit has"
                )),
            },
        });
        self.acted();
    }

    /// The key a question of the view's own is waiting for.
    ///
    /// One key does it and every other key does not, which is the way round a
    /// question has to be when it is asked about something that cannot be taken
    /// back. A chord is not an answer either: it is somebody reaching for
    /// something else a beat after this opened, and what is on the other end of
    /// it is a program.
    fn answered(
        &mut self,
        key: KeyEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        let Mode::Confirming(asked) = std::mem::take(&mut self.mode) else {
            return Ok(Doing::Carry);
        };
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.mode = Mode::Confirming(asked);
            return Ok(Doing::Carry);
        }
        let yes = matches!(key.code, KeyCode::Char('y' | 'Y'));

        match asked {
            // The line comes back exactly as it was typed, because that is
            // what somebody who did not mean to press enter wants: a keystroke
            // in the middle of a task should not cost them the task.
            Asked::Slight { line, .. } if !yes => {
                self.mode = Mode::Typing(line);
                self.notice = Some(Notice::Advice("nothing was started".to_string()));
                Ok(Doing::Carry)
            }
            Asked::Slight { line, follow, .. } => self.starting(root, config, line, follow, here),
        }
    }

    /// A key while the keys themselves are on the screen. Any of them puts the
    /// agents back, because that is what somebody came here for — except the
    /// one that closes the view, which means that wherever it is pressed, and
    /// the ones that move about the document, because fifty-five keys do not
    /// fit on a terminal and a person who cannot scroll cannot ask what half
    /// of them are.
    ///
    /// Those are the keys that walk the list, and they are the same ones here:
    /// somebody who has walked a wall has already learned them. They only add
    /// and subtract, the way the card's do — the paint owns the clamp, so a
    /// press past either end lands where the press before it did.
    ///
    /// And `/`, after which every letter is the search rather than a key. That
    /// is the one state this screen has: a line that is open takes what is
    /// typed at it, esc gives every key back, and enter closes the line with
    /// the narrowing it made still standing.
    fn reading_the_keys(&mut self, key: KeyEvent) -> Doing {
        let plain = chord(key).is_empty();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let finding = self.keymap.finding();

        // The keys that move about the document, wherever the find line is: a
        // narrowing nobody could scroll would be one they could only read the
        // first screenful of. The letters among them are the exception, because
        // a line being typed at takes every letter there is.
        let page = self.keymap.page();
        let moved = match key.code {
            KeyCode::Down if plain => Some((false, 1)),
            KeyCode::Up if plain => Some((true, 1)),
            KeyCode::PageDown if plain => Some((false, page)),
            KeyCode::PageUp if plain => Some((true, page)),
            KeyCode::Char('f') if ctrl => Some((false, page)),
            KeyCode::Char('b') if ctrl => Some((true, page)),
            KeyCode::Char('d') if ctrl => Some((false, page / 2)),
            KeyCode::Char('u') if ctrl => Some((true, page / 2)),
            KeyCode::Char('j') if plain && !finding => Some((false, 1)),
            KeyCode::Char('k') if plain && !finding => Some((true, 1)),
            _ => None,
        };
        if let Some((up, by)) = moved {
            self.keymap.scrolled(up, by);
            return Doing::Carry;
        }

        if finding {
            match key.code {
                KeyCode::Char(letter) if plain => self.keymap.typed(letter),
                KeyCode::Backspace if plain => self.keymap.rubbed(),
                KeyCode::Esc if plain => self.keymap.found_nothing(),
                // The line shuts and the narrowing it made stands: somebody
                // has typed what they wanted and is about to read it.
                KeyCode::Enter if plain => self.keymap.kept(),
                _ => return self.leaving_the_keys(key),
            }
            return Doing::Carry;
        }

        match key.code {
            KeyCode::Char('/') if plain => self.keymap.find(),
            // The wall's own two ends. `gg` is the wall's spelling of the top
            // and one `g` is enough here: there is nothing on this screen for
            // a press to cost, so nothing for the second one to guard.
            KeyCode::Char('G') if plain => self.keymap.to_the_end(true),
            KeyCode::Char('g') if plain => self.keymap.to_the_end(false),
            _ => return self.leaving_the_keys(key),
        }
        Doing::Carry
    }

    /// The agents back, which is what any key that is not this screen's means
    /// — and the view closed, for the one key that means that wherever it is
    /// pressed.
    fn leaving_the_keys(&mut self, key: KeyEvent) -> Doing {
        self.mode = Mode::List;
        match key.code {
            KeyCode::Char('q') => Doing::Close,
            _ => Doing::Carry,
        }
    }

    /// The cursor has moved. A diff belongs to the agent it was taken of, so
    /// it does not follow the cursor onto the next one.
    ///
    /// The window over the list is told to come after it, which it does at the
    /// next frame and by as little as it can: a cursor still on a drawn row
    /// moves nothing. Every cursor move comes through here, which is why this
    /// is where the window hears about one — and why the wheel, which moves no
    /// cursor, does not come through here at all.
    ///
    /// The page goes with the press wherever the cursor lands, the end of the
    /// list included: the arrows retake the card, exactly as they did before
    /// there was a page to keep. The card up is put back where it opens
    /// rather than at nothing, because the press does not always take
    /// another card — an arrow at the end of the list lands on the same agent,
    /// and one onto a heading holds the card it found — and a card left at
    /// nothing is a conversation thrown back to its first words.
    fn moved(&mut self) {
        self.wall.follow.set(true);
        self.scroll
            .open_at(self.card.as_ref().map_or(0, Card::opens_at));
        if self.look == Look::Changes {
            self.look = Look::Screen;
        }
        self.follow_the_cursor();
    }

    /// The whole fleet back, whatever it was narrowed by.
    ///
    /// Both filters, not the one a find line happened to set: somebody who
    /// pressed esc to see everything again means everything, and a state
    /// narrowing left standing under a name one that just went would be a wall
    /// still short for a reason nothing on the screen still says.
    fn widen(&mut self) {
        self.list
            .narrow(vec![Narrow::State(None), Narrow::Name(None)]);
        self.follow_the_cursor();
    }

    /// One line of the wall under the pointer, the way a page scrolls.
    ///
    /// The cursor stays where somebody left it, card and all: a wheel is a
    /// look somewhere else on the list, not a move to another agent, and a
    /// wheel that took the card with it would be a hand on the mouse answering
    /// questions nobody asked. Only the paint clamps it, so a wheel past
    /// either end of the list lands on the end.
    ///
    /// The pointer's row goes, because the rows moved under it: the line it
    /// was resting on is not the line it is over now, and the next movement
    /// says which one that is.
    fn scrolled(&mut self, up: bool) {
        let top = self.wall.top.get();
        self.wall.top.set(match up {
            true => top.saturating_sub(1),
            false => top.saturating_add(1),
        });
        self.hover = None;
    }

    /// A whole page into the card's body, or back toward its natural edge.
    fn paged(&mut self, up: bool) {
        self.paged_by(up, self.scroll.page.get().max(1));
    }

    /// And half of one, which is what the paint gave the body last frame,
    /// halved. Never nothing: a key that moved no rows would read as a key
    /// that was not taken.
    fn half(&self) -> usize {
        (self.scroll.page.get() / 2).max(1)
    }

    /// The next hunk of the patch the card is holding, or the one before it.
    ///
    /// Only over a changes card: nothing else the card can hold is made of
    /// hunks, and a chord that did something on one card and nothing on
    /// another would be a key nobody could learn. Everywhere else the two are
    /// free, and this leaves them so.
    ///
    /// The line goes with the cursor. What is on it is about the hunk it was
    /// typed under, so the step leaves it there and puts back whatever was
    /// left on the hunk it reaches — a blank line leaving nothing behind, and
    /// an empty one coming back where nothing was written. A review is
    /// written a hunk at a time and read whole, and this is the whole of what
    /// keeps the two the same thing.
    // `to_` names where the cursor goes, which is what every other stepping
    // key here is named for, rather than a conversion off the screen.
    #[allow(clippy::wrong_self_convention)]
    fn to_hunk(&mut self, forward: bool) {
        if self.look != Look::Changes {
            return;
        }
        // A line that is not the card's own — a task, a rename — is not a note
        // on anything, and a step taken under one leaves it where it is.
        let words = match &self.mode {
            Mode::Typing(composer) if self.on_the_card(composer) => Some(composer.whole()),
            _ => None,
        };
        let Some(card) = &self.card else { return };
        if let Some(words) = &words {
            self.scroll.remark(self.noting(), words);
        }
        self.scroll.to_hunk(card.body.hunks(), forward);
        if words.is_some() {
            let words = self.scroll.remarked(self.noting());
            if let Mode::Typing(composer) = &mut self.mode {
                composer.at = words.chars().count();
                composer.text = words;
                // The markers stood for pastes made on the line being left, and
                // the words coming back are the words themselves.
                composer.pastes.clear();
            }
        }
    }

    /// Where the words on the line now would be kept: under the hunk the
    /// cursor is standing on, and under no hunk at all at the top of the
    /// patch, where a review's opening words are written.
    fn noting(&self) -> Option<usize> {
        self.at_hunk().map(|(at, _)| at)
    }

    /// The review as enter would send it: what has been written about the
    /// patch so far, with `words` standing as the note where the cursor is.
    ///
    /// The line is part of the review rather than a message beside it — what
    /// somebody has just typed is as much of what they think as what they
    /// typed a minute ago. Blank takes back what was kept there, by the rule
    /// [`paint::Scroll::remark`] holds for a line stepped off with nothing on
    /// it: an emptied line is a note somebody has withdrawn.
    fn review(&self, words: &str) -> BTreeMap<Option<usize>, String> {
        let mut review: BTreeMap<_, _> = self.scroll.remarks().into_iter().collect();
        let at = self.noting();
        match words.trim().is_empty() {
            true => review.remove(&at),
            false => review.insert(at, words.to_string()),
        };
        review
    }

    /// The hunks that review would carry a note on, in patch order. Its
    /// opening words are on no hunk and are no note, so an opening alone is
    /// nothing to send.
    fn noted(&self, words: &str) -> Vec<usize> {
        self.review(words).keys().copied().flatten().collect()
    }

    /// That review as one message: the opening words, then every note on a
    /// hunk the card is still showing, in patch order.
    ///
    /// A note keyed to a hunk the patch no longer has is dropped rather than
    /// sent without one. The card can be retaken under a review — the agent
    /// carries on working while it is read — and a comment whose hunk has gone
    /// is a comment pointed at nothing.
    fn written(&self, review: &BTreeMap<Option<usize>, String>) -> String {
        let hunks = self.card.as_ref().map_or(&[][..], |card| card.body.hunks());
        let notes: Vec<(&Hunk, &str)> = review
            .iter()
            .filter_map(|(at, words)| Some((hunks.get((*at)?)?, words.as_str())))
            .collect();
        let opening = review.get(&None).map_or("", String::as_str);
        act::on_hunks(opening, &notes)
    }

    /// Which hunk of the card's patch the cursor is standing on, and the hunk
    /// itself. Nothing until somebody has stepped to one.
    ///
    /// The number as well as the hunk, because both are said out loud: the row
    /// under the line names it, and the message the line sends is about it.
    /// Read against the patch the card is holding now, the way the rule over
    /// the card is — a cursor left on the twelfth hunk of a patch that has
    /// since become a shorter one is standing on nothing.
    fn at_hunk(&self) -> Option<(usize, &Hunk)> {
        let at = self.scroll.at_hunk()?;
        let hunk = self.card.as_ref()?.body.hunks().get(at)?;
        Some((at, hunk))
    }

    /// `rows` into the card's body, or back toward its natural edge.
    ///
    /// Which key leads away follows the card: a patch or a recorded answer is
    /// read down from its top, a live screen up from its bottom. The keys
    /// only add and subtract — the paint owns the clamp, so a body that fits
    /// never leaves its edge and a press past the end lands on the last page.
    fn paged_by(&mut self, up: bool, page: usize) {
        let Some(card) = &self.card else { return };
        let away = self.scroll.away.get();
        let leaving = match card.forward() {
            true => !up,
            false => up,
        };
        self.scroll.away.set(match leaving {
            true => away.saturating_add(page),
            false => away.saturating_sub(page),
        });
    }

    /// What the mouse does: the list takes it. A click on a row is enter on
    /// it — the cursor lands and the agent's window comes forward — a click
    /// on a heading or the fold keeps its toggle, the wheel scrolls the wall
    /// a line at a time — or pages the card, when the pointer is over one —
    /// and the pointer
    /// resting on a row or a heading tints it without moving the cursor,
    /// and is where `ctrl+x` is read.
    /// Nothing else is clickable, and the clicks and the wheel are the
    /// list's the way its letter keys are — the card's own line aside, which
    /// takes no pointer and so takes none of it. A task or a name being typed
    /// and a question of the view's own keep the keys they have.
    ///
    /// A left drag is the one thing here that is not the list's at all: it
    /// selects the cells it covers and copies them, because a program that
    /// has asked the terminal for the mouse has taken the terminal's own
    /// selection away from whoever is looking, and an id on a wall is there
    /// to be taken somewhere else. So the press only says where it landed,
    /// and the release is where the two of them are told apart: cells behind
    /// it means text to copy, and no cells means the click it always was.
    fn moused(
        &mut self,
        mouse: MouseEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        match mouse.kind {
            MouseEventKind::Moved => {
                self.hover = self.line_under(mouse.column, mouse.row).filter(|at| {
                    matches!(
                        self.list.items().get(*at),
                        Some(rows::Item::Agent(_) | rows::Item::Heading(..))
                    )
                });
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.pressed_at = Some((mouse.column, mouse.row));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(from) = self.pressed_at {
                    self.selection = Some((from, (mouse.column, mouse.row)));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.pressed_at = None;
                match self.selection.take() {
                    Some((from, to)) if from != to => self.copied(from, to),
                    _ => return self.clicked(mouse, root, config, here),
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if self.list_takes_the_mouse() =>
            {
                let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
                if self.card.is_some() && self.map.over_the_card(mouse.column, mouse.row) {
                    self.paged(up);
                } else {
                    self.scrolled(up);
                }
            }
            _ => {}
        }
        Ok(Doing::Carry)
    }

    /// What a left button that went down and came up on one cell does, which
    /// is what the press itself did until a drag had to be told from a click.
    fn clicked(
        &mut self,
        mouse: MouseEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        if !self.list_takes_the_mouse() {
            return Ok(Doing::Carry);
        }
        let Some(at) = self.line_under(mouse.column, mouse.row) else {
            return Ok(Doing::Carry);
        };
        // A click is a decision the way a key is, so whatever the view had to
        // say was about the moment before it.
        self.notice = None;
        match self.list.items().get(at) {
            // A row's click is enter, not merely the cursor: somebody
            // pointing at an agent is asking to open it.
            Some(rows::Item::Agent(_)) => {
                if self.list.land(at) {
                    self.moved();
                    return self.bring_forward(root, config, here);
                }
            }
            Some(rows::Item::Heading(..)) => {
                if self.list.land(at) {
                    self.list.shut_or_open();
                    self.follow_the_cursor();
                }
            }
            // The fold gives its rows back where it stands, and the cursor
            // stays where it was: opening history is not choosing an agent
            // from it.
            Some(rows::Item::Fold(..)) => self.list.unfold_at(at),
            _ => {}
        }
        Ok(Doing::Carry)
    }

    /// Put the text a drag covered on the clipboard of the terminal the view
    /// is drawn on, and say so.
    ///
    /// The text is read off the last frame rather than out of the list: what
    /// somebody dragged over is what they could see, wherever on the screen
    /// it was and whatever band drew it. Whether the terminal takes the
    /// sequence is the terminal's own business — there is no answer to it —
    /// so the view says it copied when it has asked, which is as much as it
    /// can honestly know.
    fn copied(&mut self, from: (u16, u16), to: (u16, u16)) {
        let text = self.map.selected(from, to);
        let _ = execute!(
            std::io::stdout(),
            Print(format!("\x1b]52;c;{}\x07", clip::base64(text.as_bytes())))
        );
        self.notice = Some(Notice::Advice("copied".to_string()));
    }

    /// Whether a click or a turn of the wheel reaches the list.
    ///
    /// Walking it, and reading a card with the line at its foot open: the
    /// mouse has no use for a line, so a card up is a card being read against
    /// the wall above it, and both are still there to be pointed at. A task
    /// or a name being typed keeps the pointer the way it keeps the keys.
    fn list_takes_the_mouse(&self) -> bool {
        matches!(self.mode, Mode::List) || self.answering().is_some()
    }

    /// The line of the list under this point, bounded to the lines there are:
    /// the band the map remembers is routinely taller than the list in it.
    fn line_under(&self, column: u16, row: u16) -> Option<usize> {
        self.map
            .line_under(column, row)
            .filter(|at| *at < self.list.items().len())
    }

    /// Something was done to an agent, so what the list says about it is a
    /// moment out of date.
    fn acted(&mut self) {
        self.read = None;
    }

    /// Keep how the list is arranged, where a key changed it.
    ///
    /// As it changes rather than as the view closes: a view whose terminal
    /// went is a view that closed, and somebody who spent a minute arranging
    /// a wall should not lose it to that.
    ///
    /// Read before written, and the change merged rather than dropped over
    /// what is there: two views open at once are two hands on one wall, and
    /// the pin this view just made should not take the pin the other made a
    /// second ago off it.
    fn keep(&mut self, changed: bool) {
        let Some(path) = self.remembering.clone().filter(|_| changed) else {
            return;
        };
        let mut remembered = Remembered::read(&path);
        let disk = remembered.arrangement.clone();
        remembered.arrangement =
            Arrangement::merged(&self.published, &self.list.arrangement(), &disk);
        remembered.vendor = self.vendor;
        // Nothing on the screen is waiting on this, and the one line the view
        // has to say things on is worth more than a failure nobody can act on.
        let _ = remembered.write(&path);
        // The merged whole is the arrangement now, here and in the file: a
        // change the other view made shows on this reading rather than the
        // next, and the next change is merged against what was written.
        self.published = remembered.arrangement.clone();
        self.viewed_at = stamped(&path);
        self.list.arrange(remembered.arrangement);
    }

    /// Take what another view has arranged, where the file moved since this
    /// view last read or wrote it.
    ///
    /// Read whole rather than merged: whatever moved, this view did not make
    /// it, and every change this view did make is already in the file. The
    /// stamp is taken before the read, the way the theme's is: a second write
    /// landing between the read and the stat would otherwise stand unseen
    /// until the one after it.
    fn adopt_the_view(&mut self) {
        let Some(path) = self.remembering.clone() else {
            return;
        };
        let now = stamped(&path);
        if now == self.viewed_at {
            return;
        }
        let remembered = Remembered::read(&path);
        self.viewed_at = now;
        self.published = remembered.arrangement.clone();
        self.list.arrange(remembered.arrangement);
        self.vendor = remembered.vendor;
        self.sent = remembered.sent;
    }

    /// Keep a line that has just been sent, for a later line to bring back.
    ///
    /// Written to the file as it happens, the way the arrangement is: the
    /// line somebody wants back is as often wanted from the next view, after
    /// this one was closed on the agent it started, as from this one.
    fn remember_line(&mut self, asking: &Asking, line: &str) {
        self.sent.remember_line(asking, line);
        let Some(path) = self.remembering.clone() else {
            return;
        };
        let mut remembered = Remembered::read(&path);
        remembered.sent = self.sent.clone();
        let _ = remembered.write(&path);
    }
}

/// What an action had to say, at the severity the writer knows it earned: what
/// an action came back with is advice, and what went wrong under it is a
/// failure. Either way it is said rather than raised, because a view that
/// closed itself because git was busy would be a poor view.
fn said(outcome: Result<String>) -> Option<Notice> {
    Some(match outcome {
        Ok(said) => Notice::Advice(said),
        Err(e) => Notice::Failed(format!("{e:#}")),
    })
}

/// The same, for a forget: what it came to, and whether it left a tree
/// standing.
///
/// A forget that kept a tree is the one thing this key can do that is not
/// what it was pressed for. Nothing went wrong — the safety worked — so it is
/// not a failure; but a row that is still there and a sentence painted like
/// "fix-login-a1b forgotten" is somebody pressing the key again to find out
/// why nothing happened.
fn kept_a_tree(outcome: Result<(String, bool)>) -> Option<Notice> {
    Some(match outcome {
        Ok((said, true)) => Notice::Refused(said),
        Ok((said, false)) => Notice::Advice(said),
        Err(e) => Notice::Failed(format!("{e:#}")),
    })
}

/// The card for one agent: what it is asking and the answers it is offering;
/// the whole conversation where its vendor keeps one, with what it is saying
/// now under that while a turn runs; or the screen it is working on, or the
/// answer it left. A command's card is what the command printed.
///
/// That last one comes before everything else, because the file it is read
/// from — see [`Agent::output_tail`] — is written for a command's record and
/// for a record whose vendor never got as far as a session: it holds what
/// either said where the pane is gone or empty. The end of it, since a card is
/// taken again every second it is open and a build's log grows all the while.
/// Nothing is cut off it: no vendor drew that pane, so there is no furniture
/// on it, and every row of it is the row's own. While the command runs the
/// card is the end of the file and follows what lands there; once it has ended
/// the card opens on the top of what it has and pages down.
///
/// The conversation comes first wherever the record names a transcript amx
/// can read — see [`crate::conversation`] — because it is the agent's own
/// words, whole, where a pane is one screen of them and a recorded answer is
/// one turn. It is drawn into rows here, wrapped to the width the card has,
/// with the vendor's own markdown rendered rather than shown. A turn still
/// running ends on a live tail: what the vendor streams to the record, where
/// it streams anything, and where it does not the card is the record alone —
/// the pane is never read under a record, see [`Body::conversation`].
/// A working agent whose transcript has nothing on it yet has its first turn
/// about to land, and stands its task in the conversation's place until it
/// does — see [`conversation_of`].
///
/// The screen is captured with its paint kept, because the card shows the
/// pane as the vendor drew it: bold where claude went bold, coloured where it
/// coloured. What comes back is escape sequences, and the one thing allowed to
/// read them is the walk in [`crate::ansi`], which consumes every one.
///
/// That walk happens here, where the card is made, and the card carries what
/// it gave back. So the record's own words are read where they lie rather than
/// copied first: a card is taken again on every pass a question is up for, and
/// a copy nothing would draw is work for nobody.
///
/// What the card was read from comes back beside it — see [`Freshness`] — so
/// that the pass which asks for it next can tell whether all of that would
/// come to the same card.
fn card_of(
    view: &View,
    root: &Path,
    width: u16,
    theme: Theme,
    heard: &mut Heard,
) -> (Card<Body>, Freshness) {
    let agent = Agent::open(root, view.id()).ok();
    // Whether the line at the foot of the card would reach anybody, read
    // through the door that would refuse it.
    let listening = act::listening(root, view);
    let card = |body: Body, answer: bool, queued: Vec<String>| Card {
        id: view.id().to_string(),
        phase: view.phase(),
        question: view.state.question.clone(),
        options: view.state.options.clone(),
        walked: view.state.walked,
        kind: view.kind(),
        body,
        changes: false,
        answer,
        listening,
        queued,
    };
    // What the command printed, kept beside the record by its own boot. An
    // empty file is an empty card: the command has printed nothing yet, and a
    // capture of the pane in its place would be a screen of somebody else's
    // program with the fallback vendor's anchors held against it.
    let printed = agent
        .as_ref()
        .map(|agent| as_read(agent.dir().join(crate::store::OUTPUT)));
    if let Some(said) = agent.as_ref().and_then(Agent::output_tail) {
        // A command still printing is read up from its live edge; one that
        // has ended is read forward, because what it printed is all there and
        // the start of it is where a reader begins.
        return (
            card(Body::said(&said), view.phase().is_terminal(), Vec::new()),
            Freshness::Files(printed.into_iter().collect()),
        );
    }

    let server = Server::from_socket(view.meta.socket.clone());
    // A card holding a question is the question block and nothing else, so
    // there is no capture to take for it. The waiting agent whose question amx
    // has not read still gets one, because the pane is the one place that
    // question is written at all.
    let asks = view.phase() == Phase::Waiting && view.state.question.is_some();

    let working = view.phase() == Phase::Working;
    // The log the queued band is read from, so a send or a submission moves
    // the card.
    let log = agent
        .as_ref()
        .filter(|_| working)
        .map(|agent| as_read(agent.events_path()));
    let recorded = view.meta.transcript.clone().map(as_read);
    // The stream is read for as long as a turn runs, and for no other agent.
    let streaming = agent
        .as_ref()
        .filter(|_| working)
        .map(|agent| as_read(agent.dir().join(crate::store::LIVE)));

    // What was sent to it and not yet taken, which only a turn under way
    // holds: the vendor keeps it behind the turn and draws it in the band the
    // card cuts off, so the record is where the card reads it from.
    let queued = match (&agent, working) {
        (Some(agent), true) => agent
            .events()
            .map(|events| verbs::send::still_queued(&view.meta, &events))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if !asks && let Some(said) = conversation_of(&view.meta, working, heard) {
        // What it is saying now, under the record: the vendor's own stream
        // where there is one, and only while a turn runs — a finished turn's
        // words are all on the record already. Where the vendor streams
        // nothing the record is the whole card: its calls land as they are
        // issued and its answers as each message ends, and the row over the
        // card says what it is doing between them.
        let live = working
            .then(|| agent.as_ref().and_then(Agent::live))
            .flatten();
        // A conversation still being added to is read up from its live edge;
        // one whose turn is over reads forward from its last answer.
        return (
            card(
                Body::conversation(&said, live.as_deref(), width, theme),
                !working,
                queued,
            ),
            Freshness::Files(recorded.into_iter().chain(streaming).chain(log).collect()),
        );
    }

    // An agent whose turn is over and whose record holds its answer — idle at
    // its prompt, done, failed or stopped alike — shows that whole answer.
    // The pane is not consulted: it is a viewport claude scrolls on its own,
    // and a capture of it is a few chrome-cut lines of wherever that viewport
    // happens to stand. Only a working agent's card is the pane's picture,
    // and an idle one with nothing recorded falls back to it.
    // Where its row is drawn is nobody's business here — a card is about the
    // one agent, so the group is read off the state alone.
    let answered = rows::Group::of(view.phase(), false, false, false) == rows::Group::Completed
        && view.state.result.is_some();
    let screen = (!asks && !answered && !view.phase().is_terminal())
        .then(|| server.capture_painted(&view.meta.pane).ok())
        .flatten()
        // Emptiness is a question about the words, and a screen can carry
        // paint over none of them.
        .filter(|screen| !crate::ansi::strip_ansi(screen).trim().is_empty());

    // No falling back to the answer a finished turn left, either: a card that
    // is asking shows nothing older than the question.
    let body = match (asks, answered) {
        (true, _) => Body::none(),
        (_, true) => Body::said(view.state.result.as_deref().unwrap_or_default()),
        _ => {
            let said = screen
                .as_deref()
                .or(view.state.result.as_deref())
                .unwrap_or_default();
            // An agent whose command has ended has no pane left to hold the
            // vendor's furniture, so nothing is cut off what it left. A live
            // pane is cut with the anchors of the vendor the record says was
            // started in it: every one of them is that vendor's own, and
            // claude's find nothing on a pi screen.
            match view.phase().is_terminal() {
                true => Body::said(said),
                false => Body::screen(own_chrome(&view.meta), said),
            }
        }
    };
    (
        card(body, answered, queued),
        // Every card that reaches here is a question, a capture, or the words
        // the record itself holds — and the record is read again on the wall's
        // own cadence, which is the cadence these were taken at before there
        // was anything to keep.
        Freshness::Pane,
    )
}

/// The conversation on the record's transcript, where the record names one
/// amx can read.
///
/// Read by the shape the record's own vendor writes, the way `amx logs` reads
/// it, and none at all from a vendor that keeps no conversation — a record
/// only ever names a transcript its vendor announced, and a vendor with no
/// shape to read one by has announced nothing.
///
/// A named file that is missing, or there and saying nothing, is a vendor
/// that has been started and has not written its first turn down yet. A
/// working agent is handed the task it was given in its place, as the prompt
/// it is: that is the conversation as far as it has gone, and a card that
/// showed a pane for those seconds and a conversation after them would change
/// shape under whoever opened it. Only a working one — an agent that is
/// waiting, or whose turn is over, has nothing about to land behind the empty
/// file, and what it has to show is elsewhere.
fn conversation_of<'a>(
    meta: &crate::store::Meta,
    working: bool,
    heard: &'a mut Heard,
) -> Option<Cow<'a, [crate::conversation::Said]>> {
    let path = meta.transcript.as_ref()?;
    let format = crate::conversation::format_of(meta.agent.as_deref().unwrap_or_default())?;
    let said = heard.of(path, format);
    if said.is_empty() {
        return working
            .then(|| Cow::Owned(vec![crate::conversation::Said::Prompt(meta.task.clone())]));
    }
    Some(Cow::Borrowed(said))
}

/// A transcript already read into what was said, kept so that the next card
/// on the same file reads only what changed.
///
/// A long session's transcript runs to tens of megabytes and parsing it whole
/// takes hundreds of milliseconds, while a working agent's card is taken again
/// every second. claude and codex only append whole lines, so their files are
/// read on from where the last read stopped. A pi reading depends on the
/// file's last entry and opencode's file is rewritten, so those are read whole,
/// and only when the file has changed.
#[derive(Default)]
struct Heard {
    path: PathBuf,
    /// Device, inode, length and mtime of the file at the last read.
    stamp: Option<(u64, u64, u64, Option<SystemTime>)>,
    /// Bytes read into `said`, through the end of the last whole line.
    read: u64,
    said: Vec<crate::conversation::Said>,
}

impl Heard {
    /// Everything said in the transcript at `path`, or nothing where it
    /// cannot be read.
    fn of(&mut self, path: &Path, format: Transcript) -> &[crate::conversation::Said] {
        use std::io::{Read, Seek, SeekFrom};
        use std::os::unix::fs::MetadataExt;

        let opened = std::fs::File::open(path).and_then(|file| {
            let about = file.metadata()?;
            let stamp = (about.dev(), about.ino(), about.len(), about.modified().ok());
            Ok((file, stamp))
        });
        let Ok((mut file, stamp)) = opened else {
            *self = Heard::default();
            return &self.said;
        };
        let same = self.path == path
            && self
                .stamp
                .is_some_and(|(dev, ino, ..)| (dev, ino) == (stamp.0, stamp.1));
        if same && self.stamp == Some(stamp) {
            return &self.said;
        }
        let appends = matches!(format, Transcript::Claude | Transcript::Codex);
        if !(same && appends && stamp.2 >= self.read) {
            *self = Heard {
                path: path.to_path_buf(),
                ..Heard::default()
            };
        }
        let mut bytes = Vec::new();
        if file
            .seek(SeekFrom::Start(self.read))
            .and_then(|_| file.read_to_end(&mut bytes))
            .is_err()
        {
            *self = Heard::default();
            return &self.said;
        }
        // A line still being written is left for the next read.
        let whole = match appends {
            true => bytes
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |at| at + 1),
            false => bytes.len(),
        };
        let text = String::from_utf8_lossy(&bytes[..whole]);
        self.said.extend(crate::conversation::read(format, &text));
        self.read += whole as u64;
        self.stamp = Some(stamp);
        &self.said
    }
}

/// The chrome the vendor in this agent's pane draws under it.
///
/// Off the record, which is where the command that resolved the vendor is
/// written down: the flag and the config it came from are gone by the time
/// anybody opens a card. A record naming no command reads the vendor amx falls
/// back to, and its pane keeps the reading it has always had.
fn own_chrome(meta: &crate::store::Meta) -> &'static crate::furniture::Furniture {
    crate::rules::of(meta.agent.as_deref().unwrap_or_default()).furniture()
}

/// The tmux the view is itself running in.
///
/// Read once, when the view opens: it is the terminal's, and the terminal does
/// not change hands while somebody is looking at it.
struct Here {
    /// The tmux server's own pid, which is what says whether an agent is on
    /// *this* server. One server can be addressed by two sockets, so the
    /// sockets themselves cannot be compared.
    pid: String,
    /// The session the view's own pane is in.
    session: Option<SessionId>,
}

impl Here {
    /// The tmux this process is inside, if it is inside one.
    fn read() -> Option<Here> {
        let inside = std::env::var("TMUX").ok().filter(|v| !v.is_empty())?;
        let server = Server::from_tmux_env(&inside)?;
        // `<socket path>,<server pid>,<session index>`.
        let pid = inside.split(',').nth(1)?.to_string();

        let session = std::env::var("TMUX_PANE")
            .ok()
            .and_then(|pane| PaneId::new(pane).ok())
            .and_then(|pane| server.pane_field(&pane, "#{session_id}").ok())
            .and_then(|id| SessionId::new(id).ok());
        Some(Here { pid, session })
    }
}

/// What pressing enter on a row comes to.
enum Reach {
    /// The agent is in front of them, and there is nothing to say about it.
    There,
    /// It is not, and this says why, and how to reach it where there is a way.
    Say(Notice),
    /// The terminal is the view's own to give, and this is the session that
    /// takes it.
    Lend(Server, SessionId),
}

/// Put the agent in front of whoever is looking at the view, bringing it back
/// first where there is no pane of its own left to put in front of them.
///
/// Enter on a row is somebody asking to look at this agent, and the same key
/// answers that whether or not the pane it was in is still there — or is
/// another agent's now, which is the same thing and would put somebody else's
/// work in front of them under this name. An agent with a session behind it is
/// picked up into a fresh pane and shown, which is what `amx attach` does at a
/// shell prompt. Only one with nothing to continue is refused, and then in the
/// words that say which is missing.
fn reach(root: &Path, config: &Config, here: Option<&Here>, view: &View) -> Result<Reach> {
    let server = Server::from_socket(view.meta.socket.clone());
    if server.pane_answers_for(&view.meta.pane, view.id()) {
        return Ok(noted(root, view.id(), reaching(server, here, view)?));
    }

    let env = spawn::env_snapshot(std::env::vars());
    match verbs::resume::again(root, config, view.id(), &env)? {
        Comeback::No(why) => Ok(Reach::Say(Notice::Refused(why))),
        // The pane the record named a moment ago is not the pane it names now,
        // and where the agent is is the whole of what the rest of this is
        // about, so the record is read again rather than argued with.
        Comeback::Back => {
            let back = derive::view(root, view.id(), now())?;
            let server = Server::from_socket(back.meta.socket.clone());
            Ok(noted(root, back.id(), reaching(server, here, &back)?))
        }
    }
}

/// Write the agent down as where whoever is reading went, where they went.
///
/// The trail `amx attach --last` goes back along, and the view is the other
/// door onto the same thing: somebody who pressed enter on a row is in that
/// agent as surely as if they had typed the verb. A refusal is nowhere they
/// went, so it leaves no mark.
fn noted(root: &Path, id: &str, reached: Reach) -> Reach {
    if matches!(reached, Reach::There | Reach::Lend(..)) {
        verbs::attach::note_visited(root, id);
    }
    reached
}

/// The half of it that is tmux: an agent in a pane, and a terminal to put it
/// in front of.
///
/// This is not `amx attach`: that verb becomes tmux and is done with it, and
/// this one has a view to hold open behind whatever happens next. Which is why
/// there are two ways through. Inside tmux the client already on the terminal
/// switches to the agent's session, and the view is left drawing in the
/// session it was in, for whoever switches back. Outside tmux the view *is*
/// the terminal, so it lends it out and waits. Either way ctrl+z inside the
/// session is the way back, and it is bound here, before anybody goes in.
///
/// Nothing it answers with is a failure: an agent this view cannot reach is
/// one somebody can still reach, and the answer says how.
fn reaching(server: Server, here: Option<&Here>, view: &View) -> Result<Reach> {
    // A client cannot be asked to switch to a session on a server it is not
    // attached to, and starting a second client inside the first is what
    // "sessions should be nested with care" is about.
    if let Some(here) = here
        && server.pane_field(&view.meta.pane, "#{pid}")? != here.pid
    {
        return Ok(Reach::Say(Notice::Refused(format!(
            "{id} is on another tmux. run `amx attach {id}` to reach it",
            id = view.id()
        ))));
    }

    server.bind_way_back()?;
    server.run(&["select-window", "-t", view.meta.pane.as_str()])?;
    server.run(&["select-pane", "-t", view.meta.pane.as_str()])?;

    let session = SessionId::new(server.pane_field(&view.meta.pane, "#{session_id}")?)
        .with_context(|| format!("finding the session {} is in", view.id()))?;

    let Some(here) = here else {
        return Ok(Reach::Lend(server, session));
    };

    // Another session on the same server: the terminal's own client goes to
    // it, by name, because a server may have several and only one of them is
    // this one.
    if let Some(ours) = &here.session
        && ours != &session
    {
        for tty in server
            .run(&["list-clients", "-t", ours.as_str(), "-F", "#{client_tty}"])?
            .lines()
        {
            server.run(&["switch-client", "-c", tty, "-t", session.as_str()])?;
        }
    }
    Ok(Reach::There)
}

#[cfg(test)]
mod tests;
