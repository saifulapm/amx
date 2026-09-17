//! `amx stop` — end an agent, and decide what it leaves behind.
//!
//! Ending it is a ladder, not a killing: the pane's process group is asked to
//! stop, given a moment to finish writing whatever it was writing, and only
//! then killed. A vendor cut down mid-sentence loses the transcript it was
//! flushing, and that transcript is where an answer lives.
//!
//! What it leaves behind is the person's to decide, with the defaults being
//! the ones that lose nothing: the worktree goes, the branch stays, the record
//! stays. A worktree with uncommitted work in it is never deleted, whatever
//! anybody says — it holds work that no commit has, and deleting that is the
//! one thing amx could do that nothing undoes.
//!
//! `--delete` is the record's disposition, and it is not `--force`. One says
//! that this row goes; the other answers every question with its default.
//! Keeping them apart is what lets somebody clear a finished agent away
//! without also telling amx they do not care what happens to a worktree.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::cli::{Disposition, StopArgs};
use crate::store::{Agent, Meta, Phase};
use crate::tmux::{PaneId, Server};
use crate::{exit, paths, spawn, store, trust, warn, worktree};

/// How long the agent is given to stop of its own accord.
const GRACE: Duration = Duration::from_secs(5);

/// Run the verb against the machine.
pub fn from_env(args: &StopArgs) -> Result<i32> {
    let root = paths::state_root()?;
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    run(&root, args, &mut input, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    args: &StopArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    let agent = Agent::open(root, &args.id)?;
    let meta = agent.meta()?;

    stop_one(root, &args.id, out)?;

    // The family goes with the parent, deepest first, unless somebody asked
    // for it to stand: a child was started to answer the parent's questions,
    // and one left running has nobody to answer to.
    if !args.keep_children {
        for child in descendants(root, &args.id)? {
            stop_one(root, &child, out)?;
        }
    }

    dispositions(&meta, args, input, out)?;

    // Last, and only once everything it names has been said. The record is
    // where the worktree and the branch are written down, so a line about
    // either of them has to be printed while there is still a record to print
    // it from.
    if args.delete {
        agent.remove()?;
        writeln!(out, "removed {}'s record", args.id)?;
    }
    Ok(exit::OK)
}

/// End one agent: mark it stopped where it is not already, run whatever the
/// person asked to run at that moment, and take its pane down.
///
/// A whole rung per agent rather than every record and then every pane,
/// because the family is ended parent first: a child is written down as
/// stopped while its parent is still there.
fn stop_one(root: &Path, id: &str, out: &mut impl Write) -> Result<()> {
    let agent = Agent::open(root, id)?;
    let meta = agent.meta()?;
    let server = Server::from_socket(meta.socket.clone());

    // Recorded before the signal, so the exit the signal causes is read as
    // what it is: an agent somebody stopped, not one that failed.
    let was = agent.state()?.state;
    if !was.is_terminal() {
        agent
            .writer()?
            .update_state(|state| state.state = Phase::Stopped)?;
        stopped(&agent, &meta);
    }

    end(&server, &meta.pane, &meta.id)?;
    writeln!(out, "{id} stopped")?;
    Ok(())
}

/// Every agent whose record names its way back to `id`, deepest first.
///
/// Read off the records rather than kept anywhere: parenthood is a field, and
/// a record whose parent has been removed is nobody's descendant. Deepest
/// first so the family is ended from the leaves up, a child never left running
/// after the thing it was answering to has gone.
///
/// A cycle — a record naming itself its own parent, or two naming each other —
/// is walked once and stops there: an agent is written down once, and there is
/// nothing below the record that repeats.
fn descendants(root: &Path, id: &str) -> Result<Vec<String>> {
    let mut records = Vec::new();
    for other in store::list(root)? {
        if let Ok(meta) = Agent::open(root, &other).and_then(|agent| agent.meta()) {
            records.push(meta);
        }
    }

    let mut found: Vec<(u32, String)> = Vec::new();
    let mut seen = BTreeSet::from([id.to_string()]);
    let mut frontier = vec![(id.to_string(), 0u32)];
    while let Some((parent, depth)) = frontier.pop() {
        for meta in &records {
            if meta.parent.as_deref() == Some(parent.as_str()) && seen.insert(meta.id.clone()) {
                found.push((depth + 1, meta.id.clone()));
                frontier.push((meta.id.clone(), depth + 1));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(found.into_iter().map(|(_, id)| id).collect())
}

/// Run whatever somebody asked to have run when an agent is stopped.
///
/// Only where this verb is what wrote the phase, which is what the caller has
/// just decided: an agent that had already ended reached this moment somewhere
/// else, or never reached it at all.
///
/// Here rather than at the end of the verb, because what is left of the verb is
/// a pane being ended and a worktree that may be about to go — and the worktree
/// is where the command runs. Nothing is appended to the event log for it:
/// nothing has happened to the agent that the record does not already say, so
/// the line the command reads is built rather than read back.
///
/// The person's own config, because this verb is handed none — and the
/// project's own file is read by [`crate::errand::assembled`], which is the one
/// key of it that matters here.
fn stopped(agent: &Agent, meta: &Meta) {
    let event = store::Event::new("stop", serde_json::json!({}));
    let config = crate::config::current();
    if let Some(errand) = crate::errand::assembled(config, agent, meta, Phase::Stopped, &event) {
        // Nobody is asked whether anybody is looking at the pane: this verb is
        // closing it.
        crate::notify::start(&errand, None);
    }
}

/// Ask the agent to stop, then insist.
///
/// The pid comes from tmux, live, and is never read off disk: pids are reused,
/// and a stale one names whatever the machine has started since. The pane is
/// asked whose it is for the same reason: tmux hands pane numbers out again,
/// so a record that outlived its server names whichever pane took its number,
/// and every rung below is a signal or a kill aimed at whatever is standing
/// there. An agent whose pane answers for somebody else has already lost it,
/// and there is nothing here left to end.
///
/// Shared with `_park`, which takes an idle agent's pane and leaves the record
/// standing: how a vendor is ended is the same question there, and a second
/// answer to it would be a second thing to get the grace period wrong in.
pub(crate) fn end(server: &Server, pane: &PaneId, id: &str) -> Result<()> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if !server.pane_answers_for(pane, id) {
        return Ok(());
    }
    let group = Pid::from_raw(server.pane_pid(pane)?);

    // The whole group: the vendor forks, and a child holding the tty outlives
    // a parent that is signalled alone.
    let _ = killpg(group, Signal::SIGTERM);
    if gone(server, pane, id, GRACE) {
        return Ok(());
    }

    let _ = killpg(group, Signal::SIGKILL);
    if gone(server, pane, id, GRACE) {
        return Ok(());
    }

    // The process is gone and the pane is not: tmux's own way out.
    server.kill_pane(pane)
}

/// Whether the pane stops being this agent's within `patience` — because it
/// went, or because the number is somebody else's now.
fn gone(server: &Server, pane: &PaneId, id: &str, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        if !server.pane_answers_for(pane, id) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !server.pane_answers_for(pane, id)
}

/// What becomes of the worktree and the branch.
fn dispositions(
    meta: &Meta,
    args: &StopArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<()> {
    let (Some(tree), Some(branch)) = (&meta.worktree, &meta.branch) else {
        return Ok(());
    };
    // Asked before anything is removed, and asked of the repository rather
    // than of the tree: the tree is what may be about to go. When it has gone
    // already, git has nothing to answer from inside it — and a tree amx cut
    // says where its repository is by where it sits.
    let repo = worktree::main_repo(tree)
        .ok()
        .or_else(|| worktree::repo_of(tree))
        .unwrap_or_else(|| tree.clone());

    // Work nobody has committed is not amx's to delete, and saying so is part
    // of the answer: somebody has to know it is still there.
    if tree.exists() && worktree::is_dirty(tree).unwrap_or(true) {
        writeln!(
            out,
            "keeping {}: it holds work no commit has",
            tree.display()
        )?;
    } else if !asked(
        args.worktree,
        args.force,
        Disposition::Delete,
        &format!("delete the worktree {}?", tree.display()),
        input,
        out,
    )?
    .is_keep()
    {
        // Saying so beats failing, for the same reason the branch below says
        // so: the agent is already stopped, and the lines still to be printed
        // are the record's — including, under `--delete`, its removal.
        match worktree::remove(&repo, tree) {
            Ok(()) => {
                writeln!(out, "removed {}", tree.display())?;
                forget(meta, tree, out)?;
            }
            Err(why) => writeln!(out, "kept {}: {why:#}", tree.display())?,
        }
    } else {
        writeln!(out, "kept {}", tree.display())?;
    }

    // A branch cannot go while a worktree has it checked out, and a tree that
    // was kept still has it. Saying so is the answer; failing is not, because
    // the agent is already stopped by now.
    if tree.exists() {
        writeln!(
            out,
            "kept {branch}: {} still has it checked out",
            tree.display()
        )?;
        return Ok(());
    }

    if !asked(
        args.branch,
        args.force,
        Disposition::Keep,
        &format!("delete the branch {branch}?"),
        input,
        out,
    )?
    .is_keep()
    {
        worktree::delete_branch(&repo, branch).with_context(|| format!("deleting {branch}"))?;
        writeln!(out, "deleted {branch}")?;
    } else {
        writeln!(out, "kept {branch}")?;
    }
    Ok(())
}

/// Take the tree amx has just removed back out of the vendor's own store.
///
/// A vendor that keeps a project entry per directory it runs in gathers one
/// per agent, and nothing of the vendor's ever clears them: the directory the
/// entry names has gone, and the entry is still there saying it may be worked
/// in. Only the tree's own key, and only for the vendor whose store amx wrote
/// in the first place.
///
/// The store is looked for in the environment `stop` was typed in with the
/// harness table's pairs laid over it, which is where the vendor looked for it
/// when the agent ran. Failing to write it is worth saying and not worth
/// stopping for: the agent is already ended, and what is left is a key in a
/// file nobody is about to read.
///
/// Shared with the view's own forget, which takes a finished agent's tree
/// without going through the ladder above: a tree that goes takes its key
/// whichever door it went through.
pub(crate) fn forget(meta: &Meta, tree: &Path, out: &mut impl Write) -> Result<()> {
    let agent = meta.agent.as_deref().unwrap_or_default();
    if !trust::writes_a_store(agent) {
        return Ok(());
    }
    let mut env = spawn::env_snapshot(std::env::vars());
    spawn::harness_env(&mut env, &crate::config::for_dir(&meta.dir).0, agent);
    let Some(store) = trust::store_in(&env) else {
        return Ok(());
    };
    match trust::forget_tree(&store, tree, store::now()) {
        Ok(true) => writeln!(out, "forgot {} in {}", tree.display(), store.display())?,
        Ok(false) => {}
        Err(why) => warn!("amx stop: {why:#}"),
    }
    Ok(())
}

/// The answer to one disposition: the flag if there was one, the default if
/// nobody is to be asked, and otherwise the person.
fn asked(
    told: Option<Disposition>,
    force: bool,
    fallback: Disposition,
    question: &str,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<Disposition> {
    if let Some(told) = told {
        return Ok(told);
    }
    if force {
        return Ok(fallback);
    }

    let hint = match fallback {
        Disposition::Delete => "[Y/n]",
        Disposition::Keep => "[y/N]",
    };
    write!(out, "{question} {hint} ")?;
    out.flush()?;

    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        // Nobody there to ask: the default is the answer.
        writeln!(out)?;
        return Ok(fallback);
    }
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Disposition::Delete,
        "n" | "no" => Disposition::Keep,
        _ => fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(
        told: Option<Disposition>,
        force: bool,
        fallback: Disposition,
        typed: &str,
    ) -> Disposition {
        let mut out = Vec::new();
        asked(
            told,
            force,
            fallback,
            "delete it?",
            &mut typed.as_bytes(),
            &mut out,
        )
        .unwrap()
    }

    #[test]
    fn stop_a_flag_is_the_answer_and_nobody_is_asked() {
        let mut out = Vec::new();
        let answer = asked(
            Some(Disposition::Keep),
            false,
            Disposition::Delete,
            "delete it?",
            &mut "y\n".as_bytes(),
            &mut out,
        )
        .unwrap();
        assert_eq!(answer, Disposition::Keep);
        assert!(out.is_empty(), "and nothing was asked: {out:?}");
    }

    #[test]
    fn stop_force_takes_the_default_without_asking() {
        let mut out = Vec::new();
        assert_eq!(
            asked(
                None,
                true,
                Disposition::Delete,
                "delete it?",
                &mut "n\n".as_bytes(),
                &mut out
            )
            .unwrap(),
            Disposition::Delete
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn stop_a_typed_answer_decides() {
        assert_eq!(
            ask(None, false, Disposition::Delete, "n\n"),
            Disposition::Keep
        );
        assert_eq!(
            ask(None, false, Disposition::Keep, "y\n"),
            Disposition::Delete
        );
        assert_eq!(
            ask(None, false, Disposition::Keep, "yes\n"),
            Disposition::Delete
        );
    }

    #[test]
    fn stop_the_default_answers_for_a_shrug() {
        // Enter, something that is not an answer, or nobody there at all.
        for typed in ["\n", "maybe\n", ""] {
            assert_eq!(
                ask(None, false, Disposition::Delete, typed),
                Disposition::Delete,
                "{typed:?}"
            );
            assert_eq!(
                ask(None, false, Disposition::Keep, typed),
                Disposition::Keep,
                "{typed:?}"
            );
        }
    }

    #[test]
    fn stop_the_question_shows_which_way_enter_goes() {
        let mut out = Vec::new();
        asked(
            None,
            false,
            Disposition::Delete,
            "delete the worktree?",
            &mut "\n".as_bytes(),
            &mut out,
        )
        .unwrap();
        let asked = String::from_utf8(out).unwrap();
        assert!(asked.contains("delete the worktree?"), "{asked}");
        assert!(asked.contains("[Y/n]"), "{asked}");
    }
}
