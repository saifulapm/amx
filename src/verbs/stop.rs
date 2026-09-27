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

use anyhow::Result;
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

    // This one alone: a parent's children carry on, and a child's parent is
    // never the child's to end. A family is stopped one id at a time.
    if !stop_one(root, &args.id, &meta, out)? {
        writeln!(
            out,
            "{} was resumed while it was being stopped; left it running",
            args.id
        )?;
        return Ok(exit::FAILURE);
    }

    dispositions(&meta, args, input, out)?;

    // Last, and only once everything it names has been said. The record is
    // where the worktree and the branch are written down, so a line about
    // either of them has to be printed while there is still a record to print
    // it from.
    // A tree that stayed — holding work no commit has, or one git would not
    // remove — is named nowhere but the record, so the record stays with it.
    if args.delete {
        match meta.worktree.as_ref().filter(|tree| tree.exists()) {
            Some(tree) => writeln!(
                out,
                "kept {}'s record: {} is still there",
                args.id,
                tree.display()
            )?,
            None => {
                agent.remove()?;
                writeln!(out, "removed {}'s record", args.id)?;
            }
        }
    }
    Ok(exit::OK)
}

/// End one agent: take its pane down, mark it stopped where it is not already,
/// and run whatever the person asked to run at that moment.
///
/// Everything under the writer, from the reading to the write: an exit landing
/// while stop decides has either written its phase already, and stop leaves it
/// alone, or waits until stop has written its own. The pane first and the
/// record after, so a pane that would not go leaves the phase as it was.
///
/// `read` is the record the caller decided from. A pane on the record that is
/// not the one read is a resume that landed in between, and that agent is one
/// nobody asked to stop: false, and nothing touched.
fn stop_one(root: &Path, id: &str, read: &Meta, out: &mut impl Write) -> Result<bool> {
    stop_one_ending(root, id, read, out, end)
}

/// The same, with the way a pane is ended handed in, so a test can say what
/// tmux did.
fn stop_one_ending(
    root: &Path,
    id: &str,
    read: &Meta,
    out: &mut impl Write,
    end: impl FnOnce(&Server, &PaneId, &str) -> Result<()>,
) -> Result<bool> {
    let agent = Agent::open(root, id)?;
    let writer = agent.writer()?;
    let meta = agent.meta()?;
    if (&meta.socket, &meta.pane) != (&read.socket, &read.pane) {
        return Ok(false);
    }
    let server = Server::from_socket(meta.socket.clone());

    let was = writer.state()?.state;
    end(&server, &meta.pane, &meta.id)?;
    // Still under the writer, so the exit the signal causes waits for this
    // and reads it as what it is: an agent somebody stopped, not one that
    // failed.
    if !was.is_terminal() {
        writer.update_state_heard(agent.heartbeat(), |state| state.state = Phase::Stopped)?;
        drop(writer);
        stopped(&agent, &meta);
    }

    writeln!(out, "{id} stopped")?;
    Ok(true)
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

    if !server.answers_for_now(pane, id)? {
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
/// went, or because the number is somebody else's now. A tmux that could not
/// be asked has not said so.
fn gone(server: &Server, pane: &PaneId, id: &str, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        if matches!(server.answers_for_now(pane, id), Ok(false)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    matches!(server.answers_for_now(pane, id), Ok(false))
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

    // Commits no other branch has go with the branch, and nothing asked here
    // can bring them back: such a branch is kept whatever was asked, and the
    // count is the reason given. A branch at exactly the head a request was
    // merged from lost nothing, whatever the forge merged it as.
    let merged = crate::pr::merged_heads_written(meta);
    if let Ok(n @ 1..) = worktree::loses(&repo, branch, &merged) {
        let commits = match n {
            1 => "1 commit is".to_string(),
            n => format!("{n} commits are"),
        };
        writeln!(out, "kept {branch}: {commits} on no other branch")?;
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
        match worktree::delete_branch(&repo, branch, &merged) {
            Ok(()) => writeln!(out, "deleted {branch}")?,
            Err(why) => writeln!(out, "kept {branch}: {why:#}")?,
        }
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

    /// A record of an agent at `phase`, in a pane nothing here ever asks
    /// about: these tests hand stop the ending, so no server is needed.
    fn record(root: &Path, id: &str, pane: &str, phase: Phase) -> Agent {
        let agent = Agent::create(
            root,
            &Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                model: None,
                effort: None,
                dir: std::path::PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: crate::tmux::Socket::Name("amx-test-stop-nobody".to_string()),
                pane: PaneId::new(pane).unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: store::now(),
            },
        )
        .expect("the record");
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = phase;
                state.since = 1_000;
                state.last_event = 1_000;
            })
            .unwrap();
        agent
    }

    #[test]
    fn stop_whose_ending_fails_leaves_the_phase_as_it_was() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = record(dir.path(), "stuck-a1b", "%3", Phase::Working);
        let read = agent.meta().unwrap();

        let mut out = Vec::new();
        let stopped = stop_one_ending(dir.path(), "stuck-a1b", &read, &mut out, |_, _, _| {
            anyhow::bail!("tmux would not kill it")
        });

        assert!(stopped.is_err(), "the failure is the answer");
        let state = agent.state().unwrap();
        assert_eq!(state.state, Phase::Working);
        assert_eq!(state.since, 1_000, "{state:?}");
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn stop_refuses_a_tmux_that_cannot_be_asked_and_touches_nothing() {
        // No answer about the pane is not a pane gone: stopping on it would
        // write Stopped over a live agent and take its tree with it.
        let dir = tempfile::TempDir::new().unwrap();
        let tree = tempfile::TempDir::new().unwrap();
        let agent = record(dir.path(), "live-a1b", "%3", Phase::Working);
        let writer = agent.writer().unwrap();
        writer
            .update_meta(|meta| {
                meta.socket = crate::tmux::unaskable();
                meta.worktree = Some(tree.path().to_path_buf());
                meta.branch = Some("amx/live-a1b".to_string());
            })
            .unwrap();
        drop(writer);

        let args = StopArgs {
            id: "live-a1b".to_string(),
            force: true,
            delete: true,
            worktree: Some(Disposition::Delete),
            branch: Some(Disposition::Delete),
        };
        let mut out = Vec::new();
        let why = run(dir.path(), &args, &mut "".as_bytes(), &mut out).unwrap_err();

        assert!(
            format!("{why:#}").starts_with("tmux could not be asked: "),
            "{why:#}"
        );
        assert_eq!(agent.state().unwrap().state, Phase::Working);
        assert!(tree.path().is_dir(), "the tree stays");
        assert!(agent.dir().is_dir(), "and so does the record");
        assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    }

    #[test]
    fn stop_racing_an_exit_leaves_done() {
        // The exit hook holds the writer while stop is on its way in, and
        // writes Done before letting go. What stop decides, it decides from
        // what the exit left.
        let dir = tempfile::TempDir::new().unwrap();
        let agent = record(dir.path(), "done-a1b", "%3", Phase::Working);
        let read = agent.meta().unwrap();

        let exiting = agent.writer().unwrap();
        let mut out = Vec::new();
        std::thread::scope(|scope| {
            let stopping = scope.spawn(|| {
                stop_one_ending(dir.path(), "done-a1b", &read, &mut out, |_, _, _| Ok(()))
            });
            std::thread::sleep(Duration::from_millis(200));
            exiting
                .update_state(|state| {
                    state.exit = Some(0);
                    state.state = Phase::Done;
                })
                .unwrap();
            drop(exiting);
            assert!(
                stopping.join().unwrap().unwrap(),
                "the record was the one read"
            );
        });

        assert_eq!(agent.state().unwrap().state, Phase::Done);
    }

    #[test]
    fn stop_leaves_a_record_resumed_since_it_was_read_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = record(dir.path(), "back-a1b", "%3", Phase::Stopped);
        let read = agent.meta().unwrap();

        // `amx resume` lands between the read and the stop: a new pane, and
        // the record reset for the session it opens.
        let writer = agent.writer().unwrap();
        writer
            .update_meta(|meta| meta.pane = PaneId::new("%9").unwrap())
            .unwrap();
        writer
            .update_state(|state| state.state = Phase::Idle)
            .unwrap();
        drop(writer);

        let mut out = Vec::new();
        let stopped = stop_one_ending(dir.path(), "back-a1b", &read, &mut out, |_, pane, _| {
            panic!("{pane} is the resumed agent's, and nothing ends it")
        })
        .unwrap();

        assert!(!stopped);
        assert_eq!(agent.state().unwrap().state, Phase::Idle);
        assert_eq!(agent.meta().unwrap().pane, PaneId::new("%9").unwrap());
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
