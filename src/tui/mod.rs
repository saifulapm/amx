//! The agent view.
//!
//! One screen, held open on a terminal, answering the question somebody opens
//! it for: is anything waiting on me? The agents are gathered under what they
//! need, the cursor walks them, space looks closer at one, and enter puts it in
//! front of the terminal.
//!
//! Nothing here is in the byte path. A peek is a `capture-pane`, and attaching
//! is tmux bringing the agent's own window forward — the view stays where it
//! is, in its window, for whoever comes back to it.
//!
//! The reading is taken from disk on a clock rather than pushed at the view by
//! anything: nothing amx runs stays resident, so there is nobody to push.

mod paint;
mod rows;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::Backend;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive::{self, View};
use crate::store::now;
use crate::tmux::{PaneId, Server, SessionId};
use crate::{exit, rules};
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

/// Open the view on this terminal and hold it until somebody closes it.
pub fn run(root: &Path) -> Result<i32> {
    let mut terminal = ratatui::try_init().context("taking the terminal")?;
    let outcome = watch(root, &mut terminal, &mut Keyboard, Here::read());
    // Whatever happened, the screen goes back the way it was found.
    ratatui::restore();
    outcome
}

/// The view itself: draw what is there, act on what is typed, read again.
fn watch<B>(
    root: &Path,
    terminal: &mut Terminal<B>,
    keys: &mut impl Keys,
    here: Option<Here>,
) -> Result<i32>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut list = List::default();
    let mut peek: Option<Peek> = None;
    let mut notice: Option<String> = None;
    let mut peeking = false;
    let mut read: Option<Instant> = None;

    loop {
        let fresh = read.is_none_or(|at| at.elapsed() >= REFRESH);
        if fresh {
            list.show(derive::views(root, rules::bundled(), now())?);
            read = Some(Instant::now());
        }

        // The peek follows the cursor, and is taken again with every reading:
        // what an agent is doing is what its pane is showing now.
        let wanted = peeking.then(|| list.selected()).flatten().map(View::id);
        if fresh || peek.as_ref().map(|peek| peek.id.as_str()) != wanted {
            peek = peeking.then(|| list.selected()).flatten().map(look);
        }

        terminal.draw(|frame| paint::draw(frame, &list, peek.as_ref(), notice.as_deref()))?;

        match keys.next(TICK) {
            Typed::Nothing => {}
            Typed::Gone => return Ok(exit::OK),
            Typed::Key(key) => {
                if let Doing::Close = act(key, &mut list, &mut peeking, &mut notice, here.as_ref())?
                {
                    return Ok(exit::OK);
                }
            }
        }
    }
}

/// What one key does.
fn act(
    key: KeyEvent,
    list: &mut List,
    peeking: &mut bool,
    notice: &mut Option<String>,
    here: Option<&Here>,
) -> Result<Doing> {
    // Whatever the view had to say, it was about the last key.
    *notice = None;

    match key.code {
        // A terminal in raw mode has no interrupt of its own, and a view
        // nobody can get out of the usual way is a trap.
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(Doing::Close);
        }
        KeyCode::Char('q') => return Ok(Doing::Close),
        KeyCode::Down => list.down(),
        KeyCode::Up => list.up(),
        KeyCode::Char(' ') => *peeking = !*peeking,
        KeyCode::Esc => *peeking = false,
        KeyCode::Enter | KeyCode::Right => {
            if list.on_fold() {
                list.unfold();
            } else if let Some(view) = list.selected() {
                *notice = attach(here, view)?;
            }
        }
        _ => {}
    }
    Ok(Doing::Carry)
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
            "{} is on another tmux — {elsewhere}",
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
        let code = watch(root, &mut terminal, &mut typing(keys), None).unwrap();
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
}
