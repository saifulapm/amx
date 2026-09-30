//! The interactive agent view.
//!
//! Shows the agents grouped by what they need, walks them with a cursor, opens
//! a card over one (its question, conversation or pane, with a line to answer
//! on) and hands the terminal to an agent's tmux session. Each shell verb that
//! acts on an agent has a key here.
//!
//! - The view is never in the byte path: cards come from records, transcripts
//!   and `capture-pane`, and reaching an agent is a tmux client switch or a
//!   lent terminal, so the view is still there on return.
//! - State is read from disk on a timer. Nothing amx runs stays resident, so
//!   nothing pushes updates.
//! - Every key that types text switches the view into a mode, so list keys
//!   that are letters never reach a line being typed.

mod act;
mod clip;
mod grid;
mod keyname;
mod paint;
// Crate-visible because pins outlive the view and the verb that parks idle
// agents must honour them. See `rows::Arrangement::from_disk`.
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
use ratatui::backend::{Backend, CrosstermBackend};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{BufWriter, Stdout};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::config::Config;
use crate::derive::{self, View};
use crate::paths::stamped;
use crate::store::{Agent, Phase, now};
use crate::theme::{Theme, Watch};
use crate::tmux::{PaneId, Server, SessionId};
use crate::vendor::Transcript;
use crate::verbs::interrupt::{self, Cut};
use crate::verbs::ls::Scope;
use crate::verbs::resume::Comeback;
use crate::{exit, models, registry, spawn, verbs};
use act::{Asking, Composer, Renamed, Replied, Started};
/// The editor prompt, shared with `amx new --edit`.
pub use act::{Edited, edited};
use keyname::Bound;
use paint::{Body, Card, Hunk, Keymap, Notice};
use rows::{Arrangement, List, Narrow};

/// How often the agents are re-read.
const REFRESH: Duration = Duration::from_millis(1000);

/// How long the loop waits for input before going round again.
const TICK: Duration = Duration::from_millis(120);

/// One frame of the working pulse, claude's own spinner interval at 2.1.237.
const FRAME: Duration = Duration::from_millis(120);

/// How long a first `ctrl+x` or `c` press leaves rows armed for the second.
///
/// Five seconds: `c` arms every finished row, and reading a wall of reasons
/// takes longer than two. Both keys share the window.
const ARMED: Duration = Duration::from_secs(5);

/// Input read from the terminal.
enum Typed {
    /// Nothing arrived within the wait.
    Nothing,
    Key(KeyEvent),
    /// A mouse event. The view captures the mouse while it holds the screen.
    Mouse(MouseEvent),
    /// A bracketed paste.
    Paste(String),
    /// The terminal has gone, or a signal asked the view to end.
    Gone,
}

/// A source of terminal input. Tests substitute a script.
trait Keys {
    fn next(&mut self, patience: Duration) -> Typed;
}

/// Sets the terminal's title.
///
/// The title shows how many agents are waiting, which stays readable while
/// the window is behind others.
trait Titles {
    fn say(&mut self, said: &str);
}

/// Input from the real terminal.
struct Keyboard;

/// The real terminal's title bar.
struct TitleBar;

impl Titles for TitleBar {
    fn say(&mut self, said: &str) {
        // A terminal without a title bar ignores this.
        let _ = execute!(std::io::stdout(), SetTitle(said));
    }
}

impl Keys for Keyboard {
    fn next(&mut self, patience: Duration) -> Typed {
        // An unreadable terminal ends the view, as does a signal. The flag is
        // checked on both sides of the wait.
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

/// Input with late terminal colour replies filtered out; see
/// [`crate::shade::Late`].
struct Unanswered<K> {
    keys: K,
    late: crate::shade::Late,
    /// Events read but not yet returned, in order.
    ready: std::collections::VecDeque<Typed>,
}

/// How long to wait for the next key of a run that may be a colour reply.
/// A reply arrives in one write, so the rest of it is already queued.
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
                // Anything but a key ends the run; the held keys were real
                // typing.
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

/// What the loop does after handling an input.
enum Doing {
    Carry,
    Close,
    /// Lend the terminal to a tmux client on the agent's session until it
    /// detaches.
    Lend {
        id: String,
        on: Server,
        session: SessionId,
    },
    /// Lend the terminal to an editor for the line being typed.
    Edit,
    /// Lend the terminal to the configured diff viewer for this agent's patch.
    View {
        id: String,
    },
    /// Lend the terminal to a command bound to a key, run for the selected
    /// agent.
    Bound {
        id: String,
        spelling: String,
        command: String,
    },
}

/// What keys currently act on.
#[derive(Default)]
enum Mode {
    /// The agent list.
    #[default]
    List,
    /// A line being typed: a task, a reply, a name or a search.
    Typing(Composer),
    /// The key reference overlay.
    Keys,
    /// A question from the view itself, answered by one key.
    Confirming(Asked),
}

/// A question the view asks before acting.
///
/// Only `y` answers yes, because yes is the expensive answer: it starts a
/// program.
enum Asked {
    /// A task short enough to be a mistake, and the line it was typed on.
    /// `follow` is whether the key that asked also goes to the new agent.
    Slight {
        task: String,
        line: Composer,
        follow: bool,
    },
}

impl Asked {
    /// The question as shown, naming the key that answers it.
    fn question(&self) -> String {
        match self {
            Asked::Slight { task, .. } => {
                format!("start an agent on \"{task}\"? y starts it · anything else keeps the line")
            }
        }
    }
}

/// What the card shows.
#[derive(Default, PartialEq, Eq)]
enum Look {
    /// No card.
    #[default]
    Away,
    /// The agent's conversation or pane, retaken on each reading, with any
    /// pending question.
    Screen,
    /// The agent's diff, as taken when it was requested.
    Changes,
}

/// What the next agent is started with: vendor, dials, directory, and the cap
/// the header counts against.
///
/// The dials affect only agents not yet started. The profile is built from
/// the config file each time the view opens and never saved, so the file
/// stays the source of truth.
struct Profile {
    /// The vendor command a spawn runs: a command line, as the config holds it.
    agent: String,
    /// The command the config file named, where the vendor dial starts.
    configured: String,
    /// Dial settings. [`registry::DEFAULT`] passes no flag, leaving the
    /// vendor's own default.
    model: String,
    /// The config the view opened under. The model dial reads a harness's model
    /// list on the key press, since listing can cost a process.
    config: Config,
    permission: String,
    effort: String,
    /// Whether the next agent gets its own worktree.
    worktree: bool,
    /// Where the next agent runs, as displayed.
    dir: String,
    /// The cap the header counts against, if any.
    ///
    /// A project view counts against that project's `max_agents`. The
    /// machine-wide view counts against `max_total`, which may be unset.
    cap: Option<usize>,
}

impl Default for Profile {
    /// A project view under the default config.
    fn default() -> Profile {
        let config = Config::default();
        let cap = Some(config.max_agents);
        Profile::open(&config, cap, None, None)
    }
}

impl Profile {
    /// The profile a view opens with: the config's dials and the view's
    /// directory.
    ///
    /// A configured dial this vendor does not accept rests at the default. The
    /// cap is passed in because which config key applies depends on the view's
    /// scope.
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

    fn model_dial(&self) -> Option<registry::DialSpec> {
        registry::entry(&self.agent)?.model
    }

    fn permission_dial(&self) -> Option<registry::DialSpec> {
        registry::entry(&self.agent)?.permission
    }

    fn effort_dial(&self) -> Option<registry::DialSpec> {
        registry::entry(&self.agent)?.effort
    }

    /// The vendor dial's cycle: the configured command first, then every vendor
    /// in the registry.
    ///
    /// The configured command is kept whole, arguments included, so the cycle
    /// can always return to it.
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

    /// Turn to the next vendor.
    ///
    /// The model resets to the default, since a model belongs to one harness.
    /// Permission and effort keep their value where the new vendor accepts it
    /// and reset otherwise.
    fn cycle_vendor(&mut self) {
        let Some(next) = next_in(&self.vendors(), &self.agent) else {
            return;
        };
        self.agent = next;
        self.model = registry::DEFAULT.to_string();
        self.permission = effective(self.permission_dial(), Some(&self.permission));
        self.effort = effective(self.effort_dial(), Some(&self.effort));
    }

    /// Turn to the next model the current harness lists, then back to the
    /// default.
    ///
    /// The list is read on the key press rather than when the view opens, so
    /// only a harness someone turns the dial on pays for listing its models
    /// ([`models::models_of`] caches the list for an hour). A harness with no
    /// entry or no model dial does nothing.
    fn cycle_model(&mut self) {
        let Some(vendor) = registry::entry(&self.agent) else {
            return;
        };
        if vendor.model.is_none() {
            return;
        }
        let list = models::models_of(vendor, &self.config);
        let at = list.iter().position(|model| *model == self.model);
        // Past the last model, or from a model the list does not name, go back
        // to the default.
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

    /// The config a spawn from this view runs under: the file's config with the
    /// header's dials written over it.
    ///
    /// `new` reads flags before the config, and task-line tokens are flags, so
    /// a token on the line still overrides the header for that one spawn.
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

/// A dial value as the config stores it: `None` for the default.
fn turned_to(value: &str) -> Option<String> {
    (value != registry::DEFAULT).then(|| value.to_string())
}

/// The configured dial value if the vendor accepts it, else the default.
fn effective(dial: Option<registry::DialSpec>, configured: Option<&str>) -> String {
    match (dial, configured) {
        (Some(spec), Some(value)) if registry::accepts(&spec, value) => value.to_string(),
        _ => registry::DEFAULT.to_string(),
    }
}

/// The control and alt modifiers of a key.
///
/// Shift is left out: terminals report it by sending the shifted character,
/// except for tab.
fn chord(key: KeyEvent) -> KeyModifiers {
    key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Whether the card's reply line takes this key. Every other key goes to the
/// list.
///
/// The line takes plain characters, editing and cursor keys, enter, esc, tab,
/// `ctrl+a`, `ctrl+e` and `ctrl+w`, `ctrl+g` for the editor and `ctrl+j` for a
/// newline. Plain up and down go to the line only while it offers
/// suggestions; alt+up and alt+down always do, for history.
fn the_lines(composer: &Composer, key: KeyEvent) -> bool {
    let plain = chord(key).is_empty();
    let ctrl = chord(key) == KeyModifiers::CONTROL;
    match key.code {
        KeyCode::Char('a' | 'e' | 'w' | 'g' | 'j') if ctrl => true,
        // A chord on a character goes to the list.
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

/// Whether an empty reply line gives this key to the list anyway.
///
/// Space and enter have nothing to do on an empty line, so space closes the
/// card and enter acts on the selected row. Shift+enter still inserts a
/// newline.
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

/// The value after `now` in `cycle`, wrapping. A value the cycle does not
/// contain, such as a full model name from config, goes to the first entry.
fn next_in(cycle: &[&str], now: &str) -> Option<String> {
    match cycle.iter().position(|value| *value == now) {
        Some(at) => cycle.get((at + 1) % cycle.len()),
        None => cycle.first(),
    }
    .map(|value| value.to_string())
}

/// Rows a first `ctrl+x` or `c` press has armed, and when.
///
/// Held by id, since rows move between readings. A heading press also keeps
/// the heading's key: stopping a group moves its rows to Completed, so the
/// pressed heading can disappear while its rows are still armed.
struct Arm {
    /// The rows the second press acts on.
    ids: Vec<String>,
    /// Whether the first press was on a heading.
    swept: bool,
    /// The heading that armed it, or the heading now over its rows once that
    /// one has gone (see [`Screen::keep_the_sweep`]). A press on any other
    /// heading is a first press of its own.
    heading: Option<rows::Key>,
    /// Whether `c` armed it. `ctrl+x` and `c` never complete each other's arm.
    cleared: bool,
    /// Why each row is listed, parallel to `ids`. Only `c` gives reasons.
    why: Vec<String>,
    /// Rows among `ids` whose tree holds uncommitted work, checked on the first
    /// press. The second press skips them, so the rows say so up front.
    held: Vec<String>,
    at: Instant,
}

/// What a card was built from, to tell whether rebuilding it would change it.
///
/// Building a card reads and renders a transcript or forks `capture-pane`,
/// and most passes would produce the same card.
enum Freshness {
    /// The files the card was read from, each with its length and mtime at the
    /// read. `None` for a file that did not exist yet: a vendor names its
    /// transcript before writing it.
    Files(Vec<(PathBuf, Option<(u64, SystemTime)>)>),
    /// A pane capture or a question. Nothing on disk says whether a pane was
    /// redrawn, so these are always rebuilt.
    Pane,
}

impl Freshness {
    /// Whether any source file has changed since the card was read.
    fn moved(&self) -> bool {
        match self {
            Freshness::Pane => true,
            Freshness::Files(files) => files.iter().any(|(path, at)| stamped(path) != *at),
        }
    }
}

/// A path with its current stamp, taken before the file is read.
///
/// Stamping first means a write between the stat and the read leaves the
/// card stale on the next pass, never fresh with old contents.
fn as_read(path: PathBuf) -> (PathBuf, Option<(u64, SystemTime)>) {
    let at = stamped(&path);
    (path, at)
}

/// What the card on screen was built from: the agent, its phase and
/// question, the width it was wrapped to, and its source files.
struct Taken {
    id: String,
    phase: Phase,
    question: Option<String>,
    width: u16,
    fresh: Freshness,
}

/// The view's state: the reading, the cursor, the mode and the last notice.
#[derive(Default)]
struct Screen {
    /// The state directory, where cards read agent records.
    root: PathBuf,
    list: List,
    /// The directory the view is about or was run from. Tasks start here unless
    /// the line or the cursor says otherwise.
    standing: PathBuf,
    profile: Profile,
    theme: Theme,
    /// The theme's last warning, so fixing the file clears its own notice.
    complained: Option<String>,
    mode: Mode,
    look: Look,
    /// Whether rows show a column with vendor, model and effort. Off by
    /// default: on a single-vendor fleet it repeats one word and costs summary
    /// width.
    vendor: bool,
    card: Option<Card<Body>>,
    /// What `card` was built from, when the view built it. A card built
    /// elsewhere (a diff) has none, so the next pass rebuilds it.
    taken: Option<Taken>,
    /// The transcript behind the card, as far as it has been read.
    heard: Heard,
    /// How far the card body is paged from its natural edge, and the page size.
    /// The paint clamps it, since only it knows the body's height.
    scroll: paint::Scroll,
    /// The list's scroll offset and whether it must follow the cursor. Clamped
    /// by the paint.
    wall: paint::WallScroll,
    /// Scroll and search state of the key overlay. Clamped by the paint.
    keymap: Keymap,
    /// Keys the config file binds to commands, read once when the view opens.
    bound: Vec<Bound>,
    notice: Option<Notice>,
    /// Where the last frame put things, for mapping mouse positions.
    map: paint::Map,
    /// The list line under the pointer, when it is over an agent or a heading.
    hover: Option<usize>,
    /// Where the left button went down, while it is held. Tells a click from a
    /// drag.
    pressed_at: Option<(u16, u16)>,
    /// The cells being dragged over, press point first. Cleared on release,
    /// which copies them.
    selection: Option<((u16, u16), (u16, u16))>,
    /// The rows a first press has armed, if any.
    arm: Option<Arm>,
    /// When the agents were last read.
    read: Option<Instant>,
    /// The view state file, if there is one.
    remembering: Option<PathBuf>,
    /// The arrangement last read from or written to the view file, and the
    /// file's stamp then. Another view's write shows as a moved stamp; this
    /// view's writes merge against the file.
    published: Arrangement,
    viewed_at: Option<(u64, SystemTime)>,
    /// Whether a first `g` is waiting for the second of `gg`.
    ///
    /// Cleared at the start of every key press, so any other key cancels it.
    /// Untimed, unlike the arm: it only moves the cursor.
    going: bool,
    /// The agent last entered from this view, marked on the wall. Not saved.
    lent: Option<String>,
    /// The projects the listed agents run in, deduplicated in order, offered to
    /// `d:` on the task line.
    projects: Vec<PathBuf>,
    /// An agent the task line just started, for the cursor to land on once a
    /// reading includes it.
    started: Option<String>,
    /// The current frame of the working pulse.
    beat: usize,
    /// When that frame started.
    stepped: Option<Instant>,
    /// Lines this view has sent, tasks and replies apart, for recall.
    sent: act::Backlog,
}

/// Run the view on this terminal until it is closed.
///
/// While the view runs, the terminal is asked for bracketed paste, so a
/// pasted newline does not submit half a task, and for the kitty keyboard
/// protocol's disambiguation, so shift+enter arrives as its own key.
/// Terminals without either ignore the request.
///
/// `cap` is what the header counts against; see [`Profile::cap`].
pub fn run(root: &Path, config: &Config, scope: &Scope, cap: Option<usize>) -> Result<i32> {
    let mut terminal = take_the_terminal().context("taking the terminal")?;
    // Created before any mode is requested, so every exit path restores them.
    let held = Held;
    // Signals only set a flag the loop checks, so `held` still restores the
    // terminal.
    hear_the_end();
    // Without bracketed paste a paste arrives as keystrokes.
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    // Shift-drag still reaches the terminal's own selection.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);

    let _ = execute!(std::io::stdout(), TELL_THE_KEYS_APART);

    // Save the terminal's title; `Held` restores it.
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

    // Printed once the terminal is released, and not after an error, which is
    // what should be read then.
    if outcome.is_ok()
        && let Some(offer) = remembering.and_then(|path| offer_the_statusline(&path))
    {
        println!("{offer}");
    }
    outcome
}

/// Room for a whole frame, so the terminal reads each one in a single write.
const FRAME_BUFFER: usize = 64 * 1024;

/// What `ratatui::try_init` does, over a buffered stdout.
///
/// `Stdout` flushes every kilobyte, so a frame reached the terminal in many
/// writes and a slow link or a `tmux capture-pane` could see half of one.
/// ratatui flushes the backend once at the end of each draw.
fn take_the_terminal() -> std::io::Result<Terminal<CrosstermBackend<BufWriter<Stdout>>>> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ratatui::try_restore();
        hook(info);
    }));
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let stdout = BufWriter::with_capacity(FRAME_BUFFER, std::io::stdout());
    Terminal::new(CrosstermBackend::new(stdout))
}

/// Restores the terminal modes the view requested: on return, on a panic
/// unwinding through [`run`], and after a signal the loop heard.
///
/// Every restore is sent unconditionally; undoing a mode that was never
/// enabled does nothing.
struct Held;

impl Drop for Held {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(std::io::stdout(), Print(PUT_THE_TITLE_BACK));
        // Then clear it: tmux keeps no title stack, and the pane would
        // otherwise keep the view's last count.
        let _ = execute!(std::io::stdout(), SetTitle(""));
        // Not `restore`: it reports failure on stderr, which panics when the
        // terminal is gone, and the panic hook's own restore then aborts.
        let _ = ratatui::try_restore();
    }
}

/// Set when SIGTERM or SIGHUP arrives, or the terminal hangs up.
static ENDED: AtomicBool = AtomicBool::new(false);

extern "C" fn heard_the_end(_: nix::libc::c_int) {
    ENDED.store(true, Ordering::Relaxed);
}

/// Make SIGTERM and SIGHUP close the view the way `q` does, so the terminal
/// is restored.
///
/// The handler only sets a flag, the one async-signal-safe thing to do; the
/// loop checks it at least once per tick.
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

/// End the view when its terminal hangs up, wherever the loop is.
///
/// After a hangup crossterm's reads fail at once and are retried internally,
/// so a view waiting for a key would spin without noticing. This thread polls
/// the tty for the hangup alone, asks the loop to close, and exits the
/// process a second later if the loop has not: there is no terminal left to
/// restore.
fn watch_the_terminal() {
    use std::io::IsTerminal;
    use std::os::fd::IntoRawFd;
    // The tty crossterm reads from, found the same way crossterm finds it.
    let fd = match std::io::stdin().is_terminal() {
        true => nix::libc::STDIN_FILENO,
        false => match std::fs::File::open("/dev/tty") {
            Ok(tty) => tty.into_raw_fd(),
            Err(_) => return,
        },
    };
    std::thread::spawn(move || {
        // POLLHUP, POLLERR and POLLNVAL are reported even with no events
        // requested.
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

/// The theme the view opens with, any warnings from loading it, and the
/// watch on its file.
///
/// Loaded in [`run`] rather than in the loop, so tests pass their own palette
/// without touching the machine's theme directory.
#[derive(Default)]
struct Painting {
    theme: Theme,
    /// Problems with the theme file, shown once.
    warnings: Vec<String>,
    /// The theme file, checked each reading to pick up edits.
    watching: Watch,
}

impl Painting {
    /// Load the named theme.
    ///
    /// `ask` queries the terminal's colours. It runs here, after raw mode is on
    /// and before the loop reads keys, because the reply arrives on stdin and
    /// would otherwise land in the middle of typing.
    fn of(named: &str, ask: impl FnOnce() -> Option<String>, state_root: &Path) -> Painting {
        let named = Painting::named(named, ask, state_root);
        // Stamp before reading, so an edit in between triggers a reread.
        let watching = Watch::of(named);
        let (theme, warnings) = crate::theme::load(named);
        Painting {
            theme,
            warnings,
            watching,
        }
    }

    /// The palette name to use. Asks the terminal for its colours once and
    /// saves them for the panes amx starts.
    ///
    /// `auto` picks one of the two shipped palettes from the background shade
    /// (see [`crate::theme::AUTO`]); any other name is used as is. No reply
    /// saves nothing and keeps whatever an earlier view saved.
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
            // A failed save only leaves new panes with the old colours.
            let _ = crate::shade::remember(state_root, foreground, background);
        }
        crate::theme::chosen(named, || crate::shade::of_the_answer(answer.as_deref()))
    }
}

/// xterm's title stack push and pop. Terminals without it ignore the
/// sequences.
const KEEP_THE_TITLE: &str = "\x1b[22;2t";
const PUT_THE_TITLE_BACK: &str = "\x1b[23;2t";

/// The kitty keyboard protocol's disambiguation flag, so shift+enter arrives
/// as a modified enter.
///
/// Only this flag: event types and key releases would double every press.
/// Pushed and popped, so the terminal's previous setting is restored.
const TELL_THE_KEYS_APART: PushKeyboardEnhancementFlags =
    PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES);

/// The tmux config line the status line suggestion prints.
const STATUSLINE: &str = "set -g status-right '#(amx statusline) | %H:%M'";

/// The status line suggestion, printed the first time the view closes.
///
/// Printed for the user to paste, never written to their tmux config, and
/// only once.
fn offer_the_statusline(path: &Path) -> Option<String> {
    let mut remembered = Remembered::read(path);
    if remembered.statusline {
        return None;
    }

    // If the write fails the offer repeats next time, which beats an error.
    remembered.statusline = true;
    let _ = remembered.write(path);

    Some(format!(
        "amx can keep the fleet in the corner of your tmux status line:\n\n    \
         {STATUSLINE}\n\nPaste that into your tmux config. amx will not write \
         it for you."
    ))
}

/// What the view keeps between runs. Every field defaults, so a file written
/// by an older amx still reads.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Remembered {
    /// Whether the status line has been offered.
    statusline: bool,
    /// Pins, sleeping rows, order and axis.
    arrangement: Arrangement,
    /// Whether rows show the vendor column.
    vendor: bool,
    /// Lines sent, for recall in a later view.
    sent: act::Backlog,
}

impl Remembered {
    /// The file's contents, or the defaults if it is missing or unparseable.
    fn read(path: &Path) -> Remembered {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write the whole file atomically, so a concurrent reader sees the old
    /// contents or the new.
    fn write(&self, path: &Path) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self).context("writing what the view keeps")?;
        bytes.push(b'\n');
        crate::store::write_atomic(path, &bytes)
    }
}

/// The view's main loop: draw, handle input, re-read.
///
/// `remembering` is the view state file, if there is one; without it the
/// arrangement lasts only for this run.
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
    // A scoped view stands in its directory, otherwise in the current one.
    let standing = match scope.under() {
        Some(under) => under.to_path_buf(),
        None => std::env::current_dir().context("no working directory")?,
    };
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
    // Unusable key bindings are reported once, after the theme warning so they
    // take precedence.
    if !refused.is_empty() {
        screen.notice = Some(Notice::Refused(refused.join(" · ")));
    }
    let mut watching = painting.watching;
    // Restore the saved arrangement before the first frame.
    if let Some(path) = remembering {
        let remembered = Remembered::read(path);
        screen.list.arrange(remembered.arrangement.clone());
        screen.published = remembered.arrangement;
        screen.viewed_at = stamped(path);
        screen.vendor = remembered.vendor;
        screen.sent = remembered.sent;
    }

    // The title last set, so it is only sent when it changes.
    let mut called = String::new();

    // An input read while draining pointer movements, handled on the next pass.
    let mut held: Option<Typed> = None;

    // The first frame shows the records alone; the pass after it asks tmux.
    let mut records_only = true;

    // Only this loop waits for answers that land after the read that asked, so
    // only it lets `have_a_line_written` claim turns.
    derive::will_stay_for_the_answer();

    loop {
        let opening = std::mem::take(&mut records_only);
        let refreshing = screen.read.is_none_or(|at| at.elapsed() >= REFRESH);
        match refreshing {
            true if opening => screen.recall(root, scope)?,
            true => screen.reread(root, scope)?,
            // Between rereads only a question card is retaken.
            false => screen.freshen(),
        }
        // Pick up theme edits on the refresh cadence, not every tick.
        if refreshing && let Some((theme, warnings)) = watching.reread() {
            screen.repaint(theme);
            screen.say_of_the_theme(&warnings);
        }
        // Adopt arrangement changes another view wrote to the shared file.
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

        let arrived = match held.take() {
            Some(typed) => typed,
            // No wait after the records-only frame: take the real reading
            // straight away.
            None if opening => Typed::Nothing,
            None => keys.next(TICK),
        };
        let doing = match arrived {
            Typed::Nothing => Doing::Carry,
            Typed::Gone => return Ok(exit::OK),
            Typed::Mouse(mouse) => {
                // Collapse a run of pointer movements into one.
                let (rest, ended) = at_rest(keys, mouse);
                held = ended;
                screen.moused(rest, root, here.as_ref())?
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
                // Mark the row only if tmux attached. Either way the pointer
                // position is stale.
                match screen.notice.is_none() {
                    true => screen.went_into(id),
                    false => screen.hover = None,
                }
                // The borrower may have changed the title; set it again.
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

/// Drain a run of pointer movements and return the last position, plus any
/// other input that ended the run.
///
/// Terminals report every cell the pointer crosses, so one sweep queues
/// dozens of events and only the last matters. The input that ended the run
/// is returned rather than dropped, so keys typed behind a sweep are kept.
fn at_rest(keys: &mut impl Keys, moved: MouseEvent) -> (MouseEvent, Option<Typed>) {
    let mut rest = moved;
    if rest.kind != MouseEventKind::Moved {
        return (rest, None);
    }
    loop {
        // Zero wait: the run is whatever is already queued.
        match keys.next(Duration::ZERO) {
            Typed::Mouse(next) if next.kind == MouseEventKind::Moved => rest = next,
            Typed::Nothing => return (rest, None),
            ended => return (rest, Some(ended)),
        }
    }
}

/// Lend the terminal to a tmux client attached to the agent's session, and
/// take it back when the client detaches.
///
/// Used outside tmux, where the view and the client need the same terminal.
/// Detaching lands back on the view rather than at a shell prompt.
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
        // tmux's own message was on the screen the view has since redrawn.
        Ok(_) => Some(Notice::Failed(format!("tmux would not open {id}"))),
        Err(e) => Some(Notice::Failed(format!("reaching {id}: {e}"))),
    })
}

/// Lend the terminal to an editor on the line being typed, and put the
/// result back on the line.
///
/// The editor gets the text with pastes expanded, so the paste markers are
/// dropped from what comes back.
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
                // Cursor at the end, where the editor left off.
                composer.set_text(text);
            }
            // The text changed wholesale, so look the suggestions up again.
            screen.suggesting(config);
            None
        }
        Ok(Edited::No(why)) => Some(Notice::Refused(why)),
        Err(e) => Some(Notice::Failed(format!("{e:#}"))),
    };
    Ok(())
}

/// Lend the terminal to the configured diff viewer on this agent's patch, as
/// `amx diff` does at a shell.
///
/// Errors from the verb, such as no worktree or a removed tree, become the
/// notice.
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
    // The key checks this first; never give up the screen for nothing.
    let Some(viewer) = &config.diff else {
        return Ok(());
    };
    let read = borrowed(terminal, || verbs::diff::in_viewer(root, id, viewer))?;

    // A viewer that ran reports its own failure on the terminal. Only errors
    // from before it ran are the view's to show.
    screen.notice = read.err().map(|e| Notice::Failed(format!("{e:#}")));
    Ok(())
}

/// Lend the terminal to a command bound to a key, in the selected agent's
/// tree.
///
/// The command reports on the terminal itself. The notice only carries an
/// error from running it, prefixed with the key's spelling, since several
/// keys can be bound.
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

/// Release the terminal for `doing`, then take it back and redraw from
/// scratch.
fn borrowed<B, T>(terminal: &mut Terminal<B>, doing: impl FnOnce() -> T) -> Result<T>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    // The borrower sets up its own terminal modes.
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    // Editors expect a visible cursor. Nothing hides it again on return: every
    // frame the view draws does.
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
    /// Show the records alone, without asking tmux.
    ///
    /// Used for the first frame: a full reading costs a pane list per server
    /// and a capture per quiet agent, while the records are one file read each.
    /// The read clock stays unset, so the next pass takes a full reading.
    fn recall(&mut self, root: &Path, scope: &Scope) -> Result<()> {
        self.showing(scope.narrow(derive::recorded(root, now())?));
        Ok(())
    }

    /// Show a reading, and collect the projects its agents run in.
    ///
    /// Projects are listed once each, in reading order. If the reading moved
    /// the cursor off its agent, the list window follows the cursor; otherwise
    /// the window stays where it was scrolled.
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
        // The selected agent left the list, so its arm goes with it.
        if let Some(on) = &on
            && self.list.agent_by_id(on).is_none()
        {
            self.arm = None;
        }
        // Drop the hover if a different row moved under the pointer, since a
        // key read at the pointer would reach that row.
        if self.hover.is_some_and(|at| self.line_named(at) != pointed) {
            self.hover = None;
        }
    }

    /// A stable name for list line `at`: the agent's id or the heading's key.
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

    /// Switch to a re-read theme.
    ///
    /// The card's rows were rendered in the old colours and no source file
    /// changed, so `taken` is dropped to force a rebuild.
    fn repaint(&mut self, theme: Theme) {
        self.theme = theme;
        self.taken = None;
    }

    /// Show the theme's warnings as the notice.
    ///
    /// A fixed theme file clears its own earlier warning, but never a notice
    /// something else put up.
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

    /// Re-read the agents in the view's scope.
    ///
    /// The origin fetch covers every agent in scope, including rows a search
    /// hides, so `c` never judges against a stale upstream.
    fn reread(&mut self, root: &Path, scope: &Scope) -> Result<()> {
        // Only the agents in scope: a project view fetches that project's
        // origin.
        let views = scope.narrow(derive::views(root, now())?);
        verbs::sweep::fetch_origins_again(&views);
        self.showing(views);
        self.read = Some(Instant::now());
        self.keep_the_sweep();
        self.land_on_what_was_started();
        self.follow_the_cursor();
        Ok(())
    }

    /// Move the cursor to the agent the task line just started, once the wall
    /// has a row for it.
    ///
    /// The pending id is dropped whether or not the cursor landed, so an agent
    /// a search hides does not pull the cursor when the search is cleared.
    fn land_on_what_was_started(&mut self) {
        if let Some(id) = self.started.take() {
            self.list.land_on(&id);
        }
    }

    /// Keep the cursor and the arm with a swept group whose heading dissolved.
    ///
    /// Agents stopped during the arming window move to Completed on the next
    /// reading, so the pressed heading can vanish and another, live heading can
    /// slide under the cursor, one press from being stopped. While a heading
    /// sweep is armed and its heading has gone, a cursor on a heading moves to
    /// the heading now over the armed rows, and the arm takes that heading.
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

    /// Advance the working pulse if a frame has elapsed.
    ///
    /// Timed by the clock rather than by passes, since every key press also
    /// makes a pass.
    fn step(&mut self) {
        if self.stepped.is_none_or(|at| at.elapsed() >= FRAME) {
            self.beat = self.beat.wrapping_add(1);
            self.stepped = Some(Instant::now());
        }
    }

    /// Retake a question card between rereads.
    ///
    /// Answering one tab of a multi-question call moves the record to the next
    /// question at once, while the vendor is still redrawing, so the card is
    /// retaken every pass until it settles. Only a card with a recorded
    /// question qualifies: its body is built from the reading already in hand.
    /// A waiting card without one is a `capture-pane` and keeps the reading's
    /// cadence.
    fn freshen(&mut self) {
        let asked = self
            .card
            .as_ref()
            .is_some_and(|card| card.asks() && card.question.is_some());
        if asked {
            self.follow_the_cursor();
        }
    }

    /// The width card bodies are wrapped to: the list band's width in the last
    /// frame, or 80 before the first frame.
    fn body_width(&self) -> u16 {
        self.map.width().unwrap_or(80)
    }

    /// Whether the card on screen is what this pass would build: same agent,
    /// phase, question and width, and no source file changed.
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

    /// Retake the card for the selected agent when the look follows the cursor.
    ///
    /// A card paged away from its natural edge holds still while the cursor
    /// stays on its agent, so text does not move under the reader. A card that
    /// asks, or an agent that starts waiting, is never held. A card that
    /// [`Screen::stands`] is not rebuilt.
    ///
    /// The reply line comes and goes with the card.
    fn follow_the_cursor(&mut self) {
        match self.look {
            Look::Away => {
                self.card = None;
                self.taken = None;
                self.heard = Heard::default();
            }
            Look::Screen => {
                // A cursor on a heading or the fold keeps the current card, so
                // walking past a heading does not close and reopen it.
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
            // A diff stays as taken until it is requested again.
            Look::Changes => {}
        }

        // Open a reply line whenever a card is up and the list has the keys (an
        // answer to one tab spends the line, and the next tab needs one), and
        // close it with the card.
        let replying =
            matches!(&self.mode, Mode::Typing(line) if matches!(line.asking, Asking::Reply));
        match (self.card.is_some(), &self.mode) {
            (true, Mode::List) => self.mode = Mode::Typing(Composer::new(Asking::Reply)),
            (false, _) if replying => self.mode = Mode::List,
            _ => {}
        }
    }

    /// The reply line at the foot of the card, if that is what is being typed.
    fn answering(&self) -> Option<&Composer> {
        match &self.mode {
            Mode::Typing(composer) if self.on_the_card(composer) => Some(composer),
            _ => None,
        }
    }

    /// The line drawn in the band under the list: a task, a fork or a rename.
    ///
    /// A reply is the card's last row, and a search is drawn on the keys row so
    /// the list it narrows stays fully visible.
    fn banded(&self) -> Option<&Composer> {
        match &self.mode {
            Mode::Typing(composer) if !matches!(composer.asking, Asking::Reply | Asking::Find) => {
                Some(composer)
            }
            _ => None,
        }
    }

    /// Whether `composer` is the card's reply line.
    ///
    /// Takes the composer as an argument because the key handler has taken it
    /// out of the mode for the length of the press.
    fn on_the_card(&self, composer: &Composer) -> bool {
        matches!(composer.asking, Asking::Reply) && self.card.is_some()
    }

    /// The agent and choice to send when a digit on the card picks an option.
    ///
    /// Only on an empty line, at a question [`act::picks`] says takes a
    /// numbered choice, and only for a number the card shows. Anything else
    /// falls through to the line as a character. Whether the question takes
    /// several choices is read from the record, not the card.
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

    /// Open the card on the selected agent, with its reply line.
    fn look_closer(&mut self, root: &Path) {
        // Nothing to open on a heading or the fold.
        let Some(id) = self.list.selected().map(|view| view.id().to_string()) else {
            return;
        };
        self.look = Look::Screen;
        self.follow_the_cursor();

        // Opening the card marks the row read. A failed write only leaves the
        // mark on, which is not worth a notice.
        let _ = act::looked(root, &id);
        self.acted();
    }

    /// Close the card and its line.
    ///
    /// Also drops any review notes on a diff card; the hint under the line
    /// warns about that.
    fn look_away(&mut self) {
        self.scroll.open_at(0);
        self.look = Look::Away;
        self.follow_the_cursor();
    }

    /// Move the cursor to the row under the pointer, for keys that act where
    /// the user points. Returns whether the cursor moved.
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

    /// Record that the user went into agent `id`.
    ///
    /// Forgets the pointer, since mouse capture was off while tmux had the
    /// terminal, and closes the card, whose snapshot of the pane is stale once
    /// the user has been in the pane.
    fn went_into(&mut self, id: String) {
        self.lent = Some(id);
        self.hover = None;
        self.look_away();
    }

    /// Handle one key.
    ///
    /// Clears the hover afterwards, so a keyboard user does not act on whatever
    /// row the mouse was left over. The next movement restores it.
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

    /// Dispatch a key by mode.
    fn acting(
        &mut self,
        key: KeyEvent,
        root: &Path,
        config: &Config,
        here: Option<&Here>,
    ) -> Result<Doing> {
        // A notice is about the previous key.
        self.notice = None;

        // Raw mode has no interrupt, so ctrl+c closes the view in every mode.
        if key.code == KeyCode::Char('c') && chord(key) == KeyModifiers::CONTROL {
            return Ok(Doing::Close);
        }

        // Dials turn in the list and while typing, but not over the key overlay
        // or a question, where every key means something else.
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

    /// Turn a dial if `key` is a dial key: alt plus the dial's initial (alt+a
    /// for the agent), or shift+tab for permission, as in claude. Dials affect
    /// only the next agent started.
    fn turned(&mut self, key: KeyEvent) -> bool {
        let alt = chord(key) == KeyModifiers::ALT;
        let plain = chord(key).is_empty();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('a') if alt => self.profile.cycle_vendor(),
            KeyCode::Char('m') if alt => self.profile.cycle_model(),
            KeyCode::Char('e') if alt => self.profile.cycle_effort(),
            KeyCode::Char('w') if alt => self.profile.toggle_worktree(),
            // Terminals send shift+tab as BackTab or as tab with shift.
            KeyCode::BackTab if plain => self.profile.cycle_permission(),
            KeyCode::Tab if plain && shift => self.profile.cycle_permission(),
            _ => return false,
        }
        true
    }

    /// Handle a key in the list.
    ///
    /// Each binding matches its exact modifiers, so a chord never triggers a
    /// plain key: alt+q from a window manager must not close the view.
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
        // Shift only matters on the arrows.
        let shift = plain && key.modifiers.contains(KeyModifiers::SHIFT);
        // Taken, so every key but the second `g` cancels a pending `gg`.
        let going = std::mem::take(&mut self.going);
        match key.code {
            KeyCode::Char('q') if plain => return Ok(Doing::Close),
            // Shift+arrow moves the selected agent within its group.
            KeyCode::Down if shift => {
                let moved = self.list.move_by(1);
                self.keep(moved);
            }
            KeyCode::Up if shift => {
                let moved = self.list.move_by(-1);
                self.keep(moved);
            }
            // vim's j and k beside the arrows.
            KeyCode::Down | KeyCode::Char('j') if plain => {
                self.list.down();
                self.moved();
            }
            KeyCode::Up | KeyCode::Char('k') if plain => {
                self.list.up();
                self.moved();
            }
            // `G` goes to the bottom and `gg` to the top.
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
            // Arrows walk the list even with a card open; these page the card
            // body. ctrl+b and ctrl+f do the same on keyboards without page
            // keys.
            KeyCode::PageUp if plain => self.paged(true),
            KeyCode::PageDown if plain => self.paged(false),
            KeyCode::Char('b') if ctrl => self.paged(true),
            KeyCode::Char('f') if ctrl => self.paged(false),
            // Half pages.
            KeyCode::Char('u') if ctrl => self.paged_by(true, self.half()),
            KeyCode::Char('d') if ctrl => self.paged_by(false, self.half()),
            // Step between the hunks of a diff card.
            KeyCode::Char('n') if ctrl => self.to_hunk(true),
            KeyCode::Char('p') if ctrl => self.to_hunk(false),
            // Space moves the cursor to the pointer, like ctrl+x, and opens
            // that row's card. It closes the card only when the cursor did not
            // move.
            KeyCode::Char(' ') if plain => {
                let onto = self.land_on_the_pointer();
                match onto || matches!(self.look, Look::Away) {
                    true => self.look_closer(root),
                    false => self.look_away(),
                }
            }
            // Esc peels one layer: the card first, then a standing search.
            KeyCode::Esc if plain => match (self.card.is_some(), self.list.narrowing()) {
                (false, Some(_)) => self.widen(),
                _ => self.look_away(),
            },
            // On a heading, open or shut the group; on the fold, unfold; on a
            // row, go to the agent. `l` is vim's right. Only space opens a
            // card.
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if plain => {
                if self.list.on_heading() {
                    self.list.shut_or_open();
                    self.follow_the_cursor();
                } else if self.list.on_fold() {
                    self.list.unfold();
                } else {
                    return self.bring_forward(root, here);
                }
            }
            // alt+1 to alt+9 go to the nth agent on the wall.
            KeyCode::Char(digit @ '1'..='9') if alt => {
                let at = digit.to_digit(10).unwrap_or_default() as usize;
                return self.reach_the_nth(at, root, here);
            }
            // The first agent waiting on the user.
            KeyCode::Char('w') if plain => self.land_on_what_needs_you(),
            // The agent last visited.
            KeyCode::Backspace if plain => self.land_on_where_you_were(root),
            // The overlay always opens at the top.
            KeyCode::Char('?') if plain => {
                self.keymap.opened();
                self.mode = Mode::Keys;
            }
            KeyCode::Char('n') if plain => self.mode = Mode::Typing(self.task_line()),
            // Search, narrowing the list on every keystroke.
            KeyCode::Char('/') if plain => {
                self.mode = Mode::Typing(Composer::new(Asking::Find));
            }
            // A task line opened straight in the editor.
            KeyCode::Char('g') if ctrl => {
                self.mode = Mode::Typing(self.task_line());
                return Ok(Doing::Edit);
            }
            // Rename, starting from the current name.
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
                            // Not built from a reading, so the next look at
                            // the agent rebuilds its card.
                            self.taken = None;
                            self.look = Look::Changes;
                            self.scroll.open_at(0);
                            // Opens the reply line. A diff is never retaken.
                            self.follow_the_cursor();
                        }
                        Err(e) => self.notice = Some(Notice::Failed(format!("{e:#}"))),
                    }
                }
            }
            // The same patch in the configured diff viewer.
            KeyCode::Char('d') if alt => {
                if let Some(view) = self.list.selected() {
                    match config.diff.is_some() {
                        true => {
                            return Ok(Doing::View {
                                id: view.id().to_string(),
                            });
                        }
                        // Name the config key that would set a viewer.
                        false => {
                            self.notice = Some(Notice::Refused(
                                "no diff key in the config to read the patch with".to_string(),
                            ));
                        }
                    }
                }
            }
            // Open the row's pull request in the browser. The number comes from
            // the row, so no forge is queried.
            KeyCode::Char('o') if plain => {
                if let Some(view) = self.list.selected() {
                    self.notice = match self.list.requests(view).first() {
                        Some(pr) => act::open(view, pr.number)
                            .err()
                            .map(|e| Notice::Failed(format!("{e:#}"))),
                        None => Some(Notice::Refused(format!(
                            "no pull request on {}",
                            rows::called(view)
                        ))),
                    };
                }
            }
            // Interrupt the running turn. The verb decides what can be
            // interrupted, so the key and `amx interrupt` refuse the same cases
            // in the same words.
            KeyCode::Char('i') if plain => {
                if let Some(view) = self.list.selected() {
                    let name = rows::called(view);
                    self.notice = Some(match interrupt::cut_the_turn(root, view.id()) {
                        Ok(Cut::Turn) => Notice::Advice(format!("interrupted {name}")),
                        // Esc at a question would dismiss it for good, so point
                        // at the card instead.
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
            // Fork the selected agent with the task typed on the line; an empty
            // line forks with no first turn. A command has no conversation to
            // copy, so it is refused before the line opens.
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
            // Stop or forget the selected row, or every row under a heading.
            // The cursor first moves to the pointer, as a click would.
            KeyCode::Char('x') if ctrl => {
                self.land_on_the_pointer();
                match self.list.heading() {
                    Some(under) => self.sweep_or_arm(root, under),
                    None => self.end_or_arm(root),
                }
            }
            // Clear every finished row on the wall.
            KeyCode::Char('c') if plain => self.clear_or_arm(root),
            // Toggle the vendor column.
            KeyCode::Char('v') if plain => {
                self.vendor = !self.vendor;
                self.keep(true);
            }
            // Turn the grouping axis.
            KeyCode::Char('s') if ctrl => {
                self.list.turn();
                self.follow_the_cursor();
                self.keep(true);
            }
            // Pin the selected agent above the wall.
            KeyCode::Char('t') if ctrl => {
                let held = self.list.hold_or_let_go();
                self.keep(held);
            }
            // Sleep the selected agent below the wall.
            KeyCode::Char('z') if plain => {
                let slept = self.list.sleep_or_wake();
                self.keep(slept);
            }
            // Config-bound keys come last, so they never shadow a built-in
            // key. They need a selected agent, whose tree the command runs in.
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

    /// Handle a bracketed paste.
    ///
    /// On the task, reply and fork lines a long paste folds behind a marker; a
    /// name or search line takes the text as typed. At the list a paste opens a
    /// task line holding it, rather than replaying the text as keys.
    fn pasted(&mut self, text: &str, config: &Config) {
        // A notice is about the previous input.
        self.notice = None;

        // Normalise CRLF and CR line endings.
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
        self.suggesting(config);
    }

    /// Handle a key while a line is being typed.
    ///
    /// The composer is taken out of the mode for the press and put back unless
    /// the key sent or cancelled the line. A refused line is put back.
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

        // On the card, keys the line has no use for go to the list and the line
        // stays. On an empty line space and enter also go to the list, unless
        // review notes are waiting, in which case enter sends them.
        let sending = key.code == KeyCode::Enter && !self.noted("").is_empty();
        let empty = composer.text.is_empty() && the_lists_on_an_empty_line(key) && !sending;
        if self.on_the_card(&composer) && (empty || !the_lines(&composer, key)) {
            self.mode = Mode::Typing(composer);
            return self.pressed(key, root, config, here);
        }

        // A digit that picks an option is sent at once.
        if let Some((id, choice)) = self.picking(&composer, key) {
            let said = act::reply(root, &id, &choice);
            self.replied(said, composer);
            return Ok(Doing::Carry);
        }

        match key.code {
            // Esc closes the suggestions before the line.
            KeyCode::Esc if composer.suggest.is_some() => {
                composer.suggest = None;
                self.mode = Mode::Typing(composer);
                return Ok(Doing::Carry);
            }
            // Cancel. The card closes with its line.
            KeyCode::Esc => {
                if self.on_the_card(&composer) {
                    self.look_away();
                }
                // Cancelling a search clears the narrowing it made.
                if let Asking::Find = composer.asking {
                    self.widen();
                }
                return Ok(Doing::Carry);
            }
            // Shift+enter and alt+enter insert a newline. Shift only arrives on
            // terminals that accepted `TELL_THE_KEYS_APART`; elsewhere
            // shift+enter sends, and alt+enter is the fallback.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                composer.insert("\n");
            }
            // ctrl+j inserts a newline too: raw mode delivers 0x0A as ctrl+j.
            KeyCode::Char('j') if chord(key) == KeyModifiers::CONTROL => composer.insert("\n"),
            // Tab takes the chosen suggestion. Enter does too while the word is
            // shorter than the choice; once the word is spelled out, enter
            // sends.
            KeyCode::Tab if composer.suggest.is_some() && chord(key).is_empty() => {
                composer.complete();
            }
            // Tab on an empty task line writes `@`, which lists the vendor's
            // agents or the project's files.
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
                // A search has already narrowed the list; enter only closes
                // the line.
                if let Asking::Find = composer.asking {
                    return Ok(Doing::Carry);
                }
                // Rename. A refused name keeps the line open.
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
                // Fork, with the typed task or none.
                if let Asking::Fork { id } = &composer.asking {
                    let id = id.clone();
                    let whole = composer.whole();
                    let task = (!whole.trim().is_empty()).then_some(whole);
                    let made = act::spawn_copy(root, &id, task.as_deref());
                    self.forked(made, composer);
                    return Ok(Doing::Carry);
                }
                // A reply goes to whichever agent the card shows at the press.
                if matches!(composer.asking, Asking::Reply) {
                    if let Some(id) = self.card.as_ref().map(|card| card.id.clone()) {
                        // A diff review goes as one message: the opening words,
                        // each hunk's note, and the line as the note on the
                        // current hunk. Sent piecemeal, the agent would answer
                        // the first note before the rest arrived. Without notes
                        // the line goes alone.
                        let review = self.review(&composer.whole());
                        let said = self.written(&review);
                        if said.trim().is_empty() {
                            return Ok(Doing::Carry);
                        }
                        // Checked before the send, which spends the notes.
                        let kept = !self.scroll.remarks().is_empty();
                        let sent = act::reply(root, &id, &said);
                        let took = matches!(sent, Ok(Replied::Yes(_)));
                        self.replied(sent, composer);
                        // A sent review returns the card to the top of the
                        // patch. A refused one keeps its notes to send again.
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
            // alt+n starts the task and goes to the new agent. Task lines only.
            KeyCode::Char('n')
                if chord(key) == KeyModifiers::ALT
                    && matches!(composer.asking, Asking::Task)
                    && !composer.text.trim().is_empty() =>
            {
                return self.entering(root, config, composer, true, here);
            }
            // Put the line back before lending the terminal, so the editor's
            // result lands on it.
            KeyCode::Char('g') if chord(key) == KeyModifiers::CONTROL => {
                self.mode = Mode::Typing(composer);
                return Ok(Doing::Edit);
            }
            KeyCode::Backspace if chord(key) == KeyModifiers::ALT => composer.delete_word_back(),
            KeyCode::Char('w') if chord(key) == KeyModifiers::CONTROL => {
                composer.delete_word_back()
            }
            // Backspace on an empty search line leaves the search, as in vim.
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
            KeyCode::Left if chord(key) == KeyModifiers::CONTROL => composer.word_left(),
            KeyCode::Right if chord(key) == KeyModifiers::CONTROL => composer.word_right(),
            KeyCode::Left => composer.left(),
            KeyCode::Right => composer.right(),
            KeyCode::Home => composer.home(),
            KeyCode::End => composer.end(),
            KeyCode::Char('a') if chord(key) == KeyModifiers::CONTROL => composer.home(),
            KeyCode::Char('e') if chord(key) == KeyModifiers::CONTROL => composer.end(),
            // Up and down move through the suggestions.
            KeyCode::Up if chord(key).is_empty() && composer.suggest.is_some() => {
                composer.choose(-1)
            }
            KeyCode::Down if chord(key).is_empty() && composer.suggest.is_some() => {
                composer.choose(1)
            }
            // History: alt+arrows on any line, plain arrows on a task line. The
            // card's line keeps plain arrows for the card.
            KeyCode::Up | KeyCode::Down
                if chord(key) == KeyModifiers::ALT
                    || (chord(key).is_empty() && matches!(composer.asking, Asking::Task)) =>
            {
                let sent = self.sent.lines_for(&composer.asking);
                composer.recall(sent, key.code == KeyCode::Up);
            }
            // A chord is not a character.
            KeyCode::Char(typed)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                composer.insert(&typed.to_string());
            }
            _ => {}
        }

        // A search narrows the list again after every key.
        if let Asking::Find = composer.asking {
            self.list.narrow(act::finding(&composer.text));
            self.follow_the_cursor();
        }

        self.mode = Mode::Typing(composer);
        // Refresh suggestions, except after up and down, which only move the
        // choice and would otherwise reset it.
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.suggesting(config);
        }
        Ok(Doing::Carry)
    }

    /// Report the outcome of a reply sent from the card.
    ///
    /// A refused reply keeps its line for editing, like a refused task.
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

    /// Report the outcome of a fork, the way [`Screen::starting`] reports a
    /// spawn.
    ///
    /// The line goes into history and the cursor goes to the new agent. A
    /// refused fork keeps its line. The user stays on the wall, where the
    /// original agent is still running.
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

    /// A new task line. On a wall grouped by project it targets the project
    /// under the cursor, otherwise the view's directory.
    fn task_line(&self) -> Composer {
        let mut composer = Composer::new(Asking::Task);
        composer.under = self.list.project_under_cursor();
        composer
    }

    /// Recompute the suggestions for the word under the cursor.
    ///
    /// A task line suggests for the header's vendor in the project the line
    /// targets, or the view's directory. A reply or fork line suggests for the
    /// target agent's own vendor in its directory. The wall's projects are
    /// passed for `d:` completion.
    fn suggesting(&mut self, config: &Config) {
        let Mode::Typing(composer) = &self.mode else {
            return;
        };
        let (launching, project) = match &composer.asking {
            Asking::Reply | Asking::Fork { .. } => {
                // A reply targets the card's agent now; a fork targets the row
                // it was opened on.
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

    /// Put the agent just started in front of the user.
    ///
    /// Read from its record, since the wall's reading predates the agent.
    fn landing(&mut self, root: &Path, id: &str, here: Option<&Here>) -> Result<Doing> {
        let view = match derive::view(root, id, now()) {
            Ok(view) => view,
            // The agent runs either way; report the error and stay open.
            Err(e) => {
                self.notice = Some(Notice::Failed(format!("{e:#}")));
                return Ok(Doing::Carry);
            }
        };

        let reached = reach(root, here, &view)?;
        Ok(self.arrived(id.to_string(), reached))
    }

    /// Act on what reaching the agent `id` came to.
    fn arrived(&mut self, id: String, reached: Reach) -> Doing {
        match reached {
            // Inside tmux the client has already switched, so mark the row now.
            Reach::There => self.went_into(id),
            Reach::Say(notice) => self.notice = Some(notice),
            Reach::Lend(on, session) => return Doing::Lend { id, on, session },
        }
        Doing::Carry
    }

    /// Go to the selected agent: enter or a click on a row.
    fn bring_forward(&mut self, root: &Path, here: Option<&Here>) -> Result<Doing> {
        let Some(view) = self.list.selected() else {
            return Ok(Doing::Carry);
        };
        let id = view.id().to_string();
        let reached = reach(root, here, view)?;
        // A resumed agent is in a pane this reading does not know.
        self.acted();
        Ok(self.arrived(id, reached))
    }

    /// Go to the `at`th agent on the wall, counted from the top.
    ///
    /// Headings, the fold and agents in shut groups are not counted.
    fn reach_the_nth(&mut self, at: usize, root: &Path, here: Option<&Here>) -> Result<Doing> {
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
        let reached = reach(root, here, view)?;
        self.acted();
        Ok(self.arrived(id, reached))
    }

    /// `w`: move the cursor to the first agent waiting on the user, like
    /// `amx attach --waiting`. Only moves the cursor.
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

    /// `backspace`: move the cursor to the agent last visited, like
    /// `amx attach --last`. Only moves the cursor.
    ///
    /// Skips the selected agent, so two presses toggle between two agents, and
    /// agents not on the list.
    fn land_on_where_you_were(&mut self, root: &Path) {
        let standing = self.list.selected().map(|view| view.id().to_string());
        let back = verbs::attach::visited(root)
            .into_iter()
            .find(|id| Some(id) != standing.as_ref() && self.list.land_on(id));
        if back.is_none() {
            self.notice = Some(Notice::Advice("no agent to go back to".to_string()));
        }
    }

    /// Submit a task line: start the agent, asking first when the task is short
    /// enough to be a mistake.
    ///
    /// `follow` is whether the user goes to the new agent.
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

    /// Start an agent on the line under the header's dials.
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
                    return self.landing(root, &id, here);
                }
                // Land on the new agent once a reading includes it.
                self.started = Some(id);
            }
            // A refused task keeps its line for editing.
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
    /// Checked against the clock on every call rather than cleared on expiry,
    /// since nothing runs at the moment the window closes.
    fn arming(&self) -> Option<&Arm> {
        self.arm.as_ref().filter(|arm| arm.at.elapsed() < ARMED)
    }

    /// The rows a press has armed, while its window is still open.
    fn armed(&self) -> &[String] {
        self.arming().map_or(&[], |arm| arm.ids.as_slice())
    }

    /// The rows a `ctrl+x` armed. A `ctrl+x` never completes the arm `c` left,
    /// since those rows were not chosen with the cursor.
    fn forgetting(&self) -> &[String] {
        self.arming()
            .filter(|arm| !arm.cleared)
            .map_or(&[], |arm| arm.ids.as_slice())
    }

    /// Why each armed row is listed, parallel to [`armed`](Self::armed). Only
    /// `c` gives reasons.
    fn why(&self) -> &[String] {
        self.arming()
            .filter(|arm| arm.cleared)
            .map_or(&[], |arm| arm.why.as_slice())
    }

    /// Armed rows the second press skips because their tree holds uncommitted
    /// work.
    fn held(&self) -> &[String] {
        self.arming()
            .filter(|arm| arm.cleared)
            .map_or(&[], |arm| arm.held.as_slice())
    }

    /// Whether the arm came from a heading. A heading's second press stops live
    /// agents before forgetting them; a row's only forgets.
    fn swept(&self) -> bool {
        self.arming().is_some_and(|arm| arm.swept)
    }

    /// `ctrl+x` on a row. The first press stops a live agent, idle included,
    /// and arms the row; a second press within [`ARMED`] forgets it. If the
    /// window lapses nothing is removed.
    ///
    /// Stopping keeps the record. Forgetting removes the record and its
    /// worktree, so it always takes two presses. The warning shows on the row.
    fn end_or_arm(&mut self, root: &Path) {
        let Some(view) = self.list.selected() else {
            return;
        };
        // Only a row's own arm. Rows a heading armed are finished from the
        // heading, and a press on one of them is that row's first press.
        if !self.swept() && self.forgetting().iter().any(|id| id == view.id()) {
            self.arm = None;
            self.notice = kept_a_tree(act::forget(root, view));
            self.acted();
            return;
        }

        let id = view.id().to_string();
        match view.phase().is_terminal() {
            // The row shows the warning; clear the old notice.
            true => self.notice = None,
            false => {
                let stopped = act::stop(root, view);
                let held = stopped.is_ok();
                self.notice = said(stopped);
                self.acted();
                // A row that would not stop is not armed; the error is shown
                // instead.
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

    /// `ctrl+x` on a heading. The first press only arms every row under it,
    /// each row showing the warning. A second press within [`ARMED`] on the
    /// same heading stops the live agents, idle included, and forgets them all
    /// with the same worktree safety as a single row.
    ///
    /// Stopping a whole group can cost every pane on screen, so even the stop
    /// waits for the second press. A row whose stop fails is not forgotten, and
    /// the error is shown.
    fn sweep_or_arm(&mut self, root: &Path, under: rows::Under) {
        let pressed = self.list.key(under);
        let again = self
            .arming()
            .filter(|arm| arm.swept && !arm.cleared)
            .is_some_and(|arm| arm.heading.is_some() && arm.heading == pressed);
        if again {
            let arm = self.arm.take().expect("the arm that was just read");
            // Only armed agents still on the list can be stopped and forgotten.
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
            // One line for both outcomes, shown as a failure if any stop
            // failed.
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
        // The rows show the warning; clear the old notice.
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

    /// `c`: two presses over every finished row on the wall, as `amx clear`.
    ///
    /// The first press asks the verb which rows are finished (done, failed or
    /// stopped) and why, and marks each with its reason. A second press within
    /// [`ARMED`] takes them as the verb would. Rows in folded or shut groups
    /// count; rows a search hides do not.
    ///
    /// Asked only on the press: each finished branch costs two git calls, too
    /// many for every reading.
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

        // The rows show the warning; clear the old notice.
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

    /// Take the rows the first press marked that are still on the list.
    ///
    /// Rows found holding uncommitted work are skipped without asking git
    /// again, since the row has said so since the first press. Kept rows are
    /// counted rather than named, since a wall can keep many.
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
                // Work landed between the presses, or git would not remove the
                // tree.
                Ok(verbs::clear::Taken::Holding(_)) => kept += 1,
                // Report the first error and carry on with the rest.
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

    /// Handle the key that answers a question of the view's own.
    ///
    /// Only `y` is yes. A chord is never an answer: it is a key meant for
    /// something else.
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
            // Anything but yes restores the line as typed.
            Asked::Slight { line, .. } if !yes => {
                self.mode = Mode::Typing(line);
                self.notice = Some(Notice::Advice("nothing was started".to_string()));
                Ok(Doing::Carry)
            }
            Asked::Slight { line, follow, .. } => self.starting(root, config, line, follow, here),
        }
    }

    /// Handle a key over the key overlay.
    ///
    /// The list's movement keys scroll the overlay and `/` searches it; `q`
    /// closes the view; any other key returns to the list. An open search takes
    /// every letter, esc clears it, and enter keeps the narrowing.
    fn reading_the_keys(&mut self, key: KeyEvent) -> Doing {
        let plain = chord(key).is_empty();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let finding = self.keymap.finding();

        // Scrolling works while searching too, except j and k, which are text
        // then.
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
                KeyCode::Enter if plain => self.keymap.kept(),
                _ => return self.leaving_the_keys(key),
            }
            return Doing::Carry;
        }

        match key.code {
            KeyCode::Char('/') if plain => self.keymap.find(),
            // One `g` goes to the top here: nothing on this screen needs
            // guarding.
            KeyCode::Char('G') if plain => self.keymap.to_the_end(true),
            KeyCode::Char('g') if plain => self.keymap.to_the_end(false),
            _ => return self.leaving_the_keys(key),
        }
        Doing::Carry
    }

    /// Return to the list, or close the view on `q`.
    fn leaving_the_keys(&mut self, key: KeyEvent) -> Doing {
        self.mode = Mode::List;
        match key.code {
            KeyCode::Char('q') => Doing::Close,
            _ => Doing::Carry,
        }
    }

    /// Note that the cursor moved.
    ///
    /// Tells the list window to follow the cursor on the next frame, resets the
    /// card's paging to where the card opens, and turns a diff card back into
    /// the agent's card, since a diff belongs to the agent it was taken of. The
    /// mouse wheel moves no cursor and does not come through here.
    fn moved(&mut self) {
        self.wall.follow.set(true);
        self.scroll
            .open_at(self.card.as_ref().map_or(0, Card::opens_at));
        if self.look == Look::Changes {
            self.look = Look::Screen;
        }
        self.follow_the_cursor();
    }

    /// Clear both the state and the name narrowing, whichever a search set.
    fn widen(&mut self) {
        self.list
            .narrow(vec![Narrow::State(None), Narrow::Name(None)]);
        self.follow_the_cursor();
    }

    /// Scroll the list one line with the mouse wheel.
    ///
    /// The cursor and the card stay put; the paint clamps the offset. The hover
    /// is dropped because a different row is now under the pointer.
    fn scrolled(&mut self, up: bool) {
        let top = self.wall.top.get();
        self.wall.top.set(match up {
            true => top.saturating_sub(1),
            false => top.saturating_add(1),
        });
        self.hover = None;
    }

    /// Page the card body one page away from or back toward its natural edge.
    fn paged(&mut self, up: bool) {
        self.paged_by(up, self.scroll.page.get().max(1));
    }

    /// Half the card's last page height, at least one row.
    fn half(&self) -> usize {
        (self.scroll.page.get() / 2).max(1)
    }

    /// Move to the next or previous hunk of a diff card.
    ///
    /// Does nothing on other cards. The reply line is the note on the current
    /// hunk: a step stores it under the hunk being left and loads the note of
    /// the hunk reached, so a review is written hunk by hunk and sent whole.
    // `to_` names where the cursor goes, like the other stepping keys; it is
    // not a conversion.
    #[allow(clippy::wrong_self_convention)]
    fn to_hunk(&mut self, forward: bool) {
        if self.look != Look::Changes {
            return;
        }
        // Only the card's own line is a note.
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
                // The stored note is plain text, so no paste markers apply.
                composer.set_text(words);
            }
        }
    }

    /// The key the line's words are stored under: the current hunk, or `None`
    /// at the top of the patch, where the review's opening words go.
    fn noting(&self) -> Option<usize> {
        self.at_hunk().map(|(at, _)| at)
    }

    /// The review as enter would send it: the stored notes, with `words` as the
    /// note at the cursor. Blank `words` withdraw the note there, as
    /// [`paint::Scroll::remark`] does.
    fn review(&self, words: &str) -> BTreeMap<Option<usize>, String> {
        let mut review: BTreeMap<_, _> = self.scroll.remarks().into_iter().collect();
        let at = self.noting();
        match words.trim().is_empty() {
            true => review.remove(&at),
            false => review.insert(at, words.to_string()),
        };
        review
    }

    /// The hunks that review has notes on, in patch order. The opening words
    /// are not a note.
    fn noted(&self, words: &str) -> Vec<usize> {
        self.review(words).keys().copied().flatten().collect()
    }

    /// The review as one message: the opening words, then each note on a hunk
    /// the card still shows, in patch order.
    ///
    /// The agent keeps working while the review is written, so the card can be
    /// retaken; a note whose hunk has gone is dropped.
    fn written(&self, review: &BTreeMap<Option<usize>, String>) -> String {
        let hunks = self.card.as_ref().map_or(&[][..], |card| card.body.hunks());
        let notes: Vec<(&Hunk, &str)> = review
            .iter()
            .filter_map(|(at, words)| Some((hunks.get((*at)?)?, words.as_str())))
            .collect();
        let opening = review.get(&None).map_or("", String::as_str);
        act::on_hunks(opening, &notes)
    }

    /// The current hunk's index and the hunk, if the cursor is on one the
    /// card's patch still has.
    fn at_hunk(&self) -> Option<(usize, &Hunk)> {
        let at = self.scroll.at_hunk()?;
        let hunk = self.card.as_ref()?.body.hunks().get(at)?;
        Some((at, hunk))
    }

    /// Page the card body by `page` rows.
    ///
    /// A patch or a recorded answer reads down from its top and a live screen
    /// up from its bottom, so which key leads away depends on the card. The
    /// paint clamps the offset.
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

    /// Handle a mouse event.
    ///
    /// A click on a row goes to that agent, like enter; a click on a heading or
    /// the fold toggles it; the wheel scrolls the list, or pages the card when
    /// over it; hovering highlights a row or heading, which is where `ctrl+x`
    /// acts. Only the list and a card with its reply line take the mouse; other
    /// lines being typed and the view's own questions ignore it.
    ///
    /// A left drag selects and copies the cells it covers, since mouse capture
    /// takes away the terminal's own selection. The press records where it
    /// started; the release tells a click (same cell) from a drag.
    fn moused(&mut self, mouse: MouseEvent, root: &Path, here: Option<&Here>) -> Result<Doing> {
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
                    _ => return self.clicked(mouse, root, here),
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

    /// A left click: pressed and released on the same cell.
    fn clicked(&mut self, mouse: MouseEvent, root: &Path, here: Option<&Here>) -> Result<Doing> {
        if !self.list_takes_the_mouse() {
            return Ok(Doing::Carry);
        }
        let Some(at) = self.line_under(mouse.column, mouse.row) else {
            return Ok(Doing::Carry);
        };
        // A notice is about the previous input.
        self.notice = None;
        match self.list.items().get(at) {
            // A click on a row goes to the agent, like enter.
            Some(rows::Item::Agent(_)) => {
                if self.list.land(at) {
                    self.moved();
                    return self.bring_forward(root, here);
                }
            }
            Some(rows::Item::Heading(..)) => {
                if self.list.land(at) {
                    self.list.shut_or_open();
                    self.follow_the_cursor();
                }
            }
            // Unfold in place; the cursor stays where it was.
            Some(rows::Item::Fold(..)) => self.list.unfold_at(at),
            _ => {}
        }
        Ok(Doing::Carry)
    }

    /// Copy the text a drag covered to the terminal's clipboard with OSC 52.
    ///
    /// The text comes from the last frame, so it is exactly what was visible.
    /// Terminals do not acknowledge OSC 52, so "copied" means the request was
    /// sent.
    fn copied(&mut self, from: (u16, u16), to: (u16, u16)) {
        let text = self.map.selected(from, to);
        let _ = execute!(
            std::io::stdout(),
            Print(format!("\x1b]52;c;{}\x07", clip::base64(text.as_bytes())))
        );
        self.notice = Some(Notice::Advice("copied".to_string()));
    }

    /// Whether clicks and the wheel reach the list: in list mode, or while the
    /// card's reply line is open. Other lines being typed keep the mouse away.
    fn list_takes_the_mouse(&self) -> bool {
        matches!(self.mode, Mode::List) || self.answering().is_some()
    }

    /// The list line at this point, if any. The list band is usually taller
    /// than the list.
    fn line_under(&self, column: u16, row: u16) -> Option<usize> {
        self.map
            .line_under(column, row)
            .filter(|at| *at < self.list.items().len())
    }

    /// Force a reading on the next pass, after acting on an agent.
    fn acted(&mut self) {
        self.read = None;
    }

    /// Save the arrangement to the view file, if `changed`.
    ///
    /// Saved on every change, since a closed terminal ends the view without a
    /// clean exit. The file is read first and the change merged in, so another
    /// view's concurrent pin is not lost.
    fn keep(&mut self, changed: bool) {
        let Some(path) = self.remembering.clone().filter(|_| changed) else {
            return;
        };
        let mut remembered = Remembered::read(&path);
        let disk = remembered.arrangement.clone();
        remembered.arrangement =
            Arrangement::merged(&self.published, &self.list.arrangement(), &disk);
        remembered.vendor = self.vendor;
        // A failed write is not worth the notice line.
        let _ = remembered.write(&path);
        // Adopt the merged arrangement now, and merge the next change against
        // it.
        self.published = remembered.arrangement.clone();
        self.viewed_at = stamped(&path);
        self.list.arrange(remembered.arrangement);
    }

    /// Take in another view's arrangement if the view file changed since this
    /// view last read or wrote it.
    ///
    /// Replaced wholesale, since this view's own changes are already in the
    /// file. The stamp is taken before the read, so a write in between is
    /// picked up next time.
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

    /// Add a sent line to the history and save it, so the next view can recall
    /// it too.
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

/// An action's outcome as a notice: its message as advice, or its error as a
/// failure. Errors are shown, never raised, so the view stays open.
fn said(outcome: Result<String>) -> Option<Notice> {
    Some(match outcome {
        Ok(said) => Notice::Advice(said),
        Err(e) => Notice::Failed(format!("{e:#}")),
    })
}

/// A forget's outcome as a notice. A forget that kept a dirty worktree is a
/// refusal rather than advice, so the user sees why the row is still there.
fn kept_a_tree(outcome: Result<(String, bool)>) -> Option<Notice> {
    Some(match outcome {
        Ok((said, true)) => Notice::Refused(said),
        Ok((said, false)) => Notice::Advice(said),
        Err(e) => Notice::Failed(format!("{e:#}")),
    })
}

/// Build the card for one agent, with the files it was read from.
///
/// In order of precedence:
/// - A command, or a vendor that died before naming a transcript: the tail of
///   its captured output ([`Agent::output_tail`]), followed live while it
///   runs and read from the top once it has ended.
/// - A transcript amx can read ([`crate::conversation`]): the whole
///   conversation, markdown rendered and wrapped to `width`, with the
///   vendor's live stream under it while a turn runs. The pane is never read
///   under a transcript. A working agent with an empty transcript shows its
///   task.
/// - A recorded question: the question block alone.
/// - A finished turn with a recorded answer: that answer.
/// - Otherwise the pane, captured with its colours and cut of the vendor's
///   furniture.
fn card_of(
    view: &View,
    root: &Path,
    width: u16,
    theme: Theme,
    heard: &mut Heard,
) -> (Card<Body>, Freshness) {
    let agent = Agent::open(root, view.id()).ok();
    // Whether a reply on the card would be accepted.
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
    // A command's captured output. An empty file is an empty card: a pane
    // capture would be cut with vendor anchors that do not apply.
    let printed = agent
        .as_ref()
        .map(|agent| as_read(agent.dir().join(crate::store::OUTPUT)));
    if let Some(said) = agent.as_ref().and_then(Agent::output_tail) {
        // Read from the live edge while running, from the top once ended.
        return (
            card(Body::said(&said), view.phase().is_terminal(), Vec::new()),
            Freshness::Files(printed.into_iter().collect()),
        );
    }

    let server = Server::from_socket(view.meta.socket.clone());
    // A recorded question is the whole card. A waiting agent without one gets a
    // pane capture, the only place its question is written.
    let asks = view.phase() == Phase::Waiting && view.state.question.is_some();

    let working = view.phase() == Phase::Working;
    // Stamp every source before reading it; see `as_read`.
    let log = agent
        .as_ref()
        .filter(|_| working)
        .map(|agent| as_read(agent.events_path()));
    let recorded = view.meta.transcript.clone().map(as_read);
    // The live stream only matters while a turn runs.
    let streaming = agent
        .as_ref()
        .filter(|_| working)
        .map(|agent| as_read(agent.dir().join(crate::store::LIVE)));

    // Messages sent but not yet taken. Only a running turn queues them, and the
    // vendor draws them in the part of the pane the card cuts off.
    let queued = match (&agent, working) {
        (Some(agent), true) => agent
            .events()
            .map(|events| verbs::send::still_queued(&view.meta, &events))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if !asks && let Some(said) = conversation_of(&view.meta, working, heard) {
        // The vendor's live stream goes under the transcript while a turn runs.
        // Vendors that stream nothing show the transcript alone.
        let live = working
            .then(|| agent.as_ref().and_then(Agent::live))
            .flatten();
        // Read from the live edge while working, forward from the last answer
        // otherwise.
        return (
            card(
                Body::conversation(&said, live.as_deref(), width, theme),
                !working,
                queued,
            ),
            Freshness::Files(recorded.into_iter().chain(streaming).chain(log).collect()),
        );
    }

    // A finished turn with a recorded answer shows the whole answer rather than
    // the pane, a viewport claude scrolls on its own. The group is judged from
    // the phase alone, ignoring pins and requests.
    let answered = rows::Group::of(view.phase(), false, false, false) == rows::Group::Completed
        && view.state.result.is_some();
    let screen = (!asks && !answered && !view.phase().is_terminal())
        .then(|| server.capture_painted(&view.meta.pane).ok())
        .flatten()
        // A capture with colours but no text counts as empty.
        .filter(|screen| !crate::ansi::strip_ansi(screen).trim().is_empty());

    // A card that asks shows nothing older than the question.
    let body = match (asks, answered) {
        (true, _) => Body::none(),
        (_, true) => Body::said(view.state.result.as_deref().unwrap_or_default()),
        _ => {
            let said = screen
                .as_deref()
                .or(view.state.result.as_deref())
                .unwrap_or_default();
            // An ended agent's text has no furniture to cut. A live pane is cut
            // with the anchors of the vendor the record names.
            match view.phase().is_terminal() {
                true => Body::said(said),
                false => Body::screen(own_chrome(&view.meta), said),
            }
        }
    };
    (
        card(body, answered, queued),
        // Questions, captures and recorded answers are retaken on every
        // reading.
        Freshness::Pane,
    )
}

/// The conversation in the record's transcript, if its vendor keeps one amx
/// can read.
///
/// A missing or empty transcript on a working agent means its first turn is
/// not written yet, so its task stands in as the prompt and the card does not
/// switch from pane to conversation mid-turn. Other agents get `None` and
/// fall back to their answer or pane.
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

/// The furniture of the vendor the record says runs in this pane. A record
/// naming no command uses the fallback vendor's.
fn own_chrome(meta: &crate::store::Meta) -> &'static crate::furniture::Furniture {
    crate::rules::of(meta.agent.as_deref().unwrap_or_default()).furniture()
}

/// The tmux server and session the view itself runs in, read once at start.
struct Here {
    /// The tmux server's pid. One server can have several socket paths, so
    /// sockets cannot be compared.
    pid: String,
    /// The session of the view's own pane.
    session: Option<SessionId>,
}

impl Here {
    /// The tmux this process runs inside, if any.
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

/// What going to an agent came to.
enum Reach {
    /// The agent is now in front of the user.
    There,
    /// It is not; the notice says why, and how to reach it where possible.
    Say(Notice),
    /// The view owns the terminal and lends it to this session.
    Lend(Server, SessionId),
}

/// Put the agent in front of the user, resuming it first if its pane is gone
/// or now belongs to another agent.
///
/// An agent with a session to continue is resumed into a fresh pane, as
/// `amx attach` does; one with nothing to continue is refused with the
/// reason.
fn reach(root: &Path, here: Option<&Here>, view: &View) -> Result<Reach> {
    let server = Server::from_socket(view.meta.socket.clone());
    if server.pane_answers_for(&view.meta.pane, view.id()) {
        return Ok(noted(root, view.id(), reaching(server, here, view)?));
    }

    let env = spawn::env_snapshot(std::env::vars());
    match verbs::resume::picked_up(root, view.id(), None, &env)? {
        Comeback::No(why) => Ok(Reach::Say(Notice::Refused(why))),
        // Resuming moved the agent to a new pane, so read its record again.
        Comeback::Back => {
            let back = derive::view(root, view.id(), now())?;
            let server = Server::from_socket(back.meta.socket.clone());
            Ok(noted(root, back.id(), reaching(server, here, &back)?))
        }
    }
}

/// Record the agent in the visit trail `amx attach --last` follows, if the
/// user actually got there.
fn noted(root: &Path, id: &str, reached: Reach) -> Reach {
    if matches!(reached, Reach::There | Reach::Lend(..)) {
        verbs::attach::note_visited(root, id);
    }
    reached
}

/// The tmux part of going to an agent.
///
/// Unlike `amx attach`, which execs tmux, the view must stay alive. Inside
/// tmux the terminal's client switches to the agent's session and the view
/// keeps running in its own. Outside tmux the view lends its terminal. Either
/// way ctrl+z in the session is the way back, bound here first.
///
/// An agent the view cannot reach is reported rather than an error: it can
/// still be reached another way.
fn reaching(server: Server, here: Option<&Here>, view: &View) -> Result<Reach> {
    // A client can only switch to sessions on its own server, and nesting a
    // second client is what tmux warns against.
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

    // Switch the clients on the view's own session, by tty, since the server
    // may have other clients.
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
