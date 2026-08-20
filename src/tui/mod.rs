//! The agent view.
//!
//! One screen, held open on a terminal, answering the question somebody opens
//! it for: is anything waiting on me? The agents are gathered under what they
//! need, the cursor walks them, space looks closer at one, and enter puts it in
//! front of the terminal.
//!
//! What can be done from here is what can be done from a shell prompt — start
//! one, say something to one, stop one, see what one has changed — because a
//! person watching a wall of agents should not have to leave the screen that
//! told them something needed doing. Every one of those is a key, and every
//! key that types text puts the view in a mode that says so: a list whose keys
//! are also letters cannot have a composer that swallows them.
//!
//! Nothing here is in the byte path. A peek is a `capture-pane`, and attaching
//! is tmux bringing the agent's own window forward — the view stays where it
//! is, in its window, for whoever comes back to it.
//!
//! The reading is taken from disk on a clock rather than pushed at the view by
//! anything: nothing amx runs stays resident, so there is nobody to push.

mod act;
mod paint;
mod rows;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::Backend;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::derive::{self, View};
use crate::store::{Phase, now};
use crate::tmux::{PaneId, Server, SessionId};
use crate::{exit, rules};
use act::{Asking, Composer};
use paint::Peek;
use rows::List;

/// How often the agents are read again.
const REFRESH: Duration = Duration::from_millis(1000);

/// How long the view waits for a key before it goes round again.
const TICK: Duration = Duration::from_millis(120);

/// What arrived from the terminal.
enum Typed {
    /// Nobody typed anything in the time given.
    Nothing,
    Key(KeyEvent),
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

/// What the closer look is showing.
#[derive(Default, PartialEq, Eq)]
enum Look {
    /// Nothing: nobody asked for one.
    #[default]
    Away,
    /// The agent's own screen, taken again with every reading.
    Screen,
    /// What the agent has changed, as it stood when somebody asked.
    Changes,
}

/// The view as it stands: what was read, where the cursor is, what the keys
/// are doing, and what the view last had to say for itself.
#[derive(Default)]
struct Screen {
    list: List,
    mode: Mode,
    look: Look,
    peek: Option<Peek>,
    notice: Option<String>,
    /// When the agents were last read.
    read: Option<Instant>,
}

/// Open the view on this terminal and hold it until somebody closes it.
pub fn run(root: &Path, config: &Config) -> Result<i32> {
    let mut terminal = ratatui::try_init().context("taking the terminal")?;
    let outcome = watch(root, config, &mut terminal, &mut Keyboard, Here::read());
    // Whatever happened, the screen goes back the way it was found.
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
    let mut screen = Screen::default();

    loop {
        if screen.read.is_none_or(|at| at.elapsed() >= REFRESH) {
            screen.reread(root)?;
        }
        terminal.draw(|frame| paint::draw(frame, &screen))?;

        match keys.next(TICK) {
            Typed::Nothing => {}
            Typed::Gone => return Ok(exit::OK),
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

    /// Take the closer look again, when it is the kind that follows the
    /// cursor: what an agent is doing is what its pane is showing now.
    fn follow_the_cursor(&mut self) {
        match self.look {
            Look::Away => self.peek = None,
            Look::Screen => self.peek = self.list.selected().map(look),
            // A diff was taken when somebody asked for it, and stays as it was
            // until they ask again.
            Look::Changes => {}
        }
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

        match self.mode {
            Mode::Typing(_) => self.typed(key, root, config),
            Mode::Keys => Ok(self.reading_the_keys(key)),
            Mode::List => self.pressed(key, root, here),
        }
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
            KeyCode::Char(' ') => {
                self.look = match self.look {
                    Look::Away => Look::Screen,
                    _ => Look::Away,
                };
                self.follow_the_cursor();
            }
            KeyCode::Esc => {
                self.look = Look::Away;
                self.follow_the_cursor();
            }
            KeyCode::Enter | KeyCode::Right => {
                if self.list.on_fold() {
                    self.list.unfold();
                } else if let Some(view) = self.list.selected() {
                    self.notice = attach(here, view)?;
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Keys,
            KeyCode::Char('n') => self.mode = Mode::Typing(Composer::new(Asking::Task)),
            // What a reply is depends on what the agent is doing: an agent
            // that has stopped on a question is answered with one of the keys
            // its prompt reads, and anything else is a message.
            KeyCode::Char('r') => {
                if let Some(view) = self.list.selected() {
                    self.mode = Mode::Typing(Composer::new(Asking::Reply {
                        id: view.id().to_string(),
                        question: view.phase() == Phase::Waiting,
                    }));
                }
            }
            KeyCode::Char('d') => {
                if let Some(view) = self.list.selected() {
                    match act::changes(root, view) {
                        Ok(peek) => {
                            self.peek = Some(peek);
                            self.look = Look::Changes;
                        }
                        Err(e) => self.notice = Some(format!("{e:#}")),
                    }
                }
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(view) = self.list.selected() {
                    self.notice = said(act::end(root, view));
                    self.acted();
                }
            }
            _ => {}
        }
        Ok(Doing::Carry)
    }

    /// A key while somebody is typing a line.
    ///
    /// The composer is taken out of the mode for the length of the keypress
    /// and put back unless the key was the end of it, which is what makes
    /// entering and cancelling the same one move: the line is gone either way.
    fn typed(&mut self, key: KeyEvent, root: &Path, config: &Config) -> Result<Doing> {
        let Mode::Typing(mut composer) = std::mem::take(&mut self.mode) else {
            return Ok(Doing::Carry);
        };

        match key.code {
            // Cancelled: the line goes, and nothing was done with it.
            KeyCode::Esc => return Ok(Doing::Carry),
            KeyCode::Enter => {
                if !composer.text.trim().is_empty() {
                    self.notice = said(match &composer.asking {
                        Asking::Task => act::start(root, config, &composer.text, composer.hidden),
                        Asking::Reply { id, .. } => act::reply(root, id, &composer.text),
                    });
                    self.acted();
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

/// What an action had to say, including when it could not be done at all: a
/// view that closed itself because git was busy would be a poor view.
fn said(outcome: Result<String>) -> Option<String> {
    Some(outcome.unwrap_or_else(|e| format!("{e:#}")))
}

/// A closer look at one agent: what it is asking, and the screen it is asking
/// it on — or, for an agent whose command has ended, the answer it left.
fn look(view: &View) -> Peek {
    let server = Server::from_socket(view.meta.socket.clone());
    let screen = (!view.phase().is_terminal())
        .then(|| server.capture(&view.meta.pane).ok())
        .flatten()
        .filter(|screen| !screen.trim().is_empty());

    Peek {
        id: view.id().to_string(),
        phase: view.phase(),
        question: view.state.question.clone(),
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
fn attach(here: Option<&Here>, view: &View) -> Result<Option<String>> {
    let elsewhere = format!("run `amx attach {}` to reach it", view.id());
    let Some(here) = here else {
        return Ok(Some(elsewhere));
    };

    let server = Server::from_socket(view.meta.socket.clone());
    if !server.pane_alive(&view.meta.pane) {
        return Ok(Some(format!("{} has no pane any more", view.id())));
    }
    if server.pane_field(&view.meta.pane, "#{pid}")? != here.pid {
        return Ok(Some(format!(
            "{} is on another tmux. {elsewhere}",
            view.id()
        )));
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
    use crate::store::{Agent, Meta, Phase, State};
    use crate::tmux::Socket;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Keys from a script, and then nobody at the terminal at all.
    struct Script(std::vec::IntoIter<KeyEvent>);

    impl Keys for Script {
        fn next(&mut self, _: Duration) -> Typed {
            match self.0.next() {
                Some(key) => Typed::Key(key),
                None => Typed::Gone,
            }
        }
    }

    fn typing(keys: &[KeyCode]) -> Script {
        Script(
            keys.iter()
                .map(|code| KeyEvent::new(*code, KeyModifiers::NONE))
                .collect::<Vec<_>>()
                .into_iter(),
        )
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
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let code = watch(
            root,
            &Config::default(),
            &mut terminal,
            &mut typing(keys),
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
        assert!(screen.contains("▸ ✓ second-b2c"), "{screen}");
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
