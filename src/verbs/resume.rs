//! `amx resume` — start an agent's command again, on the session it had.
//!
//! A resume is a continuation, not a second agent: the id, the directory, the
//! branch and the event log are the ones the agent already had, and the only
//! thing that changes about it is the pane it lives in. What makes that
//! possible is the vendor's own session, recorded from a hook the first time
//! the agent announced one — without one there is nothing to pick up, and
//! saying so beats starting the task over.
//!
//! Two orderings here are the whole of the verb's correctness. The record is
//! put back to `starting` **before** the pane exists, because a pane starts
//! hooking the moment it does and a record that still says the agent ended
//! would turn those hooks away — including the one carrying the new session
//! id. And the pane is placed **before** the record learns where it is, since
//! there is no pane id to record until tmux has made one.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::spawn::{self, Handoff, Placement};
use crate::store::{Agent, Event, Meta, Phase, State};
use crate::tmux::Server;
use crate::{derive, exit, paths, rules, store, worktree};

/// What amx records when it brings an agent back.
const RESUMED: &str = "resume";

/// The vendor's flag for a session to continue.
const RESUME: &str = "--resume";

/// The vendor's flag for a session to *start*, under an id the caller chose.
const START_SESSION: &str = "--session-id";

/// Every way of writing a flag that decides which session the vendor opens.
///
/// These are the words a resume replaces rather than carries. Everything else
/// the agent was started with is the agent's, not the first turn's, and goes
/// with it: a directory it was given access to is one it still needs.
const NAMES_A_SESSION: [&str; 3] = [START_SESSION, RESUME, "-r"];

/// Run the verb against the machine.
pub fn from_env(config: &Config, id: Option<&str>, all: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    run(&root, config, id, all, &env, &mut out)
}

/// The verb, with everything it reads named.
pub fn run(
    root: &Path,
    config: &Config,
    id: Option<&str>,
    all: bool,
    env: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<i32> {
    match id {
        Some(id) if !all => one(root, config, id, env, out),
        _ => sweep(root, config, env, out),
    }
}

/// One agent, named.
fn one(
    root: &Path,
    config: &Config,
    id: &str,
    env: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<i32> {
    let view = derive::view(root, id, rules::bundled(), store::now())?;
    // Anything that has not ended is already doing what a resume would start.
    // Starting a second command over the top of it is the one outcome nobody
    // asked for.
    if !view.phase().is_terminal() {
        eprintln!(
            "amx resume: {id} is {}. stop it before starting it again",
            view.phase()
        );
        return Ok(exit::BLOCKED);
    }
    if let Some(full) = at_capacity(root, config)? {
        eprintln!("amx resume: {full}");
        return Ok(exit::BLOCKED);
    }

    bring_back(root, id, env)?;
    writeln!(out, "{id} resumed")?;
    Ok(exit::OK)
}

/// Every agent whose pane is gone — the morning after a tmux server died.
///
/// Only the stopped ones: an agent that ran to the end of its command has
/// nothing outstanding, and a sweep that started every finished agent on the
/// machine would be a way to lose an afternoon.
fn sweep(
    root: &Path,
    config: &Config,
    env: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<i32> {
    let stopped: Vec<_> = derive::views(root, rules::bundled(), store::now())?
        .into_iter()
        .filter(|view| view.phase() == Phase::Stopped)
        .collect();
    if stopped.is_empty() {
        writeln!(out, "nothing to bring back")?;
        return Ok(exit::OK);
    }

    for view in stopped {
        if let Some(full) = at_capacity(root, config)? {
            eprintln!("amx resume: {full}");
            return Ok(exit::BLOCKED);
        }
        // One agent that cannot come back is not the sweep's ending. The
        // others still can, and this is the command somebody runs when the
        // whole wall went at once.
        match bring_back(root, view.id(), env) {
            Ok(()) => writeln!(out, "{} resumed", view.id())?,
            Err(e) => eprintln!("amx resume: {}: {e:#}", view.id()),
        }
    }
    Ok(exit::OK)
}

/// Whether the machine is already running as many agents as it will.
fn at_capacity(root: &Path, config: &Config) -> Result<Option<String>> {
    let live = spawn::live(root)?.len();
    Ok((live >= config.max_agents).then(|| {
        format!(
            "{live} agents already running, and max_agents is {}",
            config.max_agents
        )
    }))
}

/// Put the agent back in a pane, continuing what it was doing.
///
/// All of it happens under the agent's writer lock. The has-it-ended check
/// and the respawn are one action: of two resumes racing, the second waits
/// at the lock and then reads the `starting` state and the live pane the
/// first wrote, so one session is never continued into two panes. The gates
/// in [`one`] and [`sweep`] are for saying so politely; this one is for
/// being right.
fn bring_back(root: &Path, id: &str, env: &BTreeMap<String, String>) -> Result<()> {
    let agent = Agent::open(root, id)?;
    let writer = agent.writer()?;

    // The raw record rather than the derived view, which may take this very
    // lock to note a question it read off a pane. An agent has ended when its
    // record says so, or when the pane the record names is gone.
    let current = writer.state()?;
    let meta = agent.meta()?;
    if !current.state.is_terminal()
        && Server::from_socket(meta.socket.clone()).pane_alive(&meta.pane)
    {
        bail!("{id} is already going again");
    }

    let Some(session) = meta.session.as_deref() else {
        bail!(
            "no session was ever recorded for {id}, so there is nothing to continue. \
             start a fresh agent with `amx new`"
        );
    };
    // The recorded id was written from a hook payload, and this is the one
    // place it becomes part of a command line: it is checked at the moment it
    // is used, not only at the moment it was written down.
    if !is_session_id(session) {
        bail!("the session recorded for {id} is not a session id, so it will not be handed on");
    }

    let recorded = spawn::read_handoff(agent.dir())
        .with_context(|| format!("reading what {id} was started with"))?;
    let dir = ready_dir(&meta)?;

    // The vendor is the one the agent was started with, and the environment is
    // the one this command was run with — the same rule `new` follows, because
    // an hour-old environment is nobody's idea of the one to run in.
    let mut env = env.clone();
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_handoff(
        agent.dir(),
        &Handoff {
            task: recorded.task.clone(),
            command: continuing(&recorded, session),
            env,
        },
    )?;

    writer.append(&Event::new(
        RESUMED,
        serde_json::json!({ "session": session }),
    ))?;
    // Everything the last turn left behind goes with it: an answer from before
    // the agent stopped is not this session's answer, and an exit code is not
    // how a running command ended. The count of messages sent stays, because
    // the log it counts is still the agent's own.
    writer.update_state(|state| {
        *state = State {
            seq: state.seq,
            ..State::default()
        }
    })?;

    let server = spawn::server(root)?;
    let boot = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    // Where it was started is where it comes back: an agent put out of sight
    // was put there on purpose.
    let placement = match meta.bg {
        true => Placement::Background,
        false => Placement::Wall,
    };
    let pane = spawn::place(&server, placement, id, &dir, &boot, &spawn::wall_lock(root))?;

    // Still under the writer taken at the top: a hook the new pane fires
    // waits at the lock until the pane is on the record, and update_meta
    // reads before it writes, so nothing a hook recorded earlier is lost.
    writer.update_meta(|meta| {
        meta.socket = server.socket().clone();
        meta.pane = pane;
    })?;
    Ok(())
}

/// The vendor's argv for a session it already has.
///
/// The agent is launched with what it was launched with the first time, minus
/// the two things this command decides for itself.
///
/// The **task** goes: it was put to this session in its first turn, and handing
/// it over again would ask for the work twice. Every **flag naming a session**
/// goes with it — `--session-id`, because it asks the vendor to *start* a
/// session under a chosen id, which is the opposite instruction to the one this
/// command carries, and `--resume`, because a resumed agent's recorded command
/// already carries the one the last resume wrote. Two of them would leave which
/// session the vendor opens up to the vendor.
///
/// `--resume=<id>` then arrives as a single token, because that is the one
/// spelling with no ambiguity about where the value is.
fn continuing(handoff: &Handoff, session: &str) -> Vec<String> {
    let mut words = handoff.command.clone().into_iter().peekable();
    let mut command: Vec<String> = Vec::new();

    while let Some(word) = words.next() {
        // Only the last word is the task, which is where `new` put it.
        if words.peek().is_none() && word == handoff.task {
            break;
        }
        let Some(value_is_a_word_of_its_own) = names_a_session(&word) else {
            command.push(word);
            continue;
        };
        // The value goes with the flag it belongs to. A word that begins with
        // `-` is never one: the vendor documents `--resume`'s value as
        // optional, and an optional value is not taken from a word that could
        // be a flag in its own right.
        if value_is_a_word_of_its_own && words.peek().is_some_and(|next| !next.starts_with('-')) {
            words.next();
        }
    }

    command.push(format!("{RESUME}={session}"));
    command
}

/// Whether a word is a flag naming a session, and if so whether its value is
/// the word after it rather than joined on with `=`.
fn names_a_session(word: &str) -> Option<bool> {
    NAMES_A_SESSION.iter().find_map(|flag| {
        if word == *flag {
            return Some(true);
        }
        word.strip_prefix(flag)
            .is_some_and(|rest| rest.starts_with('='))
            .then_some(false)
    })
}

/// Where the agent runs, put back if it is not there any more.
///
/// The tree `stop` removes by default is the tree a resume needs, and nothing
/// was lost when it went: `stop` never removes a tree holding work no commit
/// has, so everything that was in it is on the branch. Checking that branch out
/// again is the whole of the repair.
fn ready_dir(meta: &Meta) -> Result<PathBuf> {
    if meta.dir.is_dir() {
        return Ok(meta.dir.clone());
    }
    match (&meta.worktree, &meta.branch) {
        (Some(tree), Some(branch)) if tree == &meta.dir => {
            worktree::restore(&repo_above(tree)?, tree, branch)
                .with_context(|| format!("putting {} back", tree.display()))?;
            Ok(meta.dir.clone())
        }
        _ => bail!(
            "{} is gone, and it is where {} ran",
            meta.dir.display(),
            meta.id
        ),
    }
}

/// The repository a tree that is no longer there belonged to: the nearest
/// directory above it that still exists, and the repository holding that.
fn repo_above(tree: &Path) -> Result<PathBuf> {
    let mut above = tree.parent();
    while let Some(dir) = above {
        if dir.is_dir() {
            return worktree::repo_root(dir)?
                .with_context(|| format!("{} is in no repository any more", dir.display()));
        }
        above = dir.parent();
    }
    bail!("nothing is left of {}", tree.display())
}

/// Whether a recorded session id is one.
///
/// A word amx is about to hand the vendor as an argument, checked for being a
/// word and nothing else: an id that could read as a flag, or that carries
/// anything but the characters an id is made of, is not passed on.
fn is_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff(command: &[&str], task: &str) -> Handoff {
        Handoff {
            task: task.to_string(),
            command: command.iter().map(|word| word.to_string()).collect(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn resume_asks_the_vendor_to_continue_the_session_it_opened() {
        let started = handoff(
            &["claude", "--model", "opus", "fix the login bug"],
            "fix the login bug",
        );
        assert_eq!(
            continuing(&started, "abc-123"),
            ["claude", "--model", "opus", "--resume=abc-123"],
            "the flag and its value are one word: the value is optional, and a \
             separate one would be read as a flag of its own"
        );
    }

    #[test]
    fn resume_does_not_put_the_task_a_second_time() {
        // A task that looks like a flag, and one that appears twice: only the
        // last word is the task, because that is where `new` put it.
        let started = handoff(&["claude", "--model", "--model"], "--model");
        assert_eq!(
            continuing(&started, "abc"),
            ["claude", "--model", "--resume=abc"]
        );
    }

    #[test]
    fn resume_drops_the_flag_that_would_start_a_session_instead() {
        for started in [
            handoff(
                &["claude", "--session-id", "abc-123", "--model", "opus", "go"],
                "go",
            ),
            handoff(
                &["claude", "--session-id=abc-123", "--model", "opus", "go"],
                "go",
            ),
        ] {
            assert_eq!(
                continuing(&started, "abc-123"),
                ["claude", "--model", "opus", "--resume=abc-123"],
                "{:?}",
                started.command
            );
        }
    }

    #[test]
    fn clibatch_resume_carries_everything_the_agent_was_started_with() {
        // The arguments are the agent's, not the first turn's: a directory it
        // was given access to is one it still needs.
        let started = handoff(
            &[
                "claude",
                "--model",
                "opus",
                "--add-dir",
                "/srv/data",
                "--verbose",
                "port the importer",
            ],
            "port the importer",
        );
        assert_eq!(
            continuing(&started, "abc-123"),
            [
                "claude",
                "--model",
                "opus",
                "--add-dir",
                "/srv/data",
                "--verbose",
                "--resume=abc-123"
            ]
        );
    }

    #[test]
    fn clibatch_resuming_twice_asks_for_one_session_and_not_two() {
        // Each resume records what it launched, so the next one reads a command
        // that already carries a `--resume`. Handing the vendor two of them
        // leaves which session it opens up to the vendor.
        let started = handoff(&["claude", "--add-dir", "/srv/data", "go"], "go");
        let after_one = Handoff {
            command: continuing(&started, "abc-123"),
            ..started
        };
        assert_eq!(
            continuing(&after_one, "def-456"),
            ["claude", "--add-dir", "/srv/data", "--resume=def-456"]
        );

        // However it was written the first time, including by hand after the
        // separator on `amx new`.
        for written in [
            &["claude", "--resume", "old", "go"][..],
            &["claude", "--resume=old", "go"],
            &["claude", "-r", "old", "go"],
        ] {
            let started = handoff(written, "go");
            assert_eq!(
                continuing(&started, "def-456"),
                ["claude", "--resume=def-456"],
                "{written:?}"
            );
        }

        // The value is optional, so the word after one is only its value when
        // it could be: a flag after `--resume` is a flag, and it stays.
        let started = handoff(&["claude", "--resume", "--verbose", "go"], "go");
        assert_eq!(
            continuing(&started, "def-456"),
            ["claude", "--verbose", "--resume=def-456"]
        );
    }

    #[test]
    fn resume_hands_on_a_session_id_and_nothing_else() {
        assert!(is_session_id("6f1c9f4e-0d5b-4a51-9f6e-2b1f0c3d4e5a"));
        assert!(is_session_id("abc_123"));

        assert!(!is_session_id(""));
        assert!(!is_session_id("--dangerously-skip-permissions"));
        assert!(!is_session_id("abc 123"));
        assert!(!is_session_id("$(rm -rf /)"));
        assert!(!is_session_id("../../elsewhere"));
        assert!(!is_session_id(&"a".repeat(65)));
    }
}
