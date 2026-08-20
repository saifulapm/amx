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

use anyhow::{Context, Result};
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::cli::{Disposition, StopArgs};
use crate::store::{Agent, Meta, Phase};
use crate::tmux::{PaneId, Server};
use crate::{exit, paths, worktree};

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
    let server = Server::from_socket(meta.socket.clone());

    // Recorded before the signal, so the exit the signal causes is read as
    // what it is: an agent somebody stopped, not one that failed.
    let was = agent.state()?.state;
    if !was.is_terminal() {
        agent
            .writer()?
            .update_state(|state| state.state = Phase::Stopped)?;
    }

    end(&server, &meta.pane)?;
    writeln!(out, "{} stopped", args.id)?;

    dispositions(&meta, args, input, out)?;
    Ok(exit::OK)
}

/// Ask the agent to stop, then insist.
///
/// The pid comes from tmux, live, and is never read off disk: pids are reused,
/// and a stale one names whatever the machine has started since.
fn end(server: &Server, pane: &PaneId) -> Result<()> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if !server.pane_alive(pane) {
        return Ok(());
    }
    let group = Pid::from_raw(server.pane_pid(pane)?);

    // The whole group: the vendor forks, and a child holding the tty outlives
    // a parent that is signalled alone.
    let _ = killpg(group, Signal::SIGTERM);
    if gone(server, pane, GRACE) {
        return Ok(());
    }

    let _ = killpg(group, Signal::SIGKILL);
    if gone(server, pane, GRACE) {
        return Ok(());
    }

    // The process is gone and the pane is not: tmux's own way out.
    server.kill_pane(pane)
}

/// Whether the pane goes within `patience`.
fn gone(server: &Server, pane: &PaneId, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        if !server.pane_alive(pane) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !server.pane_alive(pane)
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
    // than of the tree: the tree is what may be about to go.
    let repo = worktree::main_repo(tree).unwrap_or_else(|_| tree.clone());

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
        worktree::remove(&repo, tree)?;
        writeln!(out, "removed {}", tree.display())?;
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
