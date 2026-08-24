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
use std::path::{Path, PathBuf};

use super::paint::Card;
use super::rows::Narrow;
use crate::cli::{AgentArgs, NewArgs, StopArgs};
use crate::config::Config;
use crate::derive::View;
use crate::store::{Agent, Kind, Phase};
use crate::tmux::Server;
use crate::{derive, exit, registry, rules, spawn, store, verbs, worktree};

/// A line somebody is typing, and what it is for.
pub struct Composer {
    pub asking: Asking,
    pub text: String,
}

/// What entering the line will do.
pub enum Asking {
    /// A task, for an agent that does not exist yet.
    Task,
    /// Something for an agent that is already running: a message, or the key
    /// its question is waiting for.
    Reply { id: String, question: bool },
    /// A name for one of them, which goes nowhere near the agent itself.
    Name { id: String },
}

impl Composer {
    pub fn new(asking: Asking) -> Composer {
        Composer {
            asking,
            text: String::new(),
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
            Asking::Task => "task".to_string(),
            Asking::Reply { id, question: true } => format!("answer {id} · y n 1-9 enter esc"),
            Asking::Reply { id, .. } => format!("message to {id}"),
            Asking::Name { id } => format!("rename {id}"),
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

/// The tokens a task line may be led with, and what each of them turns.
const DIALS: [&str; 4] = ["m:", "p:", "w:", "agent:"];

/// What a line's leading tokens turn, for the one spawn they lead. Empty is
/// the ordinary line, which leaves every dial where the config put it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Turned {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub permission: Option<String>,
    /// Whether this agent is given a tree of its own, when the line said.
    pub worktree: Option<bool>,
}

/// What starting an agent came to.
pub enum Started {
    /// It is running, and this says which one.
    Yes(String),
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
pub fn turned(config: &Config, line: &str) -> Result<(Turned, String), String> {
    let (tokens, task) = tokens(line);
    let mut turned = Turned::default();

    // Which vendor first: which dials exist at all is its answer, and a line
    // may name one the config does not.
    for (dial, value) in &tokens {
        if *dial == "agent:" {
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
            "agent:" => {}
            "w:" => {
                turned.worktree = Some(match *value {
                    "on" => true,
                    "off" => false,
                    _ => return Err(format!("w:{value}: on or off")),
                });
            }
            "m:" => {
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

/// Start an agent on what was typed, where the view is.
pub fn start(root: &Path, config: &Config, line: &str) -> Result<Started> {
    let (turned, task) = match turned(config, line) {
        Ok(read) => read,
        Err(refusal) => return Ok(Started::No(refusal)),
    };
    if task.trim().is_empty() {
        return Ok(Started::No(
            "the dials are turned; now say what to do".to_string(),
        ));
    }

    let dir = std::env::current_dir().context("no working directory")?;
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
        agent: named.then_some(dials),
        vendor_args: Vec::new(),
    };

    let (mut started, mut refused) = (Vec::new(), Vec::new());
    let code = verbs::new::run(
        root,
        &dir,
        spawn::env_snapshot(std::env::vars()),
        &config,
        &args,
        &mut started,
        &mut refused,
    )?;
    if code != exit::OK {
        return Ok(Started::No(one_line(&refused)));
    }
    Ok(Started::Yes(format!("started {}", one_line(&started))))
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
/// amx's: the choices are read as choices wherever they are typed, and words
/// are an answer only at the one prompt that offers a field to put them in.
/// Words typed at a permission box would land on whatever is highlighted,
/// which is an answer nobody chose, so they are refused before a byte of them
/// reaches the pane.
pub fn reply(root: &Path, id: &str, text: &str) -> Result<Replied> {
    let view = derive::view(root, id, rules::bundled(), store::now())?;
    let agent = Agent::open(root, id)?;
    let server = Server::from_socket(view.meta.socket.clone());

    match view.phase() {
        Phase::Waiting => {
            if let Some(key) = verbs::answer::named(text) {
                verbs::answer::press(&agent, &server, &view.meta.pane, &key)?;
                return Ok(Replied::Yes(format!("answered {id}")));
            }
            match view.kind() {
                Some(Kind::Question) if !text.trim().is_empty() => {
                    verbs::answer::say(&agent, &server, &view.meta.pane, text.trim())?;
                    Ok(Replied::Yes(format!("answered {id}")))
                }
                kind => Ok(Replied::No(format!(
                    "{id} is asking: {}",
                    invitation(kind, &view.state.options)
                ))),
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

/// What this question will take, in the words the card invites it with — and
/// the words it is refused in, which are the same words for the same reason.
///
/// Only what is true of the prompt in front of somebody: the choices it drew,
/// numbered as every surface numbers them, and a field for words of their own
/// where the vendor asked its own question. A permission box has no such
/// field, so the invitation to type at it is never written; a question whose
/// choices amx has not read yet does not name numbers it cannot stand behind.
pub fn invitation(kind: Option<Kind>, options: &[String]) -> String {
    let choices = match options.len() {
        0 => None,
        1 => Some("press 1".to_string()),
        many => Some(format!("press 1-{}", many.min(9))),
    };
    match (choices, kind) {
        (Some(choices), Some(Kind::Question)) => format!("{choices}, or type an answer"),
        (Some(choices), _) => format!("{choices}, y or n"),
        (None, Some(Kind::Question)) => "type an answer".to_string(),
        (None, _) => "press y, n or 1-9".to_string(),
    }
}

/// What a rename came to.
pub enum Renamed {
    /// The wall calls it something else now, and this says what.
    Yes(String),
    /// It is called what it was called, and this says why.
    No(String),
}

/// The longest name a row will carry. Past this the column that holds it cuts,
/// and a name that only reads whole in the line it was typed on is not a name
/// on the wall.
const NAME: usize = 24;

/// Call the agent under the cursor something else.
///
/// The id is untouched. It is what the record is filed under, what a shell
/// addresses, and what the pane, the branch and the tree amx cut are named
/// after — an id that moved would leave every one of those pointing at a name
/// nothing answers to. What a rename changes is the word on the row, which is
/// the thing a person reads a hundred times a day.
///
/// What is typed is made safe where it is written down: a name goes on a row,
/// into a notice and back into a line somebody is editing, and a record that
/// never held a control character cannot hand one to any of them.
pub fn rename(root: &Path, id: &str, typed: &str) -> Result<Renamed> {
    // A control character becomes the space it stands in for rather than
    // nothing at all: a name pasted over two lines is two words, and dropping
    // the newline outright would run them into one.
    let spaced: String = typed
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.as_str();
    if name.is_empty() {
        return Ok(Renamed::No("a name is a word, not nothing".to_string()));
    }
    if name.chars().count() > NAME {
        return Ok(Renamed::No(format!(
            "a name is {NAME} characters at most, so that a row can carry it"
        )));
    }

    // Written the way a reading is written rather than as something the agent
    // said: a name is a fact about the wall, and moving the record's own clock
    // for it would have the next reader trust this document over the pane.
    let agent = Agent::open(root, id)?;
    agent
        .writer()?
        .observe(|state| state.name = (name != id).then(|| name.to_string()))?;
    Ok(Renamed::Yes(format!("{id} is {name}")))
}

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
fn forget(root: &Path, view: &View) -> Result<String> {
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
        age: view.verdict.age,
        question: None,
        options: Vec::new(),
        kind: None,
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
    use crate::store::Meta;
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// An agent with a record under `root`, for the acts that change one.
    fn recorded(root: &Path, id: &str) -> Agent {
        Agent::create(
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
                bg: false,
                session: None,
                transcript: None,
                created: store::now(),
            },
        )
        .unwrap()
    }

    #[test]
    fn acts_rename_puts_the_name_on_the_record_and_leaves_the_id_where_it_was() {
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");

        let Renamed::Yes(said) = rename(root.path(), "fix-login-a1b", "  auth\u{7}  ").unwrap()
        else {
            panic!("the rename was refused")
        };
        assert!(said.contains("auth"), "{said}");
        assert_eq!(
            agent.state().unwrap().name.as_deref(),
            Some("auth"),
            "trimmed, and without the characters a terminal reads as an \
             instruction rather than a letter"
        );
        assert_eq!(
            agent.meta().unwrap().id,
            "fix-login-a1b",
            "the id is what everything else addresses, and a rename is not \
             about the id"
        );

        rename(root.path(), "fix-login-a1b", "auth\nfix").unwrap();
        assert_eq!(
            agent.state().unwrap().name.as_deref(),
            Some("auth fix"),
            "and a name pasted over two lines is the two words it is"
        );
    }

    #[test]
    fn acts_rename_refuses_what_no_row_could_carry() {
        let root = TempDir::new().unwrap();
        let agent = recorded(root.path(), "fix-login-a1b");
        let refused = |typed: &str| match rename(root.path(), "fix-login-a1b", typed).unwrap() {
            Renamed::No(why) => why,
            Renamed::Yes(said) => panic!("{typed:?} was taken: {said}"),
        };

        assert!(refused("   ").contains("a name"), "nothing is not a name");
        assert!(
            refused(&"x".repeat(NAME + 1)).contains(&NAME.to_string()),
            "and one no row can draw whole is refused with the length in it"
        );
        assert_eq!(agent.state().unwrap().name, None, "and nothing was written");

        // A name that is the id is the name it already had, so the record goes
        // back to holding none.
        rename(root.path(), "fix-login-a1b", "auth").unwrap();
        rename(root.path(), "fix-login-a1b", "fix-login-a1b").unwrap();
        assert_eq!(agent.state().unwrap().name, None);
    }

    #[test]
    fn a_line_says_what_it_is_for_before_anybody_types_into_it() {
        let composer = Composer::new(Asking::Task);
        assert_eq!(composer.prompt(), "task");

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
    fn card_invites_the_answers_the_prompt_in_front_of_somebody_will_take() {
        let two = ["the sqlite one".to_string(), "the docker one".to_string()];
        let one = ["Yes".to_string()];

        // A question of the vendor's own offers choices and a field.
        assert_eq!(
            invitation(Some(Kind::Question), &two),
            "press 1-2, or type an answer"
        );
        assert_eq!(
            invitation(Some(Kind::Question), &[]),
            "type an answer",
            "and a menu whose choices amx has not read yet names none"
        );

        // A permission box and the trust screen read one key, so the card
        // never invites words at either.
        for kind in [Some(Kind::Permission), Some(Kind::Trust), None] {
            assert_eq!(invitation(kind, &two), "press 1-2, y or n", "{kind:?}");
            assert_eq!(invitation(kind, &one), "press 1, y or n", "{kind:?}");
            assert_eq!(
                invitation(kind, &[]),
                "press y, n or 1-9",
                "with nothing read off the screen, the grammar itself: {kind:?}"
            );
        }
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

    /// A config whose vendor is the one the registry declares dials for.
    fn as_claude() -> Config {
        Config {
            agent: "claude".to_string(),
            ..Config::default()
        }
    }

    #[test]
    fn composer_reads_the_dials_off_the_front_of_a_task_line() {
        let (dials, task) = turned(&as_claude(), "m:opus p:plan w:off port the importer").unwrap();
        assert_eq!(
            dials,
            Turned {
                agent: None,
                model: Some("opus".to_string()),
                permission: Some("plan".to_string()),
                worktree: Some(false),
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
    fn what_a_verb_wrote_becomes_one_line() {
        assert_eq!(
            one_line(b"fix-login-a1b stopped\nremoved /srv/app/.amx/worktrees/fix-login-a1b\n"),
            "fix-login-a1b stopped · removed /srv/app/.amx/worktrees/fix-login-a1b"
        );
        assert_eq!(one_line(b""), "");
    }
}
