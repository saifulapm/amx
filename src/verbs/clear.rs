//! `amx clear`: forget every agent that has ended.
//!
//! Where [`super::sweep`] takes only agents whose work landed, this
//! takes every ended row (stopped, failed, done), lists each with its reason,
//! and asks once for the whole list. A row whose work landed goes the sweep's
//! way (tree, branch and record); any other goes the way the view's `ctrl+x`
//! takes it (record and tree, branch kept). An idle agent is not ended: its
//! session can still take a turn.
//!
//! - A worktree holding uncommitted work is never removed, and neither is the
//!   record that names it.

use anyhow::{Result, bail};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::derive::{self, View};
use crate::store::{Agent, Meta};
use crate::tmux::Server;
use crate::verbs::{stop, sweep};
use crate::{exit, paths, spawn, store, worktree};

/// What taking one row came to.
pub enum Taken {
    /// The record and its tree are gone.
    Gone,
    /// Both stay: the tree holds uncommitted work or git would not remove it.
    Holding(PathBuf),
}

/// Run the verb against the machine.
pub fn from_env(force: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    run(&root, force, &mut input, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    force: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    let views = derive::views(root, store::now())?;
    let rows = finished_rows(&views);

    if rows.is_empty() {
        writeln!(out, "nothing to clear")?;
        return Ok(exit::OK);
    }

    for (at, why) in &rows {
        writeln!(out, "{}  {why}", views[*at].id())?;
    }
    if !force && !agreed(rows.len(), input, out)? {
        writeln!(out, "nothing cleared")?;
        return Ok(exit::OK);
    }

    // A row that fails is reported and the rest of the list still goes.
    let mut failed = false;
    for (at, _) in &rows {
        let view = &views[*at];
        match take_row(root, view) {
            Ok(Taken::Gone) => {}
            Ok(Taken::Holding(tree)) => writeln!(
                out,
                "kept {}: {} has uncommitted changes",
                view.id(),
                tree.display()
            )?,
            Err(e) => {
                failed = true;
                writeln!(out, "could not clear {}: {e:#}", view.id())?;
            }
        }
    }
    Ok(match failed {
        true => exit::FAILURE,
        false => exit::OK,
    })
}

/// The index of each ended view, with the reason it is listed.
///
/// The reason is the sweep's where the work landed (`#12 merged`), read from
/// what is written down and never from the network, since the view calls this
/// on a key press. Otherwise it is the phase.
pub fn finished_rows(views: &[View]) -> Vec<(usize, String)> {
    views
        .iter()
        .enumerate()
        .filter(|(_, view)| view.phase().is_terminal())
        .map(|(at, view)| {
            let why = sweep::why_landed(view).unwrap_or_else(|| view.phase().to_string());
            (at, why)
        })
        .collect()
}

/// Take one ended row: tree, branch and record where the work landed, else the
/// record and the tree with the branch kept.
///
/// `stop`'s per-agent output is discarded; both callers print one line per row.
pub fn take_row(root: &Path, view: &View) -> Result<Taken> {
    if sweep::why_landed(view).is_none() {
        return forget_row(root, view);
    }
    // Checked under the writer, then released: the stop behind the sweep
    // takes the writer itself.
    let meta = {
        let agent = Agent::open(root, view.id())?;
        let _writer = agent.writer()?;
        still_over(&agent)?
    };
    if let Some(tree) = holding(&meta) {
        return Ok(Taken::Holding(tree));
    }
    sweep::take_landed(root, &meta, &mut std::io::sink())?;
    // The stop keeps a tree git would not remove, and the record with it.
    Ok(match meta.worktree.as_ref().filter(|tree| tree.exists()) {
        Some(tree) => Taken::Holding(tree.clone()),
        None => {
            spawn::end_session(&Server::from_socket(meta.socket.clone()), &meta.id)?;
            Taken::Gone
        }
    })
}

/// Remove a row whose work did not land: its record and the tree amx cut.
///
/// A tree holding uncommitted work keeps both, since the record is what names
/// its branch and base.
///
/// The writer is held from the check until the record is gone. A resume
/// between the listing and the yes takes the writer too, so either it
/// finished first and the row reads as running, or it finds no record.
pub fn forget_row(root: &Path, view: &View) -> Result<Taken> {
    let agent = Agent::open(root, view.id())?;
    let _writer = agent.writer()?;
    let meta = still_over(&agent)?;

    if let Some(tree) = holding(&meta) {
        return Ok(Taken::Holding(tree));
    }
    if let Some(tree) = &meta.worktree
        && tree.exists()
    {
        let repo = worktree::main_repo(tree).unwrap_or_else(|_| tree.clone());
        if worktree::remove(&repo, tree).is_err() {
            return Ok(Taken::Holding(tree.clone()));
        }
        // And the tree's entry in the vendor's store, as `stop` does.
        stop::forget(&meta, tree, &mut std::io::sink())?;
    }

    spawn::end_session(&Server::from_socket(meta.socket.clone()), &meta.id)?;
    agent.remove()?;
    Ok(Taken::Gone)
}

/// The record's meta, or an error if the agent is no longer ended. Call under
/// the writer.
///
/// A live pane means the agent was resumed, whatever the phase says. With no
/// pane, a non-terminal phase reads as stopped unless the agent is parked.
fn still_over(agent: &Agent) -> Result<Meta> {
    let meta = agent.meta()?;
    let state = agent.state()?;
    if Server::from_socket(meta.socket.clone()).pane_answers_for(&meta.pane, &meta.id) {
        bail!("{}'s pane is open again", meta.id);
    }
    if !state.state.is_terminal() && state.parked_at > 0 {
        bail!("{} is now {}", meta.id, state.state);
    }
    Ok(meta)
}

/// The row's tree, if it holds uncommitted work (or git cannot say).
pub fn holding(meta: &Meta) -> Option<PathBuf> {
    let tree = meta.worktree.as_ref()?;
    stop::holds_work(tree).then(|| tree.clone())
}

/// Ask once for the whole list. Anything but yes, including no input, is no.
fn agreed(count: usize, input: &mut impl BufRead, out: &mut impl Write) -> Result<bool> {
    stop::confirm(&format!("clear {count}?"), false, input, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Phase, State};
    use crate::tmux::{PaneId, Socket, Spawn};
    use std::io::Read;
    use std::process::Command;
    use tempfile::TempDir;

    /// Run git with no user or system config and a fixed identity.
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "amx tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "amx tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with one commit in it.
    fn a_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.name", "amx tests"]);
        git(
            dir.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::write(dir.path().join("README.md"), "before\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "first"]);
        dir
    }

    /// A done agent in a clean tree cut in `repo`. With `landed`, its branch is
    /// on the record and already in main, so the row goes the sweep's way.
    fn a_finished_agent(root: &Path, repo: &Path, id: &str, landed: bool) -> PathBuf {
        let tree = worktree::create(repo, id, None).unwrap();
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
                dir: repo.to_path_buf(),
                worktree: Some(tree.path.clone()),
                branch: landed.then(|| tree.branch.clone()),
                base: Some(tree.base.clone()),
                // No server listens here, so no real pane can be signalled.
                socket: Socket::Name("amx-clear-tests".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
        )
        .unwrap();
        let state = State {
            state: Phase::Done,
            last_event: store::now(),
            since: store::now(),
            ..State::default()
        };
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();
        tree.path
    }

    /// Input that resumes `ids` (a new pane each, the record repointed under
    /// the writer) before the yes is read.
    struct ResumedFirst<'a> {
        root: &'a Path,
        server: &'a Server,
        ids: &'a [&'a str],
        typed: &'a [u8],
    }

    impl Read for ResumedFirst<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            for id in std::mem::take(&mut self.ids) {
                let (_, pane) = self
                    .server
                    .new_session(&Spawn {
                        name: Some(&format!("amx-{id}")),
                        command: &["sh", "-c", "while :; do sleep 0.05; done"],
                        ..Spawn::default()
                    })
                    .unwrap();
                let agent = Agent::open(self.root, id).unwrap();
                let writer = agent.writer().unwrap();
                writer
                    .update_meta(|meta| {
                        meta.socket = self.server.socket().clone();
                        meta.pane = pane;
                    })
                    .unwrap();
                writer
                    .update_state(|state| state.state = Phase::Starting)
                    .unwrap();
            }
            self.typed.read(buf)
        }
    }

    #[test]
    fn clear_keeps_a_row_resumed_after_the_list_was_printed_and_its_tree() {
        let repo = a_repo();
        let root = TempDir::new().unwrap();
        let forgotten = a_finished_agent(root.path(), repo.path(), "forgotten-a1b", false);
        let landed = a_finished_agent(root.path(), repo.path(), "landed-c3d", true);
        let left = a_finished_agent(root.path(), repo.path(), "left-e5f", false);

        let server =
            Server::named(format!("amx-test-clear-{}", std::process::id())).with_conf("/dev/null");
        let mut input = std::io::BufReader::new(ResumedFirst {
            root: root.path(),
            server: &server,
            ids: &["forgotten-a1b", "landed-c3d"],
            typed: b"y\n",
        });
        let mut out = Vec::new();
        let cleared = run(root.path(), false, &mut input, &mut out);
        let _ = server.kill();
        cleared.unwrap();
        let out = String::from_utf8(out).unwrap();

        // All three were listed: one landed, two by phase.
        assert!(
            out.contains("landed-c3d  amx/landed-c3d merged into main"),
            "{out}"
        );
        for id in ["forgotten-a1b", "left-e5f"] {
            assert!(out.contains(&format!("{id}  done")), "{out}");
        }
        // The two resumed rows keep their records and trees and stay running.
        for (id, tree) in [("forgotten-a1b", &forgotten), ("landed-c3d", &landed)] {
            assert!(tree.exists(), "{id}'s tree: {out}");
            let agent = Agent::open(root.path(), id).expect("its record");
            assert_eq!(agent.state().unwrap().state, Phase::Starting, "{id}");
            assert!(out.contains(&format!("could not clear {id}")), "{out}");
        }
        assert!(!left.exists(), "{out}");
        assert_eq!(store::list(root.path()).unwrap().len(), 2);
    }

    #[test]
    fn clear_kills_a_session_remain_on_exit_left_standing_for_the_record() {
        // A dead pane kept by `remain-on-exit`: no longer the record's pane,
        // but holding the session name a resume would open.
        let repo = a_repo();
        let root = TempDir::new().unwrap();
        a_finished_agent(root.path(), repo.path(), "lingers-a1b", false);
        let server =
            Server::named(format!("amx-test-linger-{}", std::process::id())).with_conf("/dev/null");
        let (session, _) = server
            .new_session(&Spawn {
                name: Some("amx-lingers-a1b"),
                command: &["sh", "-c", "sleep 0.3"],
                ..Spawn::default()
            })
            .unwrap();
        server
            .set_session_option(&session, "remain-on-exit", "on")
            .unwrap();
        let agent = Agent::open(root.path(), "lingers-a1b").unwrap();
        agent
            .writer()
            .unwrap()
            .update_meta(|meta| {
                meta.socket = server.socket().clone();
                meta.pane = PaneId::new("%99").unwrap();
            })
            .unwrap();

        let mut out = Vec::new();
        let cleared = run(root.path(), true, &mut &b""[..], &mut out);
        let standing = server.session_named("amx-lingers-a1b");
        let _ = server.kill();
        let out = String::from_utf8(out).unwrap();

        assert_eq!(cleared.unwrap(), exit::OK, "{out}");
        assert!(store::list(root.path()).unwrap().is_empty(), "{out}");
        assert_eq!(standing.unwrap(), None, "{out}");
    }
}
