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
//! and attaching is tmux bringing the agent's own window forward — the view
//! stays where it is, in its window, for whoever comes back to it.
//!
//! The reading is taken from disk on a clock rather than pushed at the view by
//! anything: nothing amx runs stays resident, so there is nobody to push.

mod act;
mod paint;
mod rows;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use ratatui::Terminal;
use ratatui::backend::Backend;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::derive::{self, View};
use crate::store::{Phase, now};
use crate::tmux::{PaneId, Server, SessionId};
use crate::{exit, registry, rules};
use act::{Asking, Composer, Renamed, Replied, Started};
use paint::{Card, Notice};
use rows::List;

/// How often the agents are read again.
const REFRESH: Duration = Duration::from_millis(1000);

/// How long the view waits for a key before it goes round again.
const TICK: Duration = Duration::from_millis(120);

/// How long a frame of the working pulse lasts, which is the vendor's own
/// interval at 2.1.237.
const FRAME: Duration = Duration::from_millis(120);

/// What arrived from the terminal.
enum Typed {
    /// Nobody typed anything in the time given.
    Nothing,
    Key(KeyEvent),
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

/// The terminal itself.
struct Keyboard;

impl Keys for Keyboard {
    fn next(&mut self, patience: Duration) -> Typed {
        // A terminal that cannot be read is a terminal with nobody at it,
        // which is the same answer as somebody closing the view.
        match event::poll(patience) {
            Ok(false) => Typed::Nothing,
            Err(_) => Typed::Gone,
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => Typed::Key(key),
                Ok(Event::Paste(text)) => Typed::Paste(text),
                Ok(_) => Typed::Nothing,
                Err(_) => Typed::Gone,
            },
        }
    }
}

/// Whether the view goes round again.
enum Doing {
    Carry,
    Close,
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
/// vendor declares, where it will run, and the gate it will meet.
///
/// All of it prospective. Nothing here says anything about the agents already
/// running: a dial is about the agent that does not exist yet, so turning one
/// touches none of the ones that do. The profile starts at the config file
/// every time the view opens and dies with it — a launcher that drifted from
/// the file because of what somebody pressed last Tuesday would leave the file
/// saying one thing and the screen another.
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
    permission: String,
    /// Whether the next agent is cut a worktree of its own.
    worktree: bool,
    /// Where the next agent will run, as a person writes it.
    dir: String,
    /// How many agents may be running before `new` refuses another, so the
    /// gate is on the screen before it bites.
    max: usize,
}

impl Default for Profile {
    fn default() -> Profile {
        Profile::open(&Config::default(), None, None)
    }
}

impl Profile {
    /// The profile a view opens at: config's own answer for every dial, and
    /// the directory the view is being run from.
    ///
    /// A dial config asked for that this vendor would not take rests at the
    /// sentinel instead, which is the second half of the law the config loader
    /// keeps: no entry, no dial, and no value amx would have to invent.
    fn open(config: &Config, dir: Option<&Path>, home: Option<&Path>) -> Profile {
        let entry = registry::entry(&config.agent);
        Profile {
            agent: config.agent.clone(),
            configured: config.agent.clone(),
            model: effective(entry.and_then(|e| e.model), config.model.as_deref()),
            permission: effective(
                entry.and_then(|e| e.permission),
                config.permission.as_deref(),
            ),
            worktree: config.worktrees,
            dir: dir.map(|dir| rows::shorten(dir, home)).unwrap_or_default(),
            max: config.max_agents,
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
    /// A dial the new vendor will not take rests at the sentinel, which is
    /// the law the profile opens on read a second time: the row must not name
    /// a model to a vendor that would refuse it.
    fn cycle_vendor(&mut self) {
        let Some(next) = next_in(&self.vendors(), &self.agent) else {
            return;
        };
        self.agent = next;
        let (model, permission) = (self.model_dial(), self.permission_dial());
        self.model = effective(model, Some(&self.model));
        self.permission = effective(permission, Some(&self.permission));
    }

    /// A dial a vendor does not declare has nothing to offer, so its key does
    /// nothing rather than inventing a value the vendor would refuse.
    fn cycle_model(&mut self) {
        if let Some(next) = self
            .model_dial()
            .and_then(|d| next_in(d.cycle, &self.model))
        {
            self.model = next;
        }
    }

    fn cycle_permission(&mut self) {
        if let Some(next) = self
            .permission_dial()
            .and_then(|d| next_in(d.cycle, &self.permission))
        {
            self.permission = next;
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

/// The view as it stands: what was read, where the cursor is, what the keys
/// are doing, and what the view last had to say for itself.
#[derive(Default)]
struct Screen {
    list: List,
    /// What the next agent will be started with.
    profile: Profile,
    mode: Mode,
    look: Look,
    card: Option<Card>,
    notice: Option<Notice>,
    /// When the agents were last read.
    read: Option<Instant>,
    /// Which frame of the working pulse the rows are on.
    beat: usize,
    /// When that frame came up.
    stepped: Option<Instant>,
}

/// Open the view on this terminal and hold it until somebody closes it.
///
/// The terminal is asked to bracket pastes for as long as the view holds it.
/// Without that a pasted task arrives as the keys it is made of, and the first
/// newline in it is an enter: a truncated task dispatched, and the rest of the
/// lines queued up to dispatch themselves after it. It is the same law amx has
/// always sent text to an agent under, facing the other way.
pub fn run(root: &Path, config: &Config) -> Result<i32> {
    let mut terminal = ratatui::try_init().context("taking the terminal")?;
    // A terminal that declines is one amx cannot tell a paste from typing on,
    // which is what the composer did before it asked at all.
    let bracketed = execute!(std::io::stdout(), EnableBracketedPaste).is_ok();

    let outcome = watch(root, config, &mut terminal, &mut Keyboard, Here::read());

    // Whatever happened, the screen goes back the way it was found.
    if bracketed {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    }
    ratatui::restore();
    outcome
}

/// The view itself: draw what is there, act on what is typed, read again.
fn watch<B>(
    root: &Path,
    config: &Config,
    terminal: &mut Terminal<B>,
    keys: &mut impl Keys,
    here: Option<Here>,
) -> Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    // What the next agent will be started with is read once, from the config
    // this run was given and the directory it was run from: neither moves
    // while somebody is looking at the screen, and a dial they turn is theirs
    // until they close it.
    let mut screen = Screen {
        profile: Profile::open(
            config,
            std::env::current_dir().ok().as_deref(),
            std::env::home_dir().as_deref(),
        ),
        ..Screen::default()
    };

    loop {
        if screen.read.is_none_or(|at| at.elapsed() >= REFRESH) {
            screen.reread(root)?;
        }
        screen.step();
        terminal.draw(|frame| paint::draw(frame, &screen))?;

        match keys.next(TICK) {
            Typed::Nothing => {}
            Typed::Gone => return Ok(exit::OK),
            Typed::Paste(text) => screen.pasted(&text),
            Typed::Key(key) => {
                if let Doing::Close = screen.act(key, root, config, here.as_ref())? {
                    return Ok(exit::OK);
                }
            }
        }
    }
}

impl Screen {
    /// Read the agents again.
    fn reread(&mut self, root: &Path) -> Result<()> {
        self.list
            .show(derive::views(root, rules::bundled(), now())?);
        self.read = Some(Instant::now());
        self.follow_the_cursor();
        Ok(())
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

    /// Take the card again, when it is the kind that follows the cursor: what
    /// an agent is doing is what its pane is showing now.
    fn follow_the_cursor(&mut self) {
        match self.look {
            Look::Away => self.card = None,
            Look::Screen => self.card = self.list.selected().map(card_of),
            // A diff was taken when somebody asked for it, and stays as it was
            // until they ask again.
            Look::Changes => {}
        }
    }

    /// The line being typed on the card, when that is where it is going.
    ///
    /// The card holds one line and only one: the answer to the question it is
    /// showing. Anything else being typed — a task, a message to an agent that
    /// is not the one on the card — is a band of its own under it.
    fn answering(&self) -> Option<&Composer> {
        match &self.mode {
            Mode::Typing(composer) if self.on_the_card(composer) => Some(composer),
            _ => None,
        }
    }

    /// And every other line, which is the one the band under the card draws.
    fn banded(&self) -> Option<&Composer> {
        match &self.mode {
            Mode::Typing(composer) if !self.on_the_card(composer) => Some(composer),
            _ => None,
        }
    }

    /// Whether this line is the answer to the question the card is showing.
    ///
    /// Taken as an argument rather than read off the mode, because the one
    /// place it matters most is the keypress that has the composer out of the
    /// mode in its hand.
    fn on_the_card(&self, composer: &Composer) -> bool {
        match (&composer.asking, &self.card) {
            (Asking::Reply { id, .. }, Some(card)) => card.asks() && card.id == *id,
            _ => false,
        }
    }

    /// Open the card on the agent under the cursor, with the line to answer it
    /// on where it is asking something.
    ///
    /// The line comes with the card because that is what the card is for: an
    /// agent that has stopped is the reason somebody came to the screen, and
    /// making them press a second key to say so would be a screen that knows
    /// what they want and waits to be asked.
    fn look_closer(&mut self) {
        self.look = Look::Screen;
        self.follow_the_cursor();
        if let Some(card) = self.card.as_ref().filter(|card| card.asks()) {
            self.mode = Mode::Typing(Composer::new(Asking::Reply {
                id: card.id.clone(),
                question: true,
            }));
        }
    }

    /// Put the card away, and the line it was holding with it.
    fn look_away(&mut self) {
        self.look = Look::Away;
        self.follow_the_cursor();
    }

    /// What one key does.
    fn act(
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
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(Doing::Close);
        }

        // The dials are about the agent that does not exist yet, so they turn
        // wherever somebody might be about to start one: walking the list, and
        // typing the task itself. Not over the keys, where every key is asked
        // to put the agents back.
        if !matches!(self.mode, Mode::Keys) && self.turned(key) {
            return Ok(Doing::Carry);
        }

        match self.mode {
            Mode::Typing(_) => self.typed(key, root, config),
            Mode::Keys => Ok(self.reading_the_keys(key)),
            Mode::List => self.pressed(key, root, here),
        }
    }

    /// A dial, if this is the key that turns one.
    ///
    /// The shape of them is the vendor's: alt with the initial of what it
    /// turns, and shift+tab for the permission mode, which is the chord
    /// claude's own screens cycle it with. Each one changes what the *next*
    /// agent will be started with and nothing about the ones already running.
    fn turned(&mut self, key: KeyEvent) -> bool {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('v') if alt => self.profile.cycle_vendor(),
            KeyCode::Char('m') if alt => self.profile.cycle_model(),
            KeyCode::Char('w') if alt => self.profile.toggle_worktree(),
            // Shift+tab is a key of its own where a terminal has one, and tab
            // with shift held where it does not.
            KeyCode::BackTab => self.profile.cycle_permission(),
            KeyCode::Tab if shift => self.profile.cycle_permission(),
            _ => return false,
        }
        true
    }

    /// A key on the list.
    fn pressed(&mut self, key: KeyEvent, root: &Path, here: Option<&Here>) -> Result<Doing> {
        match key.code {
            KeyCode::Char('q') => return Ok(Doing::Close),
            KeyCode::Down => {
                self.list.down();
                self.moved();
            }
            KeyCode::Up => {
                self.list.up();
                self.moved();
            }
            KeyCode::Char(' ') => match self.look {
                Look::Away => self.look_closer(),
                _ => self.look_away(),
            },
            KeyCode::Esc => self.look_away(),
            // The same key, read where the cursor is: a heading opens and
            // shuts the group under it, the fold gives back what it is holding,
            // and a row brings its agent forward.
            KeyCode::Enter | KeyCode::Right => {
                if self.list.on_heading() {
                    self.list.shut_or_open();
                    self.follow_the_cursor();
                } else if self.list.on_fold() {
                    self.list.unfold();
                } else if let Some(view) = self.list.selected() {
                    self.notice = attach(here, view)?;
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Keys,
            KeyCode::Char('n') => self.mode = Mode::Typing(Composer::new(Asking::Task)),
            // A name for the agent under the cursor, opened on the one it is
            // going by: what somebody wants is usually a word of the current
            // name, and a line that started empty would have them type the
            // part they were keeping.
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(view) = self.list.selected() {
                    let mut composer = Composer::new(Asking::Name {
                        id: view.id().to_string(),
                    });
                    composer.text = rows::called(view).to_string();
                    self.mode = Mode::Typing(composer);
                }
            }
            // What a reply is depends on what the agent is doing: an agent
            // that has stopped on a question is answered on the card, where
            // the choices it is offering are, and anything else is a message
            // on a line of its own.
            KeyCode::Char('r') => {
                let asking = self
                    .list
                    .selected()
                    .map(|view| (view.id().to_string(), view.phase() == Phase::Waiting));
                match asking {
                    Some((_, true)) => self.look_closer(),
                    Some((id, false)) => {
                        self.mode = Mode::Typing(Composer::new(Asking::Reply {
                            id,
                            question: false,
                        }));
                    }
                    None => {}
                }
            }
            KeyCode::Char('d') => {
                if let Some(view) = self.list.selected() {
                    match act::changes(root, view) {
                        Ok(card) => {
                            self.card = Some(card);
                            self.look = Look::Changes;
                        }
                        Err(e) => self.notice = Some(Notice::Failed(format!("{e:#}"))),
                    }
                }
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(view) = self.list.selected() {
                    self.notice = said(act::end(root, view));
                    self.acted();
                }
            }
            // The same agents, gathered the other way.
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.list.turn();
                self.follow_the_cursor();
            }
            _ => {}
        }
        Ok(Doing::Carry)
    }

    /// Text arriving in one piece, which is a paste.
    ///
    /// It goes into the line verbatim, every newline in it included, and waits
    /// there: a paste is one edit, and what dispatches it is the enter pressed
    /// afterwards. Its own trailing newline is text like any other.
    ///
    /// Pasted at the list it opens a task line rather than being read as keys.
    /// A wall of agents whose keys stop things and forget things is no place to
    /// replay somebody's clipboard, and a person who pasted a task at the view
    /// meant it for the one thing here that takes text.
    fn pasted(&mut self, text: &str) {
        // Whatever the view had to say, it was about the last thing that
        // happened, and this is another one.
        self.notice = None;

        // A terminal that ends its lines the other way is still ending lines.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match &mut self.mode {
            Mode::Typing(composer) => composer.text.push_str(&text),
            _ => {
                let mut composer = Composer::new(Asking::Task);
                composer.text = text;
                self.mode = Mode::Typing(composer);
            }
        }
    }

    /// A key while somebody is typing a line.
    ///
    /// The composer is taken out of the mode for the length of the keypress and
    /// put back unless the key was the end of it, which is what makes entering
    /// and cancelling the same one move: the line is gone either way. A line
    /// that was entered and refused was not the end of it, so it comes back.
    fn typed(&mut self, key: KeyEvent, root: &Path, config: &Config) -> Result<Doing> {
        let Mode::Typing(mut composer) = std::mem::take(&mut self.mode) else {
            return Ok(Doing::Carry);
        };

        match key.code {
            // Cancelled: the line goes, and nothing was done with it. The card
            // that was holding it goes too — it and the line are one thing, so
            // one key is what closes them.
            KeyCode::Esc => {
                if self.on_the_card(&composer) {
                    self.look_away();
                }
                return Ok(Doing::Carry);
            }
            // A newline in the line rather than the end of it. The one key
            // that grows the composer by hand, and the one enter that does not
            // dispatch: a composer where the plain one did not would be a
            // composer nobody could send from.
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                composer.text.push('\n');
            }
            KeyCode::Enter => {
                // A line of nothing but filter tokens narrows the list, and
                // starts nothing: the composer is where a person is already
                // typing, so it is where they say which agents they want to
                // see.
                if let Asking::Task = composer.asking
                    && let Some(narrowing) = act::narrowing(&composer.text)
                {
                    self.list.narrow(narrowing);
                    self.follow_the_cursor();
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
                            self.notice = Some(Notice::Advice(why));
                            self.mode = Mode::Typing(composer);
                        }
                        Err(e) => {
                            self.notice = Some(Notice::Failed(format!("{e:#}")));
                            self.acted();
                        }
                    }
                    return Ok(Doing::Carry);
                }
                if composer.text.trim().is_empty() {
                    return Ok(Doing::Carry);
                }
                if let Asking::Reply { id, .. } = &composer.asking {
                    let id = id.clone();
                    match act::reply(root, &id, &composer.text) {
                        Ok(Replied::Yes(said)) => {
                            self.notice = Some(Notice::Advice(said));
                            self.acted();
                        }
                        // A line the agent would not take is a line somebody
                        // is still writing, the same as a task a dial refused:
                        // an answer retyped is an answer, and one thrown away
                        // is somebody typing it again from the start.
                        Ok(Replied::No(why)) => {
                            self.notice = Some(Notice::Advice(why));
                            self.mode = Mode::Typing(composer);
                        }
                        Err(e) => {
                            self.notice = Some(Notice::Failed(format!("{e:#}")));
                            self.acted();
                        }
                    }
                    return Ok(Doing::Carry);
                }

                // Under the dials the header is showing, which are what this
                // view says the next agent will be started with.
                let launching = self.profile.launching(config);
                match act::start(root, &launching, &composer.text, composer.hidden) {
                    Ok(Started::Yes(said)) => {
                        self.notice = Some(Notice::Advice(said));
                        self.acted();
                    }
                    // A line nothing was made from is a line somebody is still
                    // writing, so it stays where they typed it with the reason
                    // under it. A task retyped because a dial was misspelt is
                    // a task somebody types shorter the second time.
                    Ok(Started::No(why)) => {
                        self.notice = Some(Notice::Advice(why));
                        self.mode = Mode::Typing(composer);
                    }
                    Err(e) => {
                        self.notice = Some(Notice::Failed(format!("{e:#}")));
                        self.acted();
                    }
                }
                return Ok(Doing::Carry);
            }
            KeyCode::Backspace => {
                composer.text.pop();
            }
            KeyCode::Tab => composer.hidden = !composer.hidden,
            // A key held down with control or alt is somebody reaching for
            // something else, not a character they meant to type.
            KeyCode::Char(typed)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                composer.text.push(typed);
            }
            _ => {}
        }

        self.mode = Mode::Typing(composer);
        Ok(Doing::Carry)
    }

    /// A key while the keys themselves are on the screen. Any of them puts the
    /// agents back, because that is what somebody came here for — except the
    /// one that closes the view, which means that wherever it is pressed.
    fn reading_the_keys(&mut self, key: KeyEvent) -> Doing {
        self.mode = Mode::List;
        match key.code {
            KeyCode::Char('q') => Doing::Close,
            _ => Doing::Carry,
        }
    }

    /// The cursor has moved. A diff belongs to the agent it was taken of, so
    /// it does not follow the cursor onto the next one.
    fn moved(&mut self) {
        if self.look == Look::Changes {
            self.look = Look::Screen;
        }
        self.follow_the_cursor();
    }

    /// Something was done to an agent, so what the list says about it is a
    /// moment out of date.
    fn acted(&mut self) {
        self.read = None;
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

/// The card for one agent: what it is asking and the answers it is offering,
/// over the screen it is asking on — or, for an agent whose command has ended,
/// the answer it left.
/// The screen is captured with its paint kept, because the card shows the
/// pane as the vendor drew it: bold where claude went bold, coloured where it
/// coloured. What is on this string is escape sequences, and the one thing
/// allowed to read it is the walk in [`crate::ansi`], which consumes every one
/// of them.
fn card_of(view: &View) -> Card {
    let server = Server::from_socket(view.meta.socket.clone());
    let screen = (!view.phase().is_terminal())
        .then(|| server.capture_painted(&view.meta.pane).ok())
        .flatten()
        // Emptiness is a question about the words, and a screen can carry
        // paint over none of them.
        .filter(|screen| !crate::ansi::strip_ansi(screen).trim().is_empty());

    Card {
        id: view.id().to_string(),
        phase: view.phase(),
        age: view.verdict.age,
        question: view.state.question.clone(),
        options: view.state.options.clone(),
        kind: view.kind(),
        body: screen
            .or_else(|| view.state.result.clone())
            .unwrap_or_default(),
        changes: false,
    }
}

/// The tmux the view is itself running in.
///
/// Read once, when the view opens: it is the terminal's, and the terminal does
/// not change hands while somebody is looking at it.
pub struct Here {
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

/// Put the agent in front of whoever is looking at the view, and answer with
/// what to say when it could not be done.
///
/// Attaching from the view is not `amx attach`: that verb becomes tmux, and
/// this one has a view to hold open. The agent's window is brought forward on
/// its own server instead, so the view is still in its own window for whoever
/// comes back to it.
///
/// Nothing it answers with is a failure: an agent that cannot be brought
/// forward from here is one somebody can still reach, and the answer says how.
fn attach(here: Option<&Here>, view: &View) -> Result<Option<Notice>> {
    let elsewhere = format!("run `amx attach {}` to reach it", view.id());
    let Some(here) = here else {
        return Ok(Some(Notice::Advice(elsewhere)));
    };

    let server = Server::from_socket(view.meta.socket.clone());
    if !server.pane_alive(&view.meta.pane) {
        return Ok(Some(Notice::Advice(format!(
            "{} has no pane any more",
            view.id()
        ))));
    }
    if server.pane_field(&view.meta.pane, "#{pid}")? != here.pid {
        return Ok(Some(Notice::Advice(format!(
            "{} is on another tmux. {elsewhere}",
            view.id()
        ))));
    }

    server.run(&["select-window", "-t", view.meta.pane.as_str()])?;
    server.run(&["select-pane", "-t", view.meta.pane.as_str()])?;

    // Another session on the same server: the terminal's own client goes to
    // it, by name, because a server may have several and only one of them is
    // this one.
    let session = SessionId::new(server.pane_field(&view.meta.pane, "#{session_id}")?)
        .with_context(|| format!("finding the session {} is in", view.id()))?;
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
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Agent, Kind, Meta, Phase, State};
    use crate::tmux::Socket;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// What arrives from a script, and then nobody at the terminal at all.
    struct Script(std::vec::IntoIter<Typed>);

    impl Keys for Script {
        fn next(&mut self, _: Duration) -> Typed {
            self.0.next().unwrap_or(Typed::Gone)
        }
    }

    /// A key held down with control, which is how the chords are typed.
    fn ctrl(key: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)
    }

    /// A key held down with alt, which is how the dials are turned.
    fn alt(key: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(key), KeyModifiers::ALT)
    }

    /// The keys of a word, one at a time, the way somebody types it.
    fn word(text: &str) -> Vec<KeyCode> {
        text.chars().map(KeyCode::Char).collect()
    }

    /// An agent whose command ended `ago` seconds back: no pane, and nothing
    /// to ask tmux about.
    fn finished(root: &Path, id: &str, result: &str, ago: u64) {
        let at = now() - ago;
        let agent = Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx-not-a-server".to_string()),
                pane: PaneId::new("%404").unwrap(),
                session: None,
                transcript: None,
                created: at,
            },
        )
        .unwrap();
        // Written rather than recorded through a writer, because when an agent
        // ended is what orders the finished ones and a test says when.
        let state = State {
            state: Phase::Done,
            exit: Some(0),
            result: Some(result.to_string()),
            since: at,
            last_event: at,
            ..State::default()
        };
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
    }

    /// Hold the view open on a screen of this size until the script runs out,
    /// and answer with what it exited with and what was last on the screen.
    fn held(root: &Path, keys: &[KeyCode]) -> (i32, String) {
        pressing(
            root,
            keys.iter()
                .map(|code| KeyEvent::new(*code, KeyModifiers::NONE))
                .collect(),
        )
    }

    /// The same, for a script with chords in it.
    fn pressing(root: &Path, keys: Vec<KeyEvent>) -> (i32, String) {
        driving(root, keys.into_iter().map(Typed::Key).collect())
    }

    /// And the same for a script with anything else in it: a paste is not a
    /// key, and the view has to be handed one to be shown taking it.
    fn driving(root: &Path, script: Vec<Typed>) -> (i32, String) {
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let code = watch(
            root,
            &Config::default(),
            &mut terminal,
            &mut Script(script.into_iter()),
            None,
        )
        .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let screen = (0..10)
            .map(|row| {
                (0..50)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        (code, screen)
    }

    #[test]
    fn header_dials_start_at_what_the_config_file_asked_for() {
        let config = Config {
            model: Some("opus".to_string()),
            permission: Some("plan".to_string()),
            worktrees: false,
            max_agents: 3,
            ..Config::default()
        };
        let profile = Profile::open(
            &config,
            Some(Path::new("/home/dev/code/amx")),
            Some(Path::new("/home/dev")),
        );

        assert_eq!(profile.model, "opus");
        assert_eq!(profile.permission, "plan");
        assert!(!profile.worktree);
        assert_eq!(profile.max, 3);
        assert_eq!(
            profile.dir, "~/code/amx",
            "where the next one will run, written the way the headings write it"
        );
    }

    #[test]
    fn header_dials_offer_what_the_vendor_declares_and_come_back_round() {
        let mut profile = Profile::default();
        assert_eq!(
            profile.model,
            registry::DEFAULT,
            "nothing in config, so the vendor's own choice"
        );

        for want in ["fable", "opus", "sonnet", registry::DEFAULT] {
            profile.cycle_model();
            assert_eq!(profile.model, want, "claude's own cycle, in its own order");
        }

        profile.cycle_permission();
        assert_eq!(profile.permission, "acceptEdits");

        assert!(profile.worktree);
        profile.toggle_worktree();
        assert!(!profile.worktree);
    }

    #[test]
    fn header_dials_the_vendor_cycle_starts_at_the_command_config_asked_for() {
        // A vendor amx has no entry for is still what the file asked for, and
        // the cycle is the only thing that could put it back.
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };
        let mut profile = Profile::open(&config, None, None);

        profile.cycle_vendor();
        assert_eq!(profile.agent, "claude");
        assert!(
            profile.model_dial().is_some(),
            "and the dials it declares come with it"
        );

        profile.cycle_vendor();
        assert_eq!(
            profile.agent, "mock-claude",
            "and round again to the file's own answer"
        );
    }

    #[test]
    fn header_dials_a_turned_vendor_takes_the_dials_it_declares_and_no_others() {
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };
        let mut profile = Profile::open(&config, None, None);

        profile.cycle_vendor();
        profile.cycle_model();
        assert_eq!(profile.model, "fable");

        profile.cycle_vendor();
        assert_eq!(
            profile.model,
            registry::DEFAULT,
            "a vendor that declares no model dial is not started with a model"
        );
        assert_eq!(profile.launching(&config).model, None);
    }

    #[test]
    fn header_dials_the_vendor_key_leaves_a_command_it_could_not_put_back() {
        // The one registered vendor, so there is nowhere to cycle to.
        let mut profile = Profile::default();
        profile.cycle_vendor();
        assert_eq!(profile.agent, "claude");

        // And the same vendor with arguments of its own: no cycle knows what
        // they were, so the key that would drop them does nothing.
        let config = Config {
            agent: "claude --add-dir ..".to_string(),
            ..Config::default()
        };
        let mut profile = Profile::open(&config, None, None);
        profile.cycle_vendor();
        assert_eq!(profile.agent, "claude --add-dir ..");
    }

    #[test]
    fn header_dials_a_vendor_amx_never_heard_of_declares_none() {
        // The config loader clears a dial an unregistered vendor cannot take,
        // and the profile is the second half of that law: no entry, no dial,
        // and nothing for a key to turn.
        let config = Config {
            agent: "mock-claude".to_string(),
            model: Some("opus".to_string()),
            ..Config::default()
        };
        let mut profile = Profile::open(&config, None, None);

        assert!(profile.model_dial().is_none());
        assert!(profile.permission_dial().is_none());
        assert_eq!(profile.model, registry::DEFAULT);
        profile.cycle_model();
        assert_eq!(profile.model, registry::DEFAULT, "and nothing to cycle to");
    }

    #[test]
    fn header_dials_are_the_config_the_next_spawn_is_made_under() {
        let config = Config {
            max_agents: 4,
            ..Config::default()
        };
        let mut profile = Profile::open(&config, None, None);

        let resting = profile.launching(&config);
        assert_eq!(
            resting.model, None,
            "the sentinel is said by holding no value, because there is no \
             flag that means what the vendor was going to do anyway"
        );
        assert_eq!(resting.permission, None);
        assert!(resting.worktrees);
        assert_eq!(
            resting.max_agents, 4,
            "and the rest of the file is what it was"
        );

        profile.cycle_model();
        profile.cycle_permission();
        profile.toggle_worktree();

        let turned = profile.launching(&config);
        assert_eq!(turned.model.as_deref(), Some("fable"));
        assert_eq!(turned.permission.as_deref(), Some("acceptEdits"));
        assert!(!turned.worktrees, "whichever way the file had it");
    }

    #[test]
    fn header_dials_turn_under_the_keys_that_say_so() {
        let root = TempDir::new().unwrap();
        let config = Config {
            agent: "mock-claude".to_string(),
            ..Config::default()
        };
        let mut screen = Screen {
            profile: Profile::open(&config, None, None),
            ..Screen::default()
        };
        let press = |screen: &mut Screen, key| {
            screen.act(key, root.path(), &config, None).unwrap();
        };

        press(&mut screen, alt('v'));
        assert_eq!(screen.profile.agent, "claude");
        press(&mut screen, alt('m'));
        assert_eq!(screen.profile.model, "fable");
        press(
            &mut screen,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        );
        assert_eq!(screen.profile.permission, "acceptEdits");
        press(&mut screen, alt('w'));
        assert!(!screen.profile.worktree);

        // And they are live while somebody is typing a task, because that is
        // the moment before the spawn they are about.
        screen.mode = Mode::Typing(Composer::new(Asking::Task));
        press(&mut screen, alt('m'));
        assert_eq!(screen.profile.model, "opus");
        press(
            &mut screen,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
        );
        assert_eq!(screen.profile.permission, "auto");

        press(&mut screen, KeyEvent::from(KeyCode::Tab));
        press(&mut screen, KeyEvent::from(KeyCode::Char('m')));
        let Mode::Typing(composer) = &screen.mode else {
            panic!("still typing")
        };
        assert_eq!(composer.text, "m", "a letter without the chord is a letter");
        assert!(composer.hidden, "and tab on its own is still the other key");
    }

    /// A reading of an agent, as a reader hands one to the view.
    fn reading(id: &str, phase: Phase, state: State) -> View {
        View::new(
            Meta {
                id: id.to_string(),
                task: "port the importer".to_string(),
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx-not-a-server".to_string()),
                pane: PaneId::new("%404").unwrap(),
                session: None,
                transcript: None,
                created: 1,
            },
            state,
            Verdict {
                phase,
                evidence: Evidence::Hooks,
                rule: None,
                age: 29,
            },
        )
    }

    /// One that has stopped on a question of the vendor's own, which is the
    /// question that takes words as well as a key.
    fn stopped_on_a_question(id: &str) -> View {
        reading(
            id,
            Phase::Waiting,
            State {
                state: Phase::Waiting,
                question: Some("Which fixture should the port keep?".to_string()),
                options: vec!["the sqlite one".to_string(), "the docker one".to_string()],
                kind: Some(Kind::Question),
                since: 1,
                last_event: 1,
                ..State::default()
            },
        )
    }

    /// The view showing these agents, with the cursor where it opens.
    fn watching(views: Vec<View>) -> Screen {
        let mut screen = Screen::default();
        screen.list.show(views);
        screen
    }

    #[test]
    fn card_opens_on_the_question_under_the_cursor_with_the_line_to_answer_it() {
        let root = TempDir::new().unwrap();
        let config = Config::default();
        let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
        let press = |screen: &mut Screen, code| {
            screen
                .act(KeyEvent::from(code), root.path(), &config, None)
                .unwrap();
        };

        press(&mut screen, KeyCode::Char(' '));
        assert!(
            screen.card.as_ref().is_some_and(Card::asks),
            "the card is open on what the agent is asking"
        );
        assert!(
            screen.answering().is_some_and(|line| line.text.is_empty()),
            "with the line to answer it on, and nothing typed at it yet"
        );

        // A digit fills the line rather than answering with the first key
        // pressed: the same card takes words, and a menu that pressed its own
        // digits out from under an answer would answer somebody else's choice.
        press(&mut screen, KeyCode::Char('2'));
        assert_eq!(screen.answering().expect("still typing").text, "2");

        // And one key closes the line and the card it was typed on together.
        press(&mut screen, KeyCode::Esc);
        assert!(screen.card.is_none());
        assert!(screen.answering().is_none());
        assert!(matches!(screen.mode, Mode::List), "back on the agents");
    }

    #[test]
    fn card_on_an_agent_that_is_asking_nothing_takes_no_answer() {
        let root = TempDir::new().unwrap();
        let config = Config::default();
        let mut screen = watching(vec![reading(
            "busy-a1b",
            Phase::Working,
            State {
                state: Phase::Working,
                summary: Some("Running Bash".to_string()),
                ..State::default()
            },
        )]);

        screen
            .act(
                KeyEvent::from(KeyCode::Char(' ')),
                root.path(),
                &config,
                None,
            )
            .unwrap();
        assert!(
            screen.card.is_some(),
            "a closer look is still a closer look"
        );
        assert!(
            screen.answering().is_none(),
            "but there is nothing to answer, so nothing is asking for one"
        );
        assert!(
            matches!(screen.mode, Mode::List),
            "and the keys are the list's"
        );
    }

    #[test]
    fn card_is_where_a_reply_to_a_question_is_typed() {
        let root = TempDir::new().unwrap();
        let config = Config::default();
        let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);

        screen
            .act(
                KeyEvent::from(KeyCode::Char('r')),
                root.path(),
                &config,
                None,
            )
            .unwrap();
        assert!(
            screen.answering().is_some(),
            "the reply key opens the card the choices are on"
        );

        // An agent that is not asking anything takes a message, on a line of
        // its own under the wall.
        let mut screen = watching(vec![reading(
            "fix-login-b2c",
            Phase::Idle,
            State {
                state: Phase::Idle,
                ..State::default()
            },
        )]);
        screen
            .act(
                KeyEvent::from(KeyCode::Char('r')),
                root.path(),
                &config,
                None,
            )
            .unwrap();
        assert!(screen.card.is_none(), "with no card over the list");
        let line = screen.banded().expect("a line of its own");
        assert_eq!(line.prompt(), "message to fix-login-b2c");
    }

    #[test]
    fn acts_ctrl_r_opens_the_line_on_the_name_the_row_is_carrying() {
        let root = TempDir::new().unwrap();
        let config = Config::default();
        let mut screen = watching(vec![reading(
            "fix-login-a1b",
            Phase::Idle,
            State {
                state: Phase::Idle,
                name: Some("auth".to_string()),
                ..State::default()
            },
        )]);

        screen.act(ctrl('r'), root.path(), &config, None).unwrap();
        let line = screen.banded().expect("a line of its own");
        assert_eq!(line.prompt(), "rename fix-login-a1b");
        assert_eq!(
            line.text, "auth",
            "seeded with what the row says, because a rename is an edit of it \
             rather than a name typed again from nothing"
        );
    }

    #[test]
    fn glyphs_and_notices_take_their_severity_from_the_writer() {
        assert!(
            matches!(
                said(Ok("started fix-login-a1b".into())),
                Some(Notice::Advice(_))
            ),
            "what an action came back with is advice, whatever it says"
        );
        assert!(
            matches!(
                said(Err(anyhow::anyhow!("git is busy"))),
                Some(Notice::Failed(_))
            ),
            "and what went wrong is a failure"
        );
    }

    #[test]
    fn axis_turns_under_the_key_that_says_so() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);

        let (code, screen) = pressing(
            root.path(),
            vec![ctrl('s'), KeyEvent::from(KeyCode::Char('q'))],
        );
        assert_eq!(code, exit::OK);
        let drawn: Vec<&str> = screen.lines().map(str::trim_end).collect();
        assert_eq!(
            drawn[2], "/srv/app",
            "the heading is where the agent is, not what it needs:\n{screen}"
        );
        assert!(
            drawn[3].contains("done"),
            "and the row carries the state the heading used to say:\n{screen}"
        );
        assert!(
            drawn[1].contains("1 done"),
            "what there is does not change with the way it is laid out:\n{screen}"
        );
    }

    #[test]
    fn axis_narrows_the_list_from_the_line_a_task_is_typed_on() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);

        let mut keys = vec![KeyCode::Char('n')];
        keys.extend(word("a:second"));
        keys.push(KeyCode::Enter);
        keys.push(KeyCode::Char('q'));

        let (code, screen) = held(root.path(), &keys);
        assert_eq!(code, exit::OK);
        assert!(screen.contains("a:second"), "{screen}");
        assert!(screen.contains("second-b2c"), "{screen}");
        assert!(
            !screen.contains("first-a1b"),
            "the one that was named is the one that is left:\n{screen}"
        );
        assert!(
            crate::store::list(root.path()).unwrap().len() == 2,
            "and a line that narrows starts nothing"
        );
    }

    #[test]
    fn view_closes_when_somebody_closes_it() {
        let root = TempDir::new().unwrap();
        assert_eq!(held(root.path(), &[KeyCode::Char('q')]).0, exit::OK);
    }

    #[test]
    fn view_ends_when_there_is_nobody_at_the_terminal() {
        let root = TempDir::new().unwrap();
        let (code, screen) = held(root.path(), &[]);
        assert_eq!(code, exit::OK);
        assert!(screen.contains("no agents"), "{screen}");
    }

    #[test]
    fn view_walks_the_agents_and_peeks_at_the_one_under_the_cursor() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);

        // Down onto the older of them, and a closer look at it.
        let (code, screen) = held(
            root.path(),
            &[KeyCode::Down, KeyCode::Char(' '), KeyCode::Char('q')],
        );
        assert_eq!(code, exit::OK);
        assert!(screen.contains("  ● second-b2c"), "{screen}");
        assert!(screen.contains("second-b2c · done"), "{screen}");
        assert!(
            screen.contains("wrote the tests"),
            "an agent with no pane left is read from its record: {screen}"
        );
    }

    #[test]
    fn view_says_it_cannot_reach_an_agent_rather_than_going_quiet() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);

        let (_, screen) = held(root.path(), &[KeyCode::Enter, KeyCode::Char('q')]);
        assert!(
            screen.contains("run `amx attach first-a1b`"),
            "outside tmux there is no client to hand it to: {screen}"
        );
    }

    #[test]
    fn a_line_being_typed_has_the_keys_of_the_list_in_it() {
        // Every one of these is a key the list acts on. While somebody is
        // typing they are letters, or the composer could not be used at all.
        let root = TempDir::new().unwrap();
        let mut keys = vec![KeyCode::Char('n')];
        keys.extend(word("drop the queue and quit"));
        keys.push(KeyCode::Char('q'));

        let (code, screen) = held(root.path(), &keys);
        assert_eq!(code, exit::OK, "and the view did not close on any of them");
        assert!(screen.contains("drop the queue and quitq"), "{screen}");
    }

    #[test]
    fn composer_takes_a_pasted_task_as_one_edit_and_dispatches_none_of_it() {
        let root = TempDir::new().unwrap();
        let (code, screen) = driving(
            root.path(),
            vec![Typed::Paste(
                "port the importer\nand its tests\n".to_string(),
            )],
        );

        assert_eq!(code, exit::OK);
        assert!(screen.contains("task ▸ port the importer"), "{screen}");
        assert!(
            screen.contains("       and its tests"),
            "every line of it is on the line being typed: {screen}"
        );
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and the newlines in it are text, not enters"
        );
    }

    #[test]
    fn composer_adds_a_paste_to_the_line_somebody_was_already_typing() {
        let root = TempDir::new().unwrap();
        let mut script = vec![Typed::Key(KeyEvent::from(KeyCode::Char('n')))];
        script.extend(
            word("port ")
                .into_iter()
                .map(|code| Typed::Key(KeyEvent::from(code))),
        );
        // A terminal that ends its lines with a carriage return is ending
        // lines, and the composer reads them as the newlines they are.
        script.push(Typed::Paste("the importer\rand its tests".to_string()));

        let (_, screen) = driving(root.path(), script);
        assert!(screen.contains("task ▸ port the importer"), "{screen}");
        assert!(screen.contains("       and its tests"), "{screen}");
    }

    #[test]
    fn composer_takes_a_newline_from_the_key_that_makes_one_and_stays_open() {
        let root = TempDir::new().unwrap();
        let mut keys = vec![KeyEvent::from(KeyCode::Char('n'))];
        keys.extend(word("port the importer").into_iter().map(KeyEvent::from));
        keys.push(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        keys.extend(word("and its tests").into_iter().map(KeyEvent::from));

        let (code, screen) = pressing(root.path(), keys);
        assert_eq!(code, exit::OK);
        assert!(screen.contains("task ▸ port the importer"), "{screen}");
        assert!(screen.contains("       and its tests"), "{screen}");
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and the enter that makes a newline is the one that starts nothing"
        );
    }

    #[test]
    fn composer_keeps_a_line_a_dial_refused_where_it_was_typed() {
        let root = TempDir::new().unwrap();
        let mut keys = vec![KeyCode::Char('n')];
        keys.extend(word("p:nonsense port it"));
        keys.push(KeyCode::Enter);

        let (code, screen) = held(root.path(), &keys);
        assert_eq!(code, exit::OK);
        assert!(
            screen.contains("task ▸ p:nonsense port it"),
            "a line nothing was made from is a line somebody is still \
             writing: {screen}"
        );
        assert!(screen.contains("p:nonsense: claude takes"), "{screen}");
        assert!(crate::store::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn a_line_nobody_entered_does_nothing_at_all() {
        let root = TempDir::new().unwrap();
        let mut keys = vec![KeyCode::Char('n')];
        keys.extend(word("port the importer"));
        keys.push(KeyCode::Esc);
        keys.push(KeyCode::Char('q'));

        let (code, screen) = held(root.path(), &keys);
        assert_eq!(code, exit::OK, "and q is the list's key again");
        assert!(
            !screen.contains("port the importer"),
            "the line is gone with it: {screen}"
        );
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and nothing was started"
        );
    }

    #[test]
    fn the_keys_are_on_the_screen_for_the_asking() {
        let root = TempDir::new().unwrap();
        let (_, screen) = held(root.path(), &[KeyCode::Char('?'), KeyCode::Char('q')]);
        assert!(screen.contains("ctrl+x"), "{screen}");
        assert!(screen.contains("stop it"), "{screen}");
    }
}
