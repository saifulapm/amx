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
mod paint;
mod rows;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{EnterAlternateScreen, SetTitle, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::Backend;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::derive::{self, View};
use crate::store::{Phase, now};
use crate::tmux::{PaneId, Server, SessionId};
use crate::verbs::ls::Scope;
use crate::verbs::resume::Comeback;
use crate::{exit, registry, rules, spawn, verbs};
use act::{Asking, Composer, Edited, Renamed, Replied, Started};
use paint::{Card, Notice};
use rows::{Arrangement, List};

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
const ARMED: Duration = Duration::from_secs(2);

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
    /// Lend the terminal to a tmux client on this agent's session, and go
    /// round again when it is handed back.
    Lend {
        id: String,
        on: Server,
        session: SessionId,
    },
    /// Lend it to an editor for as long as somebody is writing the line in it.
    Edit,
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

/// Which of the two chords a key arrived under, if either.
///
/// Shift is not one of them. A terminal says shift by sending the character it
/// typed, so asking about it would be asking about the keyboard rather than
/// about the key. The one key it can be asked about is tab, which has no
/// character of its own to arrive as.
fn chord(key: KeyEvent) -> KeyModifiers {
    key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT)
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

/// What a press has armed and a second one would forget: one finished row, or
/// every finished row under a heading, and the moment it was armed.
///
/// Held by id rather than by where the cursor was: the wall is read again
/// every second and the rows move under it, and a window that belonged to a
/// place on the screen would arm whatever had come to rest there.
struct Arm {
    /// The rows the second press forgets.
    ids: Vec<String>,
    /// The heading the first press was on, where it was on one: that is where
    /// the press that forgets them all has to land again.
    heading: Option<rows::Under>,
    at: Instant,
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
    /// The finished row a press has armed, where one is armed.
    arm: Option<Arm>,
    /// When the agents were last read.
    read: Option<Instant>,
    /// Where what somebody arranges is kept, where there is anywhere to keep
    /// it.
    remembering: Option<PathBuf>,
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
pub fn run(root: &Path, config: &Config, scope: &Scope) -> Result<i32> {
    let mut terminal = ratatui::try_init().context("taking the terminal")?;
    // A terminal that declines is one amx cannot tell a paste from typing on,
    // which is what the composer did before it asked at all.
    let bracketed = execute!(std::io::stdout(), EnableBracketedPaste).is_ok();

    // The title the terminal came with, kept while the view has its own to
    // say, and put back below. A view that renamed somebody's window and left
    // it renamed would be leaving a count that stopped being true behind it.
    let _ = execute!(std::io::stdout(), Print(KEEP_THE_TITLE));

    let remembering = crate::paths::view_file(root);
    let outcome = watch(
        root,
        config,
        scope,
        &mut terminal,
        &mut Keyboard,
        Here::read(),
        remembering.as_deref(),
        &mut TitleBar,
    );

    // Whatever happened, the screen goes back the way it was found.
    if bracketed {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    }
    let _ = execute!(std::io::stdout(), Print(PUT_THE_TITLE_BACK));
    ratatui::restore();

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

/// What a terminal is asked with to hold on to the title it is wearing, and to
/// put it back on again.
///
/// xterm's own pair for it, which the terminals amx is drawn on speak and the
/// ones that do not ignore: an escape a terminal has never heard of costs
/// nothing, and the worst of it is a window left called `amx`.
const KEEP_THE_TITLE: &str = "\x1b[22;2t";
const PUT_THE_TITLE_BACK: &str = "\x1b[23;2t";

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
    scope: &Scope,
    terminal: &mut Terminal<B>,
    keys: &mut impl Keys,
    here: Option<Here>,
    remembering: Option<&Path>,
    titles: &mut impl Titles,
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
        remembering: remembering.map(Path::to_path_buf),
        ..Screen::default()
    };
    // The list opens the way it was left, before anything is drawn on it: a
    // view that gathered itself one way and then jumped to the other would be
    // saying the arrangement is something it does rather than something
    // somebody said.
    if let Some(path) = remembering {
        screen.list.arrange(Remembered::read(path).arrangement);
    }

    // What the terminal is called at the moment, so that it is said again when
    // it stops being true and not on every frame that did not change it.
    let mut called = String::new();

    loop {
        if screen.read.is_none_or(|at| at.elapsed() >= REFRESH) {
            screen.reread(root, scope)?;
        }
        screen.step();
        terminal.draw(|frame| paint::draw(frame, &screen))?;

        let title = paint::title(&screen.list);
        if title != called {
            titles.say(&title);
            called = title;
        }

        match keys.next(TICK) {
            Typed::Nothing => {}
            Typed::Gone => return Ok(exit::OK),
            Typed::Paste(text) => screen.pasted(&text),
            Typed::Key(key) => match screen.act(key, root, config, here.as_ref())? {
                Doing::Carry => {}
                Doing::Close => return Ok(exit::OK),
                Doing::Lend { id, on, session } => {
                    screen.notice = lend(terminal, &id, &on, &session)?;
                    // Whoever had the terminal had the title with it, so the
                    // view says what it is called again rather than trusting a
                    // name it did not put there.
                    called.clear();
                }
                Doing::Edit => {
                    edit_the_line(terminal, &mut screen)?;
                    called.clear();
                }
            },
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
fn edit_the_line<B>(terminal: &mut Terminal<B>, screen: &mut Screen) -> Result<()>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let Mode::Typing(composer) = &screen.mode else {
        return Ok(());
    };
    let text = composer.text.clone();
    let written = borrowed(terminal, || act::edited(&text))?;

    screen.notice = match written {
        Ok(Edited::Line(text)) => {
            if let Mode::Typing(composer) = &mut screen.mode {
                composer.text = text;
            }
            None
        }
        Ok(Edited::No(why)) => Some(Notice::Advice(why)),
        Err(e) => Some(Notice::Failed(format!("{e:#}"))),
    };
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
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::try_restore().context("giving the terminal up")?;

    let outcome = doing();

    enable_raw_mode().context("taking the terminal back")?;
    execute!(std::io::stdout(), EnterAlternateScreen).context("taking the terminal back")?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    terminal.clear()?;
    Ok(outcome)
}

impl Screen {
    /// Read the agents again, the ones the view was opened about.
    fn reread(&mut self, root: &Path, scope: &Scope) -> Result<()> {
        self.list
            .show(scope.narrow(derive::views(root, rules::bundled(), now())?));
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
    fn look_closer(&mut self, root: &Path) {
        // A card is a look at one agent, and a heading or the fold is not one:
        // the key does nothing there rather than leaving the view looking at
        // an agent it would find the next time the cursor moved.
        let Some(id) = self.list.selected().map(|view| view.id().to_string()) else {
            return;
        };
        self.look = Look::Screen;
        self.follow_the_cursor();
        if let Some(card) = self.card.as_ref().filter(|card| card.asks()) {
            self.mode = Mode::Typing(Composer::new(Asking::Reply {
                id: card.id.clone(),
                question: true,
            }));
        }

        // Opening the card is the whole of what the mark means, so the record
        // learns it here. A record that will not take the look costs a mark
        // that stays on a row somebody has read, which is not worth spending
        // the one line the view has to say things on.
        let _ = act::looked(root, &id);
        self.acted();
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
    /// The shape of them is the vendor's: alt with the initial of what it
    /// turns, and shift+tab for the permission mode, which is the chord
    /// claude's own screens cycle it with. Each one changes what the *next*
    /// agent will be started with and nothing about the ones already running.
    fn turned(&mut self, key: KeyEvent) -> bool {
        let alt = chord(key) == KeyModifiers::ALT;
        let plain = chord(key).is_empty();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('v') if alt => self.profile.cycle_vendor(),
            KeyCode::Char('m') if alt => self.profile.cycle_model(),
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
            KeyCode::Down if plain => {
                self.list.down();
                self.moved();
            }
            KeyCode::Up if plain => {
                self.list.up();
                self.moved();
            }
            KeyCode::Char(' ') if plain => match self.look {
                Look::Away => self.look_closer(root),
                _ => self.look_away(),
            },
            KeyCode::Esc if plain => self.look_away(),
            // The same key, read where the cursor is: a heading opens and
            // shuts the group under it, the fold gives back what it is holding,
            // and a row brings its agent forward.
            KeyCode::Enter | KeyCode::Right if plain => {
                if self.list.on_heading() {
                    self.list.shut_or_open();
                    self.follow_the_cursor();
                } else if self.list.on_fold() {
                    self.list.unfold();
                } else if let Some(view) = self.list.selected() {
                    let id = view.id().to_string();
                    let reached = reach(root, config, here, view)?;
                    // An agent that came back is in a pane this reading knows
                    // nothing about.
                    self.acted();
                    match reached {
                        Reach::There => {}
                        Reach::Say(notice) => self.notice = Some(notice),
                        Reach::Lend(on, session) => {
                            return Ok(Doing::Lend { id, on, session });
                        }
                    }
                }
            }
            // The agents by where they are on the wall, which is the number a
            // person watching one has already counted off the screen. Nine of
            // them, because there is no tenth key.
            KeyCode::Char(digit @ '1'..='9') if alt => {
                let at = digit.to_digit(10).unwrap_or_default() as usize;
                return self.reach_the_nth(at, root, config, here);
            }
            KeyCode::Char('?') if plain => self.mode = Mode::Keys,
            KeyCode::Char('n') if plain => self.mode = Mode::Typing(Composer::new(Asking::Task)),
            // The same line, opened in the editor somebody already has their
            // fingers in. A task worth a paragraph is a task worth writing
            // where writing is what the keys are for.
            KeyCode::Char('g') if ctrl => {
                self.mode = Mode::Typing(Composer::new(Asking::Task));
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
                    composer.text = rows::called(view).to_string();
                    self.mode = Mode::Typing(composer);
                }
            }
            // What a reply is depends on what the agent is doing: an agent
            // that has stopped on a question is answered on the card, where
            // the choices it is offering are, and anything else is a message
            // on a line of its own.
            KeyCode::Char('r') if plain => {
                let asking = self
                    .list
                    .selected()
                    .map(|view| (view.id().to_string(), view.phase() == Phase::Waiting));
                match asking {
                    Some((_, true)) => self.look_closer(root),
                    Some((id, false)) => {
                        self.mode = Mode::Typing(Composer::new(Asking::Reply {
                            id,
                            question: false,
                        }));
                    }
                    None => {}
                }
            }
            KeyCode::Char('d') if plain => {
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
            // The same key, read where the cursor is: on a row it is that
            // agent's ending, and on a heading it is the finished agents under
            // it, which is the one place a person is looking at a group rather
            // than at an agent.
            KeyCode::Char('x') if ctrl => match self.list.heading() {
                Some(under) => self.sweep_or_arm(root, under),
                None => self.end_or_arm(root),
            },
            // The same agents, gathered the other way.
            KeyCode::Char('s') if ctrl => {
                self.list.turn();
                self.follow_the_cursor();
                self.keep(true);
            }
            // The agent under the cursor held at the top of its group, so that
            // the one somebody is watching stays where they are looking.
            KeyCode::Char('t') if ctrl => {
                let held = self.list.hold_or_let_go();
                self.keep(held);
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

                return self.entering(root, config, composer, false, here);
            }
            // The same line entered, with whoever pressed it going along: the
            // agent is started and the terminal is put in front of it.
            //
            // On a task and nothing else. A narrowing has nothing to go to,
            // and a reply goes to an agent already on the wall, which is a row
            // away from a key that reaches one.
            KeyCode::Char('n')
                if chord(key) == KeyModifiers::ALT
                    && matches!(composer.asking, Asking::Task)
                    && !composer.narrows()
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
            KeyCode::Backspace => {
                composer.text.pop();
            }
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
        let view = match derive::view(root, id, rules::bundled(), now()) {
            Ok(view) => view,
            // Started and unreachable is worth saying and not worth closing the
            // view over: the agent is running either way, and `amx attach` is
            // still a thing somebody can type.
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                return Ok(Doing::Carry);
            }
        };

        match reach(root, config, here, &view)? {
            Reach::There => {}
            Reach::Say(notice) => self.notice = Some(notice),
            Reach::Lend(on, session) => {
                return Ok(Doing::Lend {
                    id: id.to_string(),
                    on,
                    session,
                });
            }
        }
        Ok(Doing::Carry)
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
            self.notice = Some(Notice::Advice(format!(
                "the wall has fewer than {at} agents"
            )));
            return Ok(Doing::Carry);
        };

        let id = view.id().to_string();
        let reached = reach(root, config, here, view)?;
        self.acted();
        match reached {
            Reach::There => {}
            Reach::Say(notice) => self.notice = Some(notice),
            Reach::Lend(on, session) => return Ok(Doing::Lend { id, on, session }),
        }
        Ok(Doing::Carry)
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
        if let Some(task) = act::slight(config, &composer.text) {
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
        match act::start(root, &launching, &composer.text) {
            Ok(Started::Yes { id, said }) => {
                self.notice = Some(Notice::Advice(said));
                self.acted();
                if follow {
                    return self.landing(root, config, &id, here);
                }
            }
            // A line nothing was made from is a line somebody is still
            // writing, so it stays where they typed it with the reason under
            // it. A task retyped because a dial was misspelt is a task
            // somebody types shorter the second time.
            Ok(Started::No(why)) => {
                self.notice = Some(Notice::Advice(why));
                self.mode = Mode::Typing(composer);
            }
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                self.acted();
            }
        }
        Ok(Doing::Carry)
    }

    /// The rows a press has armed, while its window is still open.
    ///
    /// Worked out from the clock every time it is asked for rather than
    /// cleared when it falls due: what closes the window is time passing, and
    /// there is nothing running in this view to do the clearing at the moment
    /// it happens.
    fn armed(&self) -> &[String] {
        self.arm
            .as_ref()
            .filter(|arm| arm.at.elapsed() < ARMED)
            .map_or(&[], |arm| arm.ids.as_slice())
    }

    /// ctrl+x on an agent's row: stop it, or arm it and then forget it.
    ///
    /// Stopping is what the key has always done to a running agent, and it
    /// costs nothing that is not on a branch: the pane goes and the record
    /// stays. Forgetting is the other kind of thing — the record goes, and the
    /// tree it was cut goes with it — so it is not what any single keystroke
    /// does. The first press on a finished row arms it and the row says so, in
    /// the place somebody is already reading; the press inside that window is
    /// the one that forgets.
    ///
    /// The row says it rather than the line under the keys, because the row is
    /// what the cursor is on and what the second press would take away. A
    /// warning at the foot of a screen is about the view; this one is about
    /// one agent.
    fn end_or_arm(&mut self, root: &Path) {
        let Some(view) = self.list.selected() else {
            return;
        };
        let armed = self.armed().iter().any(|id| id == view.id());
        if view.phase().is_terminal() && !armed {
            let id = view.id().to_string();
            // The row is the whole of what the view has to say about this, so
            // whatever it was saying before makes way for it.
            self.notice = None;
            self.arm = Some(Arm {
                ids: vec![id],
                heading: None,
                at: Instant::now(),
            });
            return;
        }

        self.arm = None;
        self.notice = said(act::end(root, view));
        self.acted();
    }

    /// ctrl+x on a heading: arm the finished agents under it, or forget them.
    ///
    /// The same two presses the key reads on a row, over a group. The first
    /// arms every finished row under the heading and each of them says so
    /// where its summary was — the rows are what the second press would take
    /// away, so the rows are where the warning is, and the footer asks
    /// nothing. The press inside the window, on the same heading, forgets
    /// them all; a window left to lapse disarms with nothing removed.
    ///
    /// Only the finished ones, wherever the heading is: an agent that is still
    /// running is not something a key that clears history may reach, and a
    /// group with nothing finished under it is told so rather than armed to no
    /// end.
    fn sweep_or_arm(&mut self, root: &Path, under: rows::Under) {
        let again = self
            .arm
            .as_ref()
            .filter(|arm| arm.at.elapsed() < ARMED)
            .is_some_and(|arm| arm.heading == Some(under));
        if again {
            let arm = self.arm.take().expect("the arm that was just read");
            // What the first press armed, as the list has it now: an agent
            // whose record has gone in the meantime is not one this can
            // forget.
            let views: Vec<&View> = arm
                .ids
                .iter()
                .filter_map(|id| self.list.agent_by_id(id))
                .collect();
            self.notice = said(act::forget_all(root, &views));
            self.acted();
            return;
        }

        let ids: Vec<String> = self
            .list
            .members(under)
            .into_iter()
            .filter(|view| view.phase().is_terminal())
            .map(|view| view.id().to_string())
            .collect();
        if ids.is_empty() {
            self.notice = Some(Notice::Advice(format!(
                "nothing under {} has finished",
                self.list.title(under)
            )));
            return;
        }

        // The rows are the whole of what the view has to say about this, so
        // whatever it was saying before makes way for them.
        self.notice = None;
        self.arm = Some(Arm {
            ids,
            heading: Some(under),
            at: Instant::now(),
        });
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

    /// Keep how the list is arranged, where a key changed it.
    ///
    /// As it changes rather than as the view closes: a view whose terminal
    /// went is a view that closed, and somebody who spent a minute arranging
    /// a wall should not lose it to that.
    ///
    /// Read before written, so the rest of the file is what it was: the offer
    /// at the foot of this one lives in it too, and two views open at once are
    /// two people arranging one wall.
    fn keep(&self, changed: bool) {
        let Some(path) = self.remembering.as_ref().filter(|_| changed) else {
            return;
        };
        let mut remembered = Remembered::read(path);
        remembered.arrangement = self.list.arrangement();
        // Nothing on the screen is waiting on this, and the one line the view
        // has to say things on is worth more than a failure nobody can act on.
        let _ = remembered.write(path);
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
/// first where there is no pane left to put in front of them.
///
/// Enter on a row is somebody asking to look at this agent, and the same key
/// answers that whether or not the pane it was in is still there: an agent
/// with a session behind it is picked up into a fresh pane and shown, which
/// is what `amx attach` does at a shell prompt. Only one with nothing to
/// continue is refused, and then in the words that say which is missing.
fn reach(root: &Path, config: &Config, here: Option<&Here>, view: &View) -> Result<Reach> {
    let server = Server::from_socket(view.meta.socket.clone());
    if server.pane_alive(&view.meta.pane) {
        return reaching(server, here, view);
    }

    let env = spawn::env_snapshot(std::env::vars());
    match verbs::resume::again(root, config, view.id(), &env)? {
        Comeback::No(why) => Ok(Reach::Say(Notice::Advice(why))),
        // The pane the record named a moment ago is not the pane it names now,
        // and where the agent is is the whole of what the rest of this is
        // about, so the record is read again rather than argued with.
        Comeback::Back => {
            let back = derive::view(root, view.id(), rules::bundled(), now())?;
            let server = Server::from_socket(back.meta.socket.clone());
            reaching(server, here, &back)
        }
    }
}

/// The half of it that is tmux: an agent in a pane, and a terminal to put it
/// in front of.
///
/// This is not `amx attach`: that verb becomes tmux and is done with it, and
/// this one has a view to hold open behind whatever happens next. Which is why
/// there are two ways through. Inside tmux the client already on the terminal
/// switches to the agent's session, and the view is left drawing in the
/// session it was in, for whoever switches back. Outside tmux the view *is*
/// the terminal, so it lends it out and waits.
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
        return Ok(Reach::Say(Notice::Advice(format!(
            "{id} is on another tmux. run `amx attach {id}` to reach it",
            id = view.id()
        ))));
    }

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

    /// What the view called the terminal, in the order it said so.
    #[derive(Default)]
    struct Said(Vec<String>);

    impl Titles for Said {
        fn say(&mut self, said: &str) {
            self.0.push(said.to_string());
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
        finished_in(root, id, result, ago, "/srv/app");
    }

    /// The same, for a test about which directory an agent worked in.
    fn finished_in(root: &Path, id: &str, result: &str, ago: u64, dir: &str) {
        let at = now() - ago;
        let agent = Agent::create(
            root,
            &Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                dir: PathBuf::from(dir),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx-not-a-server".to_string()),
                pane: PaneId::new("%404").unwrap(),
                bg: false,
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
        drawn_about(root, &Scope::default(), script, None)
    }

    /// And the same for a view opened about one directory rather than the
    /// whole machine, or one that keeps what somebody arranges.
    fn drawn_about(
        root: &Path,
        scope: &Scope,
        script: Vec<Typed>,
        remembering: Option<&Path>,
    ) -> (i32, String) {
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let code = watch(
            root,
            &Config::default(),
            scope,
            &mut terminal,
            &mut Script(script.into_iter()),
            None,
            remembering,
            &mut Said::default(),
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
        assert_eq!(
            screen.profile.permission, "auto",
            "and tab on its own is not the chord that turns the dial"
        );
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
                bg: false,
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
    fn acts_the_status_line_is_offered_once_and_never_written_anywhere() {
        let state = TempDir::new().unwrap();
        let root = state.path().join("agents");
        std::fs::create_dir_all(&root).unwrap();

        let kept = crate::paths::view_file(&root).expect("somewhere to keep it");

        let offer = offer_the_statusline(&kept).expect("the first quit offers");
        assert!(
            offer.contains("set -g status-right '#(amx statusline) | %H:%M'"),
            "it is pasted, so it is the whole line tmux takes: {offer}"
        );
        assert!(
            offer.lines().count() > 1,
            "and the line has a row to itself, to be copied off: {offer}"
        );

        assert_eq!(
            offer_the_statusline(&kept),
            None,
            "an offer that comes back every time is an advertisement"
        );
        assert_eq!(
            kept.parent(),
            Some(state.path()),
            "and it is remembered beside the agents rather than among them, \
             so the next view knows"
        );
        assert!(kept.exists());
    }

    /// The agents the list is drawing, in the order it has them.
    fn ordered(screen: &Screen) -> Vec<String> {
        screen
            .list
            .items()
            .iter()
            .filter_map(|item| screen.list.agent(*item))
            .map(|view| view.id().to_string())
            .collect()
    }

    #[test]
    fn acts_shift_with_an_arrow_moves_the_agent_rather_than_the_cursor() {
        let root = TempDir::new().unwrap();
        let working = |id: &str| {
            reading(
                id,
                Phase::Working,
                State {
                    state: Phase::Working,
                    since: 1,
                    last_event: 1,
                    ..State::default()
                },
            )
        };
        let mut screen = watching(vec![
            working("busy-a1b"),
            working("busy-b2c"),
            working("busy-c3d"),
        ]);
        let press = |screen: &mut Screen, key: KeyEvent| {
            screen
                .act(key, root.path(), &Config::default(), None)
                .unwrap();
        };
        let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);

        assert_eq!(ordered(&screen), ["busy-a1b", "busy-b2c", "busy-c3d"]);
        press(&mut screen, shift(KeyCode::Down));
        assert_eq!(ordered(&screen), ["busy-b2c", "busy-a1b", "busy-c3d"]);
        assert_eq!(
            screen.list.selected().unwrap().id(),
            "busy-a1b",
            "the cursor goes with the agent rather than off it"
        );

        press(&mut screen, shift(KeyCode::Up));
        assert_eq!(
            ordered(&screen),
            ["busy-a1b", "busy-b2c", "busy-c3d"],
            "and back where it was"
        );

        // One of them held at the top, and then the row it is on is not one
        // the agents under it can be moved into.
        press(&mut screen, KeyEvent::from(KeyCode::Down));
        press(&mut screen, ctrl('t'));
        assert_eq!(ordered(&screen), ["busy-b2c", "busy-a1b", "busy-c3d"]);
        press(&mut screen, KeyEvent::from(KeyCode::Down));
        press(&mut screen, shift(KeyCode::Up));
        assert_eq!(ordered(&screen), ["busy-b2c", "busy-a1b", "busy-c3d"]);
        assert_eq!(
            screen.list.selected().unwrap().id(),
            "busy-a1b",
            "a move that was refused is not a cursor that moved"
        );

        press(&mut screen, KeyEvent::from(KeyCode::Up));
        assert_eq!(
            screen.list.selected().unwrap().id(),
            "busy-b2c",
            "and the arrow without the chord walks the list as it always did"
        );
    }

    #[test]
    fn acts_what_the_view_keeps_opens_the_next_one_and_leaves_the_file_alone() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);

        // A file an older amx wrote, which knows about the offer and nothing
        // about arranging anything.
        let kept = root.path().join("view.json");
        std::fs::write(&kept, b"{\"statusline\": true}\n").unwrap();

        let at = |screen: &str, id: &str| {
            screen
                .lines()
                .position(|line| line.contains(id))
                .unwrap_or_else(|| panic!("no row for {id} in:\n{screen}"))
        };
        let (_, screen) = drawn_about(
            root.path(),
            &Scope::default(),
            vec![
                Typed::Key(KeyEvent::from(KeyCode::Down)),
                Typed::Key(ctrl('t')),
                Typed::Key(ctrl('s')),
                Typed::Key(KeyEvent::from(KeyCode::Char('q'))),
            ],
            Some(&kept),
        );
        assert!(
            at(&screen, "second-b2c") < at(&screen, "first-a1b"),
            "the one being held is at the top of its group:\n{screen}"
        );

        let (_, again) = drawn_about(
            root.path(),
            &Scope::default(),
            vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
            Some(&kept),
        );
        assert!(
            at(&again, "second-b2c") < at(&again, "first-a1b"),
            "and the next view opens on it, having been told nothing else:\n{again}"
        );
        assert!(
            again.contains("/srv/app"),
            "gathered the way the last one was left, too:\n{again}"
        );

        let written = std::fs::read_to_string(&kept).unwrap();
        assert!(
            written.contains("\"statusline\": true"),
            "what the file already said is still in it: {written}"
        );
    }

    #[test]
    fn acts_ctrl_x_arms_a_finished_row_and_the_press_after_it_forgets_it() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);
        let left = || crate::store::list(root.path()).unwrap().len();

        // The view opens on the newest of them, which is the row the key is
        // read on.
        let (_, armed) = pressing(root.path(), vec![ctrl('x')]);
        assert!(armed.contains("ctrl+x again forgets"), "{armed}");
        assert!(
            armed.contains("wrote the tests"),
            "and the row nobody armed is saying what it always said: {armed}"
        );
        assert_eq!(left(), 2, "and one press forgets nothing");

        // A second press with the cursor somewhere else arms the row it is on
        // rather than finishing what the first one started.
        let (_, moved) = pressing(
            root.path(),
            vec![ctrl('x'), KeyEvent::from(KeyCode::Down), ctrl('x')],
        );
        assert!(moved.contains("ctrl+x again forgets"), "{moved}");
        assert_eq!(left(), 2, "a press on another row arms that one instead");

        let (_, gone) = pressing(root.path(), vec![ctrl('x'), ctrl('x')]);
        assert!(gone.contains("first-a1b forgotten"), "{gone}");
        assert_eq!(left(), 1);
    }

    #[test]
    fn acts_ctrl_x_on_a_heading_arms_the_finished_under_it_and_the_press_after_forgets_them() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);
        let left = || crate::store::list(root.path()).unwrap().len();

        // Up from the row the view opens on is the heading over it. The first
        // press arms every finished row under it, each saying so where its
        // summary was, and the footer asks nothing.
        let (_, armed) = pressing(root.path(), vec![KeyEvent::from(KeyCode::Up), ctrl('x')]);
        assert_eq!(armed.matches("ctrl+x again forgets").count(), 2, "{armed}");
        assert!(!armed.contains("forget 2 finished"), "{armed}");
        assert_eq!(left(), 2, "and arming is all that has happened");

        // A key that is not the second press forgets nothing.
        let (_, kept) = pressing(
            root.path(),
            vec![
                KeyEvent::from(KeyCode::Up),
                ctrl('x'),
                KeyEvent::from(KeyCode::Down),
            ],
        );
        assert_eq!(left(), 2);
        assert!(!kept.contains("forgot"), "{kept}");

        // The second press on the heading, inside the window, forgets them
        // all.
        let (_, swept) = pressing(
            root.path(),
            vec![KeyEvent::from(KeyCode::Up), ctrl('x'), ctrl('x')],
        );
        assert!(swept.contains("forgot 2"), "{swept}");
        assert_eq!(left(), 0);
    }

    #[test]
    fn acts_ctrl_x_on_a_heading_over_nothing_finished_says_so_and_asks_nothing() {
        let root = TempDir::new().unwrap();
        let mut screen = watching(vec![reading(
            "busy-a1b",
            Phase::Working,
            State {
                state: Phase::Working,
                ..State::default()
            },
        )]);
        screen.list.up();

        screen
            .act(ctrl('x'), root.path(), &Config::default(), None)
            .unwrap();
        assert!(
            matches!(screen.mode, Mode::List),
            "a group with nothing finished under it is not a question"
        );
        let Some(Notice::Advice(said)) = &screen.notice else {
            panic!("no advice: the key did nothing and said nothing")
        };
        assert!(said.contains("working"), "{said}");
    }

    #[test]
    fn acts_peeking_at_an_agent_takes_the_mark_off_its_row() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);

        // The view opens on the newest ending, so the card opens on that one.
        let (code, drawn) = held(root.path(), &[KeyCode::Char(' '), KeyCode::Char('q')]);
        assert_eq!(code, exit::OK);
        let row = |id: &str| {
            drawn
                .lines()
                .find(|line| line.contains(id))
                .unwrap_or_else(|| panic!("no row for {id}:\n{drawn}"))
        };
        assert!(
            row("first-a1b").starts_with("  "),
            "the row somebody looked at has nothing left to say:\n{drawn}"
        );
        assert!(
            row("second-b2c").starts_with('•'),
            "and the one they did not is still holding something:\n{drawn}"
        );

        let looked = |id: &str| {
            crate::store::Agent::open(root.path(), id)
                .unwrap()
                .state()
                .unwrap()
                .seen
        };
        assert!(looked("first-a1b") > 0, "the look is on the record");
        assert_eq!(
            looked("second-b2c"),
            0,
            "and an agent nobody opened was not marked read on their behalf"
        );
    }

    #[test]
    fn acts_alt_and_a_digit_reach_the_agent_at_that_place_on_the_wall() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        finished(root.path(), "second-b2c", "wrote the tests", 120);

        // Both have ended with nothing recorded to pick up again, so neither
        // can be reached and the view says so by name — which is how a test
        // reads which row was counted to. The cursor is on the first of them
        // and was never moved.
        let (_, second) = pressing(
            root.path(),
            vec![alt('2'), KeyEvent::from(KeyCode::Char('q'))],
        );
        assert!(
            second.contains("no session was ever recorded for second-b2c"),
            "{second}"
        );

        let (_, first) = pressing(
            root.path(),
            vec![alt('1'), KeyEvent::from(KeyCode::Char('q'))],
        );
        assert!(
            first.contains("no session was ever recorded for first-a1b"),
            "{first}"
        );

        // A digit past the end of the wall says so rather than going quiet:
        // the digits count agents, and a heading is not one.
        let (_, past) = pressing(
            root.path(),
            vec![alt('9'), KeyEvent::from(KeyCode::Char('q'))],
        );
        assert!(past.contains("fewer than 9"), "{past}");
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

    /// Every key a terminal can send this view: the printable characters, the
    /// keys with names of their own, and each of them under every chord that
    /// can be held down in front of it.
    fn every_key() -> Vec<KeyEvent> {
        let named = [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Insert,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
        ];
        let chords = [
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ];
        (' '..='~')
            .map(KeyCode::Char)
            .chain(named)
            .chain((1..=12).map(KeyCode::F))
            .flat_map(|code| chords.map(move |held| KeyEvent::new(code, held)))
            .collect()
    }

    /// What the keys on the screen would call this one, so that what a brute
    /// force found bound can be looked for among them.
    fn named(key: KeyEvent) -> String {
        let mut said = String::new();
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            said.push_str("ctrl+");
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            said.push_str("alt+");
        }
        // Shift is worth naming on the one key a terminal sends it with.
        // Everywhere else it arrives as the character it typed.
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            said.push_str("shift+");
        }
        said.push_str(&match key.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(typed) => typed.to_string(),
            KeyCode::Enter => "enter".to_string(),
            KeyCode::Esc => "esc".to_string(),
            KeyCode::Tab | KeyCode::BackTab => "tab".to_string(),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            other => format!("{other:?}").to_lowercase(),
        });
        said
    }

    /// Whether the keys on the screen name this one.
    fn listed(key: KeyEvent) -> bool {
        let named = named(key);
        paint::HELP
            .iter()
            .any(|(keys, _)| keys.split(' ').any(|key| key == named || runs(key, &named)))
    }

    /// Whether a key column naming a run of keys names this one.
    ///
    /// `alt+1..9` is nine bindings, and nine rows saying the same words nine
    /// times would be a screen somebody has to read to find out they all do
    /// the same thing.
    fn runs(column: &str, named: &str) -> bool {
        let Some((first, last)) = column.split_once("..") else {
            return false;
        };
        let Some((chord, from)) = first.split_at_checked(first.len() - 1) else {
            return false;
        };
        match named.strip_prefix(chord) {
            Some(key) if key.len() == 1 => (from..=last).contains(&key),
            _ => false,
        }
    }

    /// Everything one keypress could leave different, as something two
    /// screens can be told apart by.
    fn standing(screen: &Screen) -> String {
        let mode = match &screen.mode {
            Mode::List => "list".to_string(),
            Mode::Keys => "keys".to_string(),
            Mode::Confirming(asked) => format!("confirming {}", asked.question()),
            Mode::Typing(composer) => format!("typing {} {}", composer.prompt(), composer.text),
        };
        let look = match screen.look {
            Look::Away => "away",
            Look::Screen => "screen",
            Look::Changes => "changes",
        };
        let notice = match &screen.notice {
            Some(Notice::Advice(said) | Notice::Failed(said)) => said.as_str(),
            None => "",
        };
        let dials = &screen.profile;
        format!(
            "{mode} · {look} · {notice} · {:?} · {:?} · {} {} {} {}",
            screen.list,
            screen.card.as_ref().map(|card| (&card.id, card.changes)),
            dials.agent,
            dials.model,
            dials.permission,
            dials.worktree,
        )
    }

    /// A fleet with somewhere for the cursor to stand on every kind of line
    /// there is: an agent that is running, the headings over the groups, and
    /// enough finished ones to put a fold under them.
    fn a_wall() -> Vec<View> {
        let mut views = vec![stopped_on_a_question("ask-a1b")];
        views.extend((0..5).map(|n| {
            reading(
                &format!("done-{n}"),
                Phase::Done,
                State {
                    state: Phase::Done,
                    exit: Some(0),
                    since: 1,
                    last_event: 1,
                    ..State::default()
                },
            )
        }));
        views
    }

    /// A place the cursor can stand, and what to call it in a failure.
    type Standing = (&'static str, fn(&mut Screen));

    /// Press one key on a view standing where `stand` puts it, and answer
    /// whether the key did anything at all.
    ///
    /// A key that leaves the screen where it was may still have done
    /// something: the ones that hand the terminal to a tmux client or to an
    /// editor say so by what they answer with rather than by what they change.
    fn acts_on(key: KeyEvent, root: &Path, stand: fn(&mut Screen)) -> bool {
        let mut screen = watching(a_wall());
        stand(&mut screen);

        let before = standing(&screen);
        let did = !matches!(
            screen.act(key, root, &Config::default(), None),
            Ok(Doing::Carry)
        );
        did || standing(&screen) != before
    }

    #[test]
    fn keymap_every_key_the_list_acts_on_is_named_among_the_keys() {
        let root = TempDir::new().unwrap();
        // Every kind of line the cursor stops on, because the same key does
        // different things on each of them, and a card over the list, which is
        // the other place the list's own keys are read.
        let standing: [Standing; 4] = [
            ("an agent's row", |_| {}),
            ("a heading", |screen| screen.list.up()),
            ("the fold", |screen| {
                for _ in 0..5 {
                    screen.list.down();
                }
            }),
            ("a card", |screen| {
                screen.look = Look::Screen;
                screen.card = screen.list.selected().map(card_of);
            }),
        ];

        for (where_it_is, stand) in standing {
            for key in every_key() {
                if !acts_on(key, root.path(), stand) {
                    continue;
                }
                assert!(
                    listed(key),
                    "{} does something on {where_it_is} and is not among the \
                     keys, so nobody who pressed it could find out what it did",
                    named(key)
                );
            }
        }
    }

    #[test]
    fn keymap_a_chord_the_view_never_bound_reaches_none_of_its_keys() {
        let root = TempDir::new().unwrap();
        // Each of these carries a key the list does act on. Held down with
        // something the list never asked for, they are somebody reaching past
        // the view: alt+q is a window being arranged, not a view being closed.
        for key in [alt('q'), ctrl('q'), alt('d'), ctrl('n'), alt('?')] {
            assert!(
                !acts_on(key, root.path(), |_| {}),
                "{} is not a key of this view",
                named(key)
            );
        }
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
    fn view_carries_the_waiting_count_in_what_the_terminal_is_called() {
        let waiting = watching(vec![
            stopped_on_a_question("ask-a1b"),
            stopped_on_a_question("ask-b2c"),
            reading(
                "busy-c3d",
                Phase::Working,
                State {
                    state: Phase::Working,
                    ..State::default()
                },
            ),
        ]);
        assert_eq!(
            paint::title(&waiting.list),
            "amx · 2 waiting",
            "the one question a tab bar can answer from across the room"
        );

        let quiet = watching(vec![reading(
            "busy-a1b",
            Phase::Working,
            State {
                state: Phase::Working,
                ..State::default()
            },
        )]);
        assert_eq!(
            paint::title(&quiet.list),
            "amx",
            "and a fleet with nothing waiting says nothing about a count"
        );
    }

    #[test]
    fn view_says_what_to_call_the_terminal_when_it_changes_and_not_otherwise() {
        let root = TempDir::new().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let mut said = Said::default();

        watch(
            root.path(),
            &Config::default(),
            &Scope::default(),
            &mut terminal,
            &mut Script(vec![Typed::Key(KeyEvent::from(KeyCode::Down))].into_iter()),
            None,
            None,
            &mut said,
        )
        .unwrap();

        assert_eq!(
            said.0,
            ["amx"],
            "said once and not again on every frame that did not change it"
        );
    }

    #[test]
    fn view_closes_when_somebody_closes_it() {
        let root = TempDir::new().unwrap();
        assert_eq!(held(root.path(), &[KeyCode::Char('q')]).0, exit::OK);
    }

    #[test]
    fn view_opened_about_a_directory_draws_that_directory_alone() {
        let root = TempDir::new().unwrap();
        finished_in(root.path(), "here-a1b", "wrote the parser", 60, "/srv/app");
        finished_in(
            root.path(),
            "deeper-b2c",
            "wrote the tests",
            90,
            "/srv/app/importer",
        );
        // The one a comparison of strings alone would have drawn with them.
        finished_in(root.path(), "alike-c3d", "read the log", 120, "/srv/app2");
        finished_in(root.path(), "far-d4e", "cut a release", 150, "/srv/other");

        let scope = Scope::of(Some(Path::new("/srv/app"))).unwrap();
        let (code, screen) = drawn_about(
            root.path(),
            &scope,
            vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
            None,
        );

        assert_eq!(code, exit::OK);
        for drawn in ["here-a1b", "deeper-b2c"] {
            assert!(screen.contains(drawn), "{drawn} is under it:\n{screen}");
        }
        for other in ["alike-c3d", "far-d4e"] {
            assert!(
                !screen.contains(other),
                "{other} is somebody else's afternoon:\n{screen}"
            );
        }
        assert!(
            screen.contains("2 done"),
            "and the count is of what was drawn, not of the machine:\n{screen}"
        );
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
            screen.contains("no session was ever recorded for first-a1b"),
            "an agent with nothing behind it to pick up is nowhere to be \
             taken, and the view says which is missing: {screen}"
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
    fn composer_alt_n_enters_the_line_the_way_enter_does_and_goes_with_it() {
        let root = TempDir::new().unwrap();
        let config = Config::default();
        let mut screen = Screen::default();
        let press = |screen: &mut Screen, key: KeyEvent| {
            screen.act(key, root.path(), &config, None).unwrap();
        };

        // A line the dials refuse is refused whichever key entered it, and
        // nothing was made on the way to finding out.
        press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
        for code in word("p:nonsense port it") {
            press(&mut screen, KeyEvent::from(code));
        }
        press(&mut screen, alt('n'));
        assert_eq!(
            screen
                .banded()
                .expect("the line stays where it was typed")
                .text,
            "p:nonsense port it"
        );
        assert!(crate::store::list(root.path()).unwrap().is_empty());

        // And a task barely long enough to be one is asked about first: the
        // key that goes with the agent is still the key that starts it.
        let mut screen = Screen::default();
        press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
        for code in word("fix") {
            press(&mut screen, KeyEvent::from(code));
        }
        press(&mut screen, alt('n'));
        let Mode::Confirming(Asked::Slight { follow, .. }) = &screen.mode else {
            panic!("three letters were started without a question")
        };
        assert!(*follow, "and the answer takes whoever asked to the agent");
    }

    #[test]
    fn acts_the_view_reaches_an_agent_it_started_by_reading_the_record_again() {
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        let mut screen = Screen::default();

        // Read from the record rather than from the list, which is a second
        // old and knows nothing about an agent younger than that.
        screen
            .landing(root.path(), &Config::default(), "first-a1b", None)
            .unwrap();
        let Some(Notice::Advice(said)) = &screen.notice else {
            panic!("nothing was said about where the agent went")
        };
        // Nothing was ever recorded for this one to be picked up again, so
        // what it is told is which of the two is missing.
        assert!(said.contains("first-a1b"), "{said}");
        assert!(said.contains("session"), "{said}");
    }

    #[test]
    fn acts_enter_on_an_agent_with_nothing_to_continue_says_which_is_missing() {
        // A pane that is gone is not the answer to what enter asked, and it is
        // not the reason either: what the row wants is the session it would
        // have been carried back on, and this one never had one.
        let root = TempDir::new().unwrap();
        finished(root.path(), "first-a1b", "wrote the parser", 60);
        let view = derive::view(root.path(), "first-a1b", rules::bundled(), now()).unwrap();

        let Reach::Say(Notice::Advice(said)) =
            reach(root.path(), &Config::default(), None, &view).unwrap()
        else {
            panic!("an agent with nothing to continue was reached anyway")
        };
        assert!(
            !said.contains("no pane any more"),
            "which is a fact about the pane, not a reason: {said}"
        );
        assert!(said.contains("amx new"), "and what to do instead: {said}");
    }

    #[test]
    fn composer_ctrl_g_takes_the_line_to_the_editor_and_leaves_it_open() {
        let root = TempDir::new().unwrap();
        let config = Config::default();
        let mut screen = Screen::default();

        // On the list it opens a task line and goes straight to the editor
        // with it, so a task worth a paragraph costs one keystroke.
        let doing = screen.act(ctrl('g'), root.path(), &config, None).unwrap();
        assert!(matches!(doing, Doing::Edit));
        let line = screen.banded().expect("a line for the editor to fill");
        assert_eq!(line.prompt(), "task");
        assert!(line.text.is_empty());

        // And on a line somebody is already typing, it is that line that goes
        // and that line the view is still holding when it comes back.
        let Mode::Typing(composer) = &mut screen.mode else {
            panic!("no line to edit")
        };
        composer.text = "port the importer".to_string();
        let doing = screen.act(ctrl('g'), root.path(), &config, None).unwrap();
        assert!(matches!(doing, Doing::Edit));
        assert_eq!(
            screen.banded().expect("the line is still there").text,
            "port the importer"
        );
    }

    #[test]
    fn composer_asks_once_before_starting_an_agent_on_a_task_of_three_letters() {
        let root = TempDir::new().unwrap();
        let mut keys = vec![KeyCode::Char('n')];
        keys.extend(word("fix"));
        keys.push(KeyCode::Enter);

        let (code, asked) = held(root.path(), &keys);
        assert_eq!(code, exit::OK);
        assert!(
            asked.contains("start an agent on \"fix\"? y"),
            "the question quotes what would be started:\n{asked}"
        );
        assert!(
            crate::store::list(root.path()).unwrap().is_empty(),
            "and the question is all that has happened"
        );

        // Any key but the one it asked for keeps the line, exactly as it was
        // typed: a task is worth more than the keystroke that interrupted it.
        keys.push(KeyCode::Char('n'));
        let (_, kept) = held(root.path(), &keys);
        assert!(kept.contains("task ▸ fix"), "{kept}");
        assert!(kept.contains("nothing was started"), "{kept}");
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
        // Seven rows of overlay on a terminal this short, so the keys are in
        // bands: the first of them down the left, and the next one beside it.
        // What a screen this small gives up is what the keys say rather than
        // which keys they are, so every one of them is still on it.
        assert!(screen.contains("↑ ↓"), "{screen}");
        assert!(screen.contains("shift+tab"), "{screen}");
        assert!(screen.contains("ctrl+g"), "{screen}");
        assert!(screen.contains("any key goes back"), "{screen}");
    }
}
