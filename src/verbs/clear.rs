//! `amx clear` — forget the rows that are over.
//!
//! [`sweep`](super::sweep) takes the agents somebody else established are done
//! with: a request the forge settled, a branch git reads as in the main line.
//! Most of what fills a wall is none of those. A row somebody stopped, a
//! command that ran and exited, an agent that was never given a branch to land
//! anything on — nothing outside amx will ever have an opinion about them, so
//! nothing outside amx can say when their records go. Without this verb they
//! go one `ctrl+x` or one `amx stop --delete` at a time, which on a wall of
//! sixty is why they do not go at all.
//!
//! So the two verbs are the same shape over different lists: everything
//! finished is listed with the reason it is finished, one question covers the
//! list, and then each row is taken the way its own evidence says to — the
//! sweep's way where the work landed, and otherwise the way `ctrl+x` takes one
//! row, which is the record and a tree amx cut, with the branch left standing.
//!
//! An agent sitting at its prompt is not finished. It has a session somebody
//! can still send a turn to, and the whole cost of leaving it on the wall is a
//! row; the whole cost of getting it wrong is a conversation nobody can reach
//! again.
//!
//! The one law it will not break is `stop`'s: a tree holding work no commit
//! has is never removed, and neither is the record that names it.

use anyhow::{Result, bail};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::derive::{self, View};
use crate::store::{Agent, Meta};
use crate::tmux::Server;
use crate::verbs::{stop, sweep};
use crate::{exit, paths, store, worktree};

/// What taking one row came to.
pub enum Taken {
    /// The record is gone, and the tree amx cut with it.
    Gone,
    /// Both are still here, because this tree holds work no commit has or git
    /// would not remove it.
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

    // A row that will not go is said and passed by: the rest of the list is
    // still what was asked for.
    let mut failed = false;
    for (at, _) in &rows {
        let view = &views[*at];
        match take_row(root, view) {
            Ok(Taken::Gone) => {}
            Ok(Taken::Holding(tree)) => writeln!(
                out,
                "kept {}: {} is still there, and so is its record",
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

/// Which of these rows are finished, and why each one is on the list.
///
/// Where the work landed the reason is the sweep's own — `#12 merged` — read
/// off what the last look wrote down and never off a network: this is what the
/// view presses for too, and a press must not stand still while a forge
/// answers. Everywhere else the reason is the phase, which is all there is to
/// say about a row that stopped.
///
/// The index rather than the view, because the caller has the views and the
/// view has no place on the wall until somebody counts them.
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

/// Take one finished row the way its own evidence says to.
///
/// Work that landed goes the sweep's way — the tree, the branch and the record
/// together, since the repository holds every commit that was on it. Work that
/// did not keeps its branch: nothing here says it is safe to lose, and a
/// branch costs a line in `git branch`.
///
/// What the ladder says as it goes is dropped. Both doors this is behind print
/// their own sentence about the whole list, and neither has room for the run
/// of lines `stop` writes per agent.
pub fn take_row(root: &Path, view: &View) -> Result<Taken> {
    if sweep::why_landed(view).is_none() {
        return forget_row(root, view);
    }
    // Asked again under the writer, which is then let go: the stop behind the
    // sweep takes it for itself. What it would read is what was just asked.
    let meta = {
        let agent = Agent::open(root, view.id())?;
        let _writer = agent.writer()?;
        still_over(&agent)?
    };
    if let Some(tree) = holding(&meta) {
        return Ok(Taken::Holding(tree));
    }
    sweep::take_landed(root, &meta, &mut std::io::sink())?;
    // The stop behind the sweep keeps a tree git would not remove, and the
    // record with it.
    Ok(match meta.worktree.as_ref().filter(|tree| tree.exists()) {
        Some(tree) => Taken::Holding(tree.clone()),
        None => Taken::Gone,
    })
}

/// Forget a row whose work went nowhere: its record, and the tree amx gave it.
///
/// A tree holding work no commit has keeps both. Its record is where the
/// branch and the commit that tree was cut from are named, and a tree nothing
/// names is work nobody will find again.
///
/// The writer is taken before anything is decided and held until the record
/// is gone. The row was read before somebody said yes, and a resume in
/// between takes the writer too: it either finished first and the record
/// reads as running, or it waits and finds nothing to resume.
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
        // And its key in the vendor's store with it, the way `stop` takes it:
        // the caller has one line to say what happened to the whole list.
        stop::forget(&meta, tree, &mut std::io::sink())?;
    }

    agent.remove()?;
    Ok(Taken::Gone)
}

/// The record as it is now, if it is still finished; asked under its writer.
///
/// Finished the way the reader reads it: a pane that still answers for the
/// agent is somebody's to read whatever the phase on the record says, and is
/// what a resume leaves behind it. With no pane, a phase that is not terminal
/// reads as stopped unless amx let the pane go and means to bring it back.
fn still_over(agent: &Agent) -> Result<Meta> {
    let meta = agent.meta()?;
    let state = agent.state()?;
    if Server::from_socket(meta.socket.clone()).pane_answers_for(&meta.pane, &meta.id) {
        bail!("{}'s pane is still there", meta.id);
    }
    if !state.state.is_terminal() && state.parked_at > 0 {
        bail!("{} is {} now", meta.id, state.state);
    }
    Ok(meta)
}

/// The tree this row will not give up, if it has one.
///
/// A tree amx cannot read is read as dirty: the answer that keeps the work is
/// the answer to give when git will not say.
fn holding(meta: &Meta) -> Option<PathBuf> {
    let tree = meta.worktree.as_ref()?;
    (tree.exists() && worktree::is_dirty(tree).unwrap_or(true)).then(|| tree.clone())
}

/// The one question, asked once for the whole list.
///
/// It reads the way every other question amx asks reads: the default is the
/// one that loses nothing, and anything that is not plainly yes — a shrug, a
/// typo, nobody there at all — takes it.
fn agreed(count: usize, input: &mut impl BufRead, out: &mut impl Write) -> Result<bool> {
    write!(out, "clear {count}? [y/N] ")?;
    out.flush()?;

    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        writeln!(out)?;
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Phase, State};
    use crate::tmux::{PaneId, Socket, Spawn};
    use std::io::Read;
    use std::process::Command;
    use tempfile::TempDir;

    /// git as the tests run it: none of the developer's own configuration and
    /// an identity of its own.
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

    /// An agent that has finished, in a clean tree of its own cut in `repo`.
    /// With its branch named the branch is in the main line, so the row goes
    /// the sweep's way; without, it is forgotten.
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
                // A socket no tmux server on this machine answers on: nothing
                // here has a pane, and nothing here may signal one that is
                // somebody's.
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

    /// Somebody at the prompt who, before typing yes, resumes these agents
    /// from another shell: a pane placed for each, then the record pointed at
    /// it and reset, under the writer.
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

        // All three were on the list, and were finished when it was read:
        // one the sweep's way and two by their phase.
        assert!(
            out.contains("landed-c3d  amx/landed-c3d merged into main"),
            "{out}"
        );
        for id in ["forgotten-a1b", "left-e5f"] {
            assert!(out.contains(&format!("{id}  done")), "{out}");
        }
        // The two that came back keep their records and their trees, and
        // stay running.
        for (id, tree) in [("forgotten-a1b", &forgotten), ("landed-c3d", &landed)] {
            assert!(tree.exists(), "{id}'s tree: {out}");
            let agent = Agent::open(root.path(), id).expect("its record");
            assert_eq!(agent.state().unwrap().state, Phase::Starting, "{id}");
            assert!(out.contains(&format!("could not clear {id}")), "{out}");
        }
        // The one nobody touched went.
        assert!(!left.exists(), "{out}");
        assert_eq!(store::list(root.path()).unwrap().len(), 2);
    }
}
