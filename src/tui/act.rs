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

use anyhow::{Context, Result};
use std::path::Path;

use super::paint::Peek;
use super::rows::Narrow;
use crate::cli::{NewArgs, StopArgs};
use crate::config::Config;
use crate::derive::View;
use crate::store::{Agent, Phase};
use crate::tmux::Server;
use crate::{derive, exit, rules, spawn, store, verbs, worktree};

/// A line somebody is typing, and what it is for.
pub struct Composer {
    pub asking: Asking,
    pub text: String,
    /// Start the agent where nobody is looking.
    pub hidden: bool,
}

/// What entering the line will do.
pub enum Asking {
    /// A task, for an agent that does not exist yet.
    Task,
    /// Something for an agent that is already running: a message, or the key
    /// its question is waiting for.
    Reply { id: String, question: bool },
}

impl Composer {
    pub fn new(asking: Asking) -> Composer {
        Composer {
            asking,
            text: String::new(),
            hidden: false,
        }
    }

    /// What the line calls itself, so nobody types a task at an agent.
    ///
    /// A task line renames itself the moment what is typed into it would
    /// narrow the list instead: the one thing a person needs to know before
    /// pressing enter is what enter is about to do.
    pub fn prompt(&self) -> String {
        match &self.asking {
            Asking::Task if self.narrows() => "narrow".to_string(),
            Asking::Task if self.hidden => "task · out of sight".to_string(),
            Asking::Task => "task".to_string(),
            Asking::Reply { id, question: true } => format!("answer {id} · y n 1-9 enter esc"),
            Asking::Reply { id, .. } => format!("message to {id}"),
        }
    }

    /// Whether entering this line narrows the list rather than starting
    /// anything with it.
    pub fn narrows(&self) -> bool {
        matches!(self.asking, Asking::Task) && narrowing(&self.text).is_some()
    }
}

/// A line of nothing but `s:` and `a:` tokens narrows the list; anything else
/// is a task, colons and all.
///
/// Nothing but: "s:waiting is what to check" is a sentence somebody may well
/// want an agent to act on, and a surface that guessed otherwise would be one
/// nobody could type into.
pub fn narrowing(line: &str) -> Option<Vec<Narrow>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty()
        || !tokens
            .iter()
            .all(|token| token.starts_with("s:") || token.starts_with("a:"))
    {
        return None;
    }

    Some(
        tokens
            .iter()
            .map(|token| {
                let (key, want) = token.split_at(2);
                // A token with nothing after it drops that narrowing.
                let want = (!want.is_empty()).then(|| want.to_string());
                match key {
                    "s:" => Narrow::State(want),
                    _ => Narrow::Name(want),
                }
            })
            .collect(),
    )
}

/// Start an agent on what was typed, where the view is.
pub fn start(root: &Path, config: &Config, task: &str, hidden: bool) -> Result<String> {
    let dir = std::env::current_dir().context("no working directory")?;
    let args = NewArgs {
        task: task.to_string(),
        bg: hidden,
        name: None,
        dir: None,
        no_worktree: false,
        agent: None,
        vendor_args: Vec::new(),
    };

    let (mut started, mut refused) = (Vec::new(), Vec::new());
    let code = verbs::new::run(
        root,
        &dir,
        spawn::env_snapshot(std::env::vars()),
        config,
        &args,
        &mut started,
        &mut refused,
    )?;
    if code != exit::OK {
        return Ok(one_line(&refused));
    }
    Ok(format!("started {}", one_line(&started)))
}

/// Say something to the agent under the cursor.
pub fn reply(root: &Path, id: &str, text: &str) -> Result<String> {
    let view = derive::view(root, id, rules::bundled(), store::now())?;
    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());

    match view.phase() {
        Phase::Waiting => {
            let Some(key) = verbs::answer::named(text) else {
                return Ok(format!(
                    "{id} is asking; y, n, 1-9, enter or esc answers it"
                ));
            };
            verbs::answer::press(&agent, &server, &view.meta.pane, &key)?;
            Ok(format!("answered {id}"))
        }
        phase if phase.is_terminal() => Ok(format!("{id} is {phase}; nothing is listening")),
        _ => {
            verbs::send::deliver(&agent, &server, &view.meta.pane, text)?;
            Ok(format!("sent to {id}"))
        }
    }
}

/// End the agent under the cursor: one that is running is stopped, and one
/// that has already ended is forgotten.
pub fn end(root: &Path, view: &View) -> Result<String> {
    if view.phase().is_terminal() {
        return forget(root, view);
    }

    // The dispositions a person gets asked about at a shell prompt are taken
    // here as the defaults they already are: the worktree goes, the branch
    // stays, the record stays. Nothing that could lose work is decided by a
    // keystroke.
    // `forget` above is this file's own `--delete`, and it is a second
    // keystroke rather than part of this one: ending an agent and clearing its
    // row away are two decisions on the wall as well as at a prompt.
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

/// Forget an agent whose command has ended: its record, and the tree it was
/// given with it.
///
/// A tree holding work no commit has keeps both. Its record is where the
/// branch and the commit that tree was cut from are named, and a tree nothing
/// names is work nobody will find again.
fn forget(root: &Path, view: &View) -> Result<String> {
    let agent = Agent::open(root, view.id())?;

    if let Some(tree) = &view.meta.worktree
        && tree.exists()
    {
        if worktree::is_dirty(tree).unwrap_or(true) {
            return Ok(format!(
                "keeping {}: {} holds work no commit has",
                view.id(),
                tree.display()
            ));
        }
        let repo = worktree::main_repo(tree).unwrap_or_else(|_| tree.clone());
        worktree::remove(&repo, tree)?;
    }

    agent.remove()?;
    Ok(format!("{} forgotten", view.id()))
}

/// What the agent has changed, for the closer look.
///
/// Taken once, when somebody asks, and held: re-running `git diff` on every
/// reading would put a repository's whole worth of work behind a clock tick.
pub fn changes(root: &Path, view: &View) -> Result<Peek> {
    let mut patch = Vec::new();
    verbs::diff::run(root, view.id(), &mut patch)?;

    let patch = String::from_utf8_lossy(&patch).into_owned();
    Ok(Peek {
        id: view.id().to_string(),
        phase: view.phase(),
        question: None,
        body: match patch.trim().is_empty() {
            true => "nothing changed yet".to_string(),
            false => patch,
        },
        changes: true,
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

    #[test]
    fn a_line_says_what_it_is_for_before_anybody_types_into_it() {
        let mut composer = Composer::new(Asking::Task);
        assert_eq!(composer.prompt(), "task");
        composer.hidden = true;
        assert!(composer.prompt().contains("out of sight"), "somewhere else");

        let asking = Composer::new(Asking::Reply {
            id: "ask-a1b".to_string(),
            question: true,
        });
        assert!(
            asking.prompt().contains("answer ask-a1b"),
            "{}",
            asking.prompt()
        );
        assert!(
            asking.prompt().contains("y n 1-9 enter esc"),
            "and the only keys it takes: {}",
            asking.prompt()
        );

        let message = Composer::new(Asking::Reply {
            id: "fix-login-b2c".to_string(),
            question: false,
        });
        assert_eq!(message.prompt(), "message to fix-login-b2c");
    }

    #[test]
    fn axis_reads_a_line_of_nothing_but_tokens_as_a_narrowing() {
        assert_eq!(
            narrowing("s:working"),
            Some(vec![Narrow::State(Some("working".to_string()))])
        );
        assert_eq!(
            narrowing("a:port  s:waiting"),
            Some(vec![
                Narrow::Name(Some("port".to_string())),
                Narrow::State(Some("waiting".to_string())),
            ]),
            "as many as were typed, in the order they were typed"
        );
        assert_eq!(
            narrowing("s:"),
            Some(vec![Narrow::State(None)]),
            "and a token with nothing after it drops that one"
        );
    }

    #[test]
    fn axis_takes_a_line_with_a_colon_in_it_for_the_task_it_is() {
        for line in [
            "s:waiting is what to check",
            "port the importer",
            "fix s:waiting",
            "",
            "   ",
        ] {
            assert_eq!(narrowing(line), None, "{line:?} is a task, colons and all");
        }
    }

    #[test]
    fn axis_says_a_line_that_narrows_is_not_a_task() {
        let mut composer = Composer::new(Asking::Task);
        composer.text = "s:waiting".to_string();
        assert_eq!(composer.prompt(), "narrow");

        composer.text = "s:waiting is what to check".to_string();
        assert_eq!(composer.prompt(), "task", "and a task still is one");
    }

    #[test]
    fn what_a_verb_wrote_becomes_one_line() {
        assert_eq!(
            one_line(b"fix-login-a1b stopped\nremoved /srv/app/.amx/worktrees/fix-login-a1b\n"),
            "fix-login-a1b stopped · removed /srv/app/.amx/worktrees/fix-login-a1b"
        );
        assert_eq!(one_line(b""), "");
    }
}
