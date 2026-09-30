//! `amx stop`: end an agent and decide what happens to its worktree, branch
//! and record.
//!
//! The pane's process group gets SIGTERM and a grace period before SIGKILL, so
//! the vendor can finish flushing its transcript. The defaults lose nothing:
//! the worktree is removed, the branch and record are kept. `--delete` removes
//! the record; `--force` takes every default without asking. The two are
//! separate so a finished agent can be cleared without also waiving the
//! worktree questions.
//!
//! - A worktree holding uncommitted work is never deleted, and a record is
//!   never removed while its worktree still exists.

use anyhow::Result;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use nix::sys::signal::Signal;

use crate::cli::{Disposition, StopArgs};
use crate::store::{Agent, Meta, Phase};
use crate::tmux::{PaneId, Server};
use crate::{exit, paths, spawn, store, trust, warn, worktree};

/// How long the agent gets to stop on its own, per signal.
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

    // This agent only: its children and its parent keep running.
    if !stop_one(root, &args.id, &meta, out)? {
        writeln!(
            out,
            "{} was resumed before it could be stopped; it is still running",
            args.id
        )?;
        return Ok(exit::FAILURE);
    }

    dispositions(&meta, args, input, out)?;

    // Last, since the lines above are printed from the record. A tree that
    // stayed is named only by the record, so the record stays with it.
    if args.delete {
        match meta.worktree.as_ref().filter(|tree| tree.exists()) {
            Some(tree) => writeln!(
                out,
                "kept {}'s record: its worktree {} still exists",
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

/// End one agent: take its pane down, mark it stopped if it had not ended, and
/// run the `on_stopped` command.
///
/// `read` is the record the caller decided from. If the record now names a
/// different pane, a resume landed in between: returns false and touches
/// nothing.
fn stop_one(root: &Path, id: &str, read: &Meta, out: &mut impl Write) -> Result<bool> {
    let agent = Agent::open(root, id)?;
    let signal = read
        .agent
        .as_deref()
        .and_then(crate::registry::entry)
        .and_then(|vendor| vendor.interrupt_signal);
    if !turn_ended(&agent, read, signal, GRACE, signalled)? {
        warn!("amx stop: {}", no_ending(read));
    }
    stop_one_ending(root, id, read, out, end)
}

/// For a vendor with an interrupt signal, send it and wait up to `patience`
/// for the record to leave the turn. False only when the signal went out and
/// no ending came.
///
/// Runs before the writer is taken, since the ending arrives as a hook that
/// writes under it. A vendor killed mid-turn can leave the session claimed:
/// opencode's service holds a killed server's claim until it boots again.
///
/// `send` returns whether the signal was delivered.
fn turn_ended(
    agent: &Agent,
    read: &Meta,
    signal: Option<Signal>,
    patience: Duration,
    send: impl FnOnce(&Server, &PaneId, &str, Signal) -> Result<bool>,
) -> Result<bool> {
    let in_a_turn = |phase: Phase| matches!(phase, Phase::Working | Phase::Waiting);
    let Some(signal) = signal else {
        return Ok(true);
    };
    let meta = agent.meta()?;
    // A resume since the read is reported by `stop_one_ending`.
    if (&meta.socket, &meta.pane) != (&read.socket, &read.pane) || !in_a_turn(agent.state()?.state)
    {
        return Ok(true);
    }
    let server = Server::from_socket(meta.socket.clone());
    if !send(&server, &meta.pane, &meta.id, signal)? {
        return Ok(true);
    }
    let deadline = Instant::now() + patience;
    loop {
        if !in_a_turn(agent.state()?.state) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Send `signal` to the vendor in this agent's pane, if the pane is still
/// this agent's.
///
/// The pane's own process is the wrapper shell that records how the vendor
/// ended; a signal with its default action would kill it. The vendor is its
/// child.
fn signalled(server: &Server, pane: &PaneId, id: &str, signal: Signal) -> Result<bool> {
    if !server.answers_for_now(pane, id)? {
        return Ok(false);
    }
    let mut sent = false;
    for child in children(server.pane_pid(pane)?) {
        sent |= nix::sys::signal::kill(nix::unistd::Pid::from_raw(child), signal).is_ok();
    }
    Ok(sent)
}

/// The child pids of `pid`, from `/proc`. Empty if unreadable.
fn children(pid: i32) -> Vec<i32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|child| child.parse().ok())
        .collect()
}

/// The warning for a turn that did not end when signalled, naming the session
/// that may stay claimed, since the next resume opens it.
fn no_ending(meta: &Meta) -> String {
    let session = match &meta.session {
        Some(session) => format!("session {session}"),
        None => "its session".to_string(),
    };
    format!(
        "{}'s turn did not end within {}s; closing its pane anyway, so {session} may still be marked as running",
        meta.id,
        GRACE.as_secs()
    )
}

/// [`stop_one`] after the turn is dealt with, with the pane ending passed in
/// for tests.
///
/// Holds the writer from the check to the write: an exit landing meanwhile has
/// either written its phase already, which is left alone, or waits for this
/// one. The pane goes first, so a pane that will not go leaves the phase
/// unchanged.
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
    // Still under the writer, so the exit the kill causes waits and reads the
    // agent as stopped, not failed.
    if !was.is_terminal() {
        writer.update_state_heard(agent.heartbeat(), |state| state.state = Phase::Stopped)?;
        drop(writer);
        stopped(&agent, &meta);
    }

    writeln!(out, "{id} stopped")?;
    Ok(true)
}

/// Start the `on_stopped` command for an agent this verb stopped.
///
/// Called only when this verb wrote the phase, and before the worktree may be
/// removed, since the command runs there. The event it is handed is built, not
/// logged. The person's config is used; [`crate::errand::assembled`] reads the
/// project's file itself.
fn stopped(agent: &Agent, meta: &Meta) {
    let event = store::Event::new("stop", serde_json::json!({}));
    let config = crate::config::current();
    if let Some(errand) = crate::errand::assembled(config, agent, meta, Phase::Stopped, &event) {
        // No check for a watched pane: this verb is closing it.
        crate::notify::start(&errand, None);
    }
}

/// End the agent's pane: SIGTERM to its process group, then SIGKILL, then
/// `kill-pane`, each after [`GRACE`].
///
/// The pid is read from tmux, never from disk, since pids are reused. The pane
/// must still answer for this agent, since tmux reuses pane numbers too; a
/// pane that answers for another agent is left alone. Also used by `_park`.
pub(crate) fn end(server: &Server, pane: &PaneId, id: &str) -> Result<()> {
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;

    if !server.answers_for_now(pane, id)? {
        return Ok(());
    }
    let group = Pid::from_raw(server.pane_pid(pane)?);

    // The whole group: a forked child holding the tty outlives its parent.
    let _ = killpg(group, Signal::SIGTERM);
    if gone(server, pane, id, GRACE) {
        return Ok(());
    }

    let _ = killpg(group, Signal::SIGKILL);
    if gone(server, pane, id, GRACE) {
        return Ok(());
    }

    // The process is gone but the pane is not.
    server.kill_pane(pane)
}

/// Whether the pane stops answering for this agent within `patience`. A tmux
/// that cannot be asked counts as no.
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
    // Resolved before anything is removed, since the tree may go. A tree that
    // is already gone is placed by the path amx cut it at.
    let repo = worktree::main_repo(tree)
        .ok()
        .or_else(|| worktree::repo_of(tree))
        .unwrap_or_else(|| tree.clone());

    if holds_work(tree) {
        writeln!(
            out,
            "keeping {}: it has uncommitted changes",
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
        // Reported, not an error: the agent is already stopped and the rest of
        // the output still has to be printed.
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

    // A branch checked out in a kept tree cannot be deleted.
    if tree.exists() {
        writeln!(
            out,
            "kept {branch}: {} still has it checked out",
            tree.display()
        )?;
        return Ok(());
    }

    // A branch with commits on no other branch is kept whatever was asked. A
    // branch at a head a request was merged from loses nothing, however the
    // forge merged it.
    let merged = crate::pr::merged_heads_written(meta);
    if let Ok(n @ 1..) = worktree::loses(&repo, branch, &merged) {
        let commits = match n {
            1 => "1 commit is".to_string(),
            n => format!("{n} commits are"),
        };
        writeln!(out, "kept {branch}: {commits} not on any other branch")?;
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

/// Whether `tree` is there and holds work no commit has. A tree git cannot
/// read counts as holding some.
pub(crate) fn holds_work(tree: &Path) -> bool {
    tree.exists() && worktree::is_dirty(tree).unwrap_or(true)
}

/// Remove a deleted tree's entry from the vendor's own project store.
///
/// A vendor that keeps one entry per directory would otherwise collect one per
/// agent forever. Only the tree's key, and only for a vendor whose store amx
/// writes. The store is found in this process's environment with the harness
/// table's variables applied, as the agent saw it. A failed write is a warning.
/// Also used by `clear` and the view.
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
        Ok(true) => writeln!(out, "removed {} from {}", tree.display(), store.display())?,
        Ok(false) => {}
        Err(why) => warn!("amx stop: {why:#}"),
    }
    Ok(())
}

/// One disposition: the flag if given, the default under `--force`, else the
/// person's answer.
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
    Ok(
        match confirm(question, fallback == Disposition::Delete, input, out)? {
            true => Disposition::Delete,
            false => Disposition::Keep,
        },
    )
}

/// Ask a yes/no question on `out`. An empty line, anything that is not yes or
/// no, and no input at all are `default`.
pub(crate) fn confirm(
    question: &str,
    default: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<bool> {
    let hint = match default {
        true => "[Y/n]",
        false => "[y/N]",
    };
    write!(out, "{question} {hint} ")?;
    out.flush()?;

    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        writeln!(out)?;
        return Ok(default);
    }
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default,
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

    /// An agent at `phase`. The tests pass in the pane ending, so no server is
    /// needed.
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
        // An unaskable tmux is not a gone pane: stopping would mark a live
        // agent stopped and take its tree.
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
            format!("{why:#}").starts_with("listing the tmux panes: "),
            "{why:#}"
        );
        assert_eq!(agent.state().unwrap().state, Phase::Working);
        assert!(tree.path().is_dir(), "the tree stays");
        assert!(agent.dir().is_dir(), "and so does the record");
        assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    }

    #[test]
    fn stop_racing_an_exit_leaves_done() {
        // The exit hook holds the writer and writes Done before stop gets it.
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
    fn stop_signals_a_turn_to_end_and_waits_for_the_ending() {
        // A vendor with a signal is told to end its turn, and stop waits for
        // the record to leave it.
        let signal = crate::vendor::second::ELSEWHERE.interrupt_signal;
        for phase in [Phase::Working, Phase::Waiting] {
            let dir = tempfile::TempDir::new().unwrap();
            let agent = record(dir.path(), "busy-a1b", "%3", phase);
            let read = agent.meta().unwrap();

            let mut sent = None;
            let ended = std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(Duration::from_millis(100));
                    agent
                        .writer()
                        .unwrap()
                        .update_state(|state| state.state = Phase::Idle)
                        .unwrap();
                });
                turn_ended(&agent, &read, signal, GRACE, |_, pane, id, signal| {
                    sent = Some((pane.clone(), id.to_string(), signal));
                    Ok(true)
                })
                .unwrap()
            });

            assert!(ended, "{phase}");
            assert_eq!(
                sent,
                Some((
                    PaneId::new("%3").unwrap(),
                    "busy-a1b".to_string(),
                    nix::sys::signal::Signal::SIGUSR1
                )),
                "{phase}"
            );
        }
    }

    #[test]
    fn stop_sends_no_signal_where_there_is_no_turn_or_no_signal() {
        let dir = tempfile::TempDir::new().unwrap();
        let signal = crate::vendor::second::ELSEWHERE.interrupt_signal;
        for phase in [Phase::Idle, Phase::Starting, Phase::Done, Phase::Stopped] {
            let agent = record(dir.path(), &format!("{phase}-a1b"), "%3", phase);
            let read = agent.meta().unwrap();
            let ended = turn_ended(&agent, &read, signal, GRACE, |_, _, _, _| {
                panic!("{phase} has no turn to end")
            });
            assert!(ended.unwrap(), "{phase}");
        }

        let agent = record(dir.path(), "claude-a1b", "%3", Phase::Working);
        let read = agent.meta().unwrap();
        let ended = turn_ended(&agent, &read, None, GRACE, |_, _, _, _| {
            panic!("a vendor with no signal is ended as it stands")
        });
        assert!(ended.unwrap());
    }

    #[test]
    fn stop_warns_naming_the_session_when_no_ending_comes() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = record(dir.path(), "deaf-a1b", "%3", Phase::Working);
        let read = agent.meta().unwrap();
        let signal = crate::vendor::second::ELSEWHERE.interrupt_signal;

        let started = Instant::now();
        let ended = turn_ended(
            &agent,
            &read,
            signal,
            Duration::from_millis(200),
            |_, _, _, _| Ok(true),
        )
        .unwrap();
        assert!(!ended);
        assert!(started.elapsed() >= Duration::from_millis(200));

        let named = no_ending(&Meta {
            session: Some("ses_4f2a".to_string()),
            ..read.clone()
        });
        assert!(named.contains("deaf-a1b"), "{named}");
        assert!(named.contains("ses_4f2a"), "{named}");
        assert_eq!(GRACE, Duration::from_secs(5));
    }

    #[test]
    fn stop_signals_the_vendor_under_the_panes_wrapper_not_the_wrapper() {
        // The pane runs `sh -c '"$0" "$@"; amx _exit ...'`. Signalling that
        // shell would kill it before it records the exit; the vendor is its
        // child.
        let mut wrapper = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#""$0" "$@"; true"#)
            .args(["sleep", "30"])
            .spawn()
            .unwrap();
        let pid = wrapper.id() as i32;
        let mut under = Vec::new();
        for _ in 0..100 {
            under = children(pid);
            if !under.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let comm = |pid: i32| std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap();
        assert_eq!(under.len(), 1, "{under:?}");
        assert_eq!(comm(under[0]).trim(), "sleep");

        let _ = wrapper.kill();
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(under[0]),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = wrapper.wait();
    }

    #[test]
    fn stop_leaves_a_record_resumed_since_it_was_read_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = record(dir.path(), "back-a1b", "%3", Phase::Stopped);
        let read = agent.meta().unwrap();

        // A resume between the read and the stop: new pane, reset state.
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
        // Enter, an unrecognised answer, or no input at all.
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
