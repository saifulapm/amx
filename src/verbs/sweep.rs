//! `amx sweep` — forget the agents whose work is in.
//!
//! An agent that finished leaves three things behind: a record, a tree and a
//! branch. Each of them is worth keeping while the work is still going
//! somewhere — the record holds the answer, the tree holds the diff somebody
//! may still want to read, the branch holds the commits. Once the request went
//! in, or somebody merged the branch themselves, all three are a copy of
//! history the repository already has, and clearing them one `amx stop` at a
//! time is the chore that stops people from clearing them at all.
//!
//! What makes an agent a candidate is never amx's own opinion of the work. It
//! is the forge saying the request is over, git saying every commit on the
//! branch is in the main line, or the origin no longer having the branch at
//! all: three facts somebody else established, and none of them can be
//! established by a wall going quiet.
//!
//! The verb asks the forge itself where what the last look wrote down is
//! stale, and fetches once per repository before it reads the third one. Both
//! cost a moment, and both are what an operator who never opens the view is
//! owed: the view keeps what is written down current for itself, and nothing
//! keeps it current for anybody else. The view's own `c` is the opposite
//! reading — what is written down, and never a wait.
//!
//! Nothing is swept without being listed first, with the reason it is on the
//! list, and nothing at all is taken until that list has been agreed to. The
//! one law it will not break for an answer is the law `stop` keeps: a tree
//! holding work no commit has is never removed, and neither is the record that
//! names it.

use anyhow::Result;
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::cli::{Disposition, StopArgs};
use crate::derive::{self, View};
use crate::pr::{self, Pr};
use crate::store::{Meta, Phase};
use crate::verbs::stop;
use crate::{exit, paths, store, warn, worktree};

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
    fetch_origins(&views);
    let candidates: Vec<(View, String)> = views
        .into_iter()
        .filter_map(|view| why_landed_asking(&view).map(|why| (view, why)))
        .collect();

    if candidates.is_empty() {
        writeln!(out, "nothing to sweep")?;
        return Ok(exit::OK);
    }

    for (view, why) in &candidates {
        writeln!(out, "{}  {why}", view.id())?;
    }
    if !force && !agreed(candidates.len(), input, out)? {
        writeln!(out, "nothing swept")?;
        return Ok(exit::OK);
    }

    for (view, _) in &candidates {
        take_landed(root, &view.meta, out)?;
    }
    Ok(exit::OK)
}

/// Why this agent is on the list, or nothing at all, without waiting on
/// anything.
///
/// What the view presses `c` for. The forge is asked only through what the last
/// look wrote down, and the upstream only as git last recorded it: a press must
/// not stand still while a network answers, and the view is the reader that
/// keeps both of those worth reading.
pub fn why_landed(view: &View) -> Option<String> {
    landed(view, pr::written)
}

/// The same, for the verb, which asks the forge where what is written down is
/// stale.
///
/// A sweep at a shell has nothing written down to fall back on: the operator
/// who never opens the view has no pr.json beside any record, and every
/// agent whose work went in through a request would sit on the wall forever.
/// It can afford the question, too — it prints a list and waits for an answer
/// anyway. The upstream is read after [`prune_origin`](worktree::prune_origin)
/// has been run over the repository, which is [`run`]'s first act.
pub fn why_landed_asking(view: &View) -> Option<String> {
    landed(view, pr::asked_now)
}

/// The three facts, in the order they cost something to establish, with the
/// requests read however the caller reads them.
///
/// A turn that is over or an agent sitting idle, on a branch of amx's cutting,
/// whose work has landed.
fn landed(view: &View, requests: fn(&Meta) -> Vec<Pr>) -> Option<String> {
    if !finished(view.phase()) {
        return None;
    }
    let branch = view.meta.branch.as_deref()?;
    settled(&requests(&view.meta))
        .or_else(|| in_the_main_line(&view.meta, branch))
        .or_else(|| gone_from_origin(&view.meta, branch))
}

/// Bring every repository the walk is about to ask about up to date, once
/// apiece.
///
/// Without it `gone from origin` would be a week behind, since git records a
/// deleted upstream only when somebody fetches. Once per repository rather
/// than once per agent, because a wall of agents is usually a wall of trees
/// cut in two or three checkouts, and only the agents a candidate could come
/// from are worth the fetch at all.
///
/// A fetch that fails costs the third fact and nothing else — a network that
/// is not there, a forge asking for a password nobody is here to type — so it
/// is said once and the walk goes on. A repository with no origin is not that:
/// [`prune_origin`](worktree::prune_origin) runs nothing there and says nothing.
fn fetch_origins(views: &[View]) {
    let mut fetched = BTreeSet::new();
    for view in views {
        if !finished(view.phase()) || view.meta.branch.is_none() {
            continue;
        }
        let repo = repository(&view.meta);
        if !fetched.insert(repo.clone()) {
            continue;
        }
        if let Err(e) = worktree::prune_origin(&repo) {
            warn!(
                "amx sweep: could not fetch origin in {}: {e:#}",
                repo.display()
            );
        }
    }
}

/// Whether the agent is done being an agent: its turn ended, or it is sitting
/// there with nothing to do. A parked agent reads idle and counts.
fn finished(phase: Phase) -> bool {
    phase.is_terminal() || phase == Phase::Idle
}

/// The first request on the branch that is over, said the way a row says it.
fn settled(prs: &[Pr]) -> Option<String> {
    prs.iter()
        .find(|pr| pr.standing.settled())
        .map(|pr| format!("{} {}", pr.label(), pr.standing.says()))
}

/// The other way work lands: somebody merged the branch themselves, and there
/// is no request to have an opinion about it.
fn in_the_main_line(meta: &Meta, branch: &str) -> Option<String> {
    let repo = repository(meta);
    worktree::is_merged(&repo, branch)
        .ok()?
        .then(|| format!("{branch} merged into {}", worktree::main_branch(&repo)))
}

/// The third way work lands, and the only one that sees a squash merge: the
/// forge took the commits under a sha this branch does not hold, so nothing is
/// merged into anything as far as git can tell, and then it deleted the branch.
///
/// Read off what git recorded rather than off the origin, so this costs no
/// network wherever it is asked from. Making that record current is the fetch
/// [`run`] does once per repository.
fn gone_from_origin(meta: &Meta, branch: &str) -> Option<String> {
    worktree::upstream_gone(&repository(meta), branch)
        .ok()?
        .then(|| format!("{branch} gone from origin"))
}

/// Where git is asked about this agent's branch.
///
/// The repository rather than the tree, since the tree is what may be about to
/// go — and a tree already removed leaves git nothing to answer from inside it.
/// An agent amx cut no tree for works in the directory it was started in, and
/// that is the repository.
fn repository(meta: &Meta) -> PathBuf {
    let Some(tree) = &meta.worktree else {
        return meta.dir.clone();
    };
    worktree::main_repo(tree)
        .ok()
        .or_else(|| worktree::repo_of(tree))
        .unwrap_or_else(|| meta.dir.clone())
}

/// The one question, asked once for the whole list.
///
/// It reads the way every other question amx asks reads: the default is the
/// one that loses nothing, and anything that is not plainly yes — a shrug, a
/// typo, nobody there at all — takes it.
fn agreed(count: usize, input: &mut impl BufRead, out: &mut impl Write) -> Result<bool> {
    write!(out, "sweep {count}? [y/N] ")?;
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

/// Take one candidate: the tree, the branch and the record, and the pane first
/// where the agent is somehow still in one.
///
/// Which is `stop --force --delete --worktree delete --branch delete` and
/// nothing else. The whole ladder — the grace period, the tree git is asked to
/// remove, the branch that cannot go while a tree holds it, every line said as
/// it happens — is written down once, in the verb whose job it is.
pub fn take_landed(root: &Path, meta: &Meta, out: &mut impl Write) -> Result<()> {
    // The one thing `stop --force` would not save us from is the one thing
    // that cannot be undone, so it is answered before the ladder starts: a
    // dirty tree keeps its record too, because the record is where the tree
    // and the branch are named.
    if let Some(tree) = &meta.worktree
        && tree.exists()
        && worktree::is_dirty(tree).unwrap_or(true)
    {
        writeln!(
            out,
            "kept {}: {} holds work no commit has",
            meta.id,
            tree.display()
        )?;
        return Ok(());
    }

    stop::run(
        root,
        &StopArgs {
            id: meta.id.clone(),
            force: true,
            delete: true,
            worktree: Some(Disposition::Delete),
            branch: Some(Disposition::Delete),
        },
        &mut std::io::empty(),
        out,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr::Standing;
    use crate::store::{Agent, State};
    use crate::tmux::{PaneId, Socket};
    use std::process::Command;
    use tempfile::TempDir;

    /// git as the tests run it: none of the developer's own configuration and
    /// an identity of its own.
    fn git(dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
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

    /// An origin for `repo` to push to and be pruned against, bare and in a
    /// directory of its own.
    fn an_origin(repo: &Path) -> TempDir {
        let bare = TempDir::new().unwrap();
        git(bare.path(), &["init", "--bare", "-b", "main"]);
        git(
            repo,
            &["remote", "add", "origin", &bare.path().to_string_lossy()],
        );
        git(repo, &["push", "-q", "origin", "main"]);
        bare
    }

    /// An agent that has ended, with a tree of its own cut in `repo` and its
    /// branch already in the main line.
    ///
    /// The tree is cut and left where it is, so the branch holds exactly what
    /// main holds and git reads it as merged without anybody having to merge
    /// anything.
    fn a_swept_agent(root: &Path, repo: &Path, id: &str) -> Meta {
        let tree = worktree::create(repo, id, None).unwrap();
        let meta = Meta {
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            dir: repo.to_path_buf(),
            worktree: Some(tree.path.clone()),
            branch: Some(tree.branch.clone()),
            base: Some(tree.base.clone()),
            // A socket no tmux server on this machine answers on: nothing here
            // has a pane, and nothing here may signal one that is somebody's.
            socket: Socket::Name("amx-sweep-tests".to_string()),
            pane: PaneId::new("%1").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        };
        let agent = Agent::create(root, &meta).unwrap();
        ended(&agent);
        meta
    }

    /// Straight onto the disk: what the sweep reads is the phase, and these
    /// agents are meant to have finished.
    fn ended(agent: &Agent) {
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
    }

    /// The verb, with what it printed.
    fn swept(root: &Path, force: bool, typed: &str) -> String {
        let mut out = Vec::new();
        let code = run(root, force, &mut typed.as_bytes(), &mut out).unwrap();
        assert_eq!(code, exit::OK, "a sweep is never a failure");
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn sweep_is_only_ever_about_an_agent_that_is_finished_with_its_branch() {
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped, Phase::Idle] {
            assert!(finished(phase), "{phase:?}");
        }
        for phase in [Phase::Starting, Phase::Working, Phase::Waiting] {
            assert!(
                !finished(phase),
                "{phase:?}: an agent still at work is not swept out from under itself"
            );
        }
        assert!(
            !finished(Phase::Unknown),
            "and a reader that cannot tell is not a reason to take anything"
        );
    }

    #[test]
    fn sweep_reads_a_request_that_is_over_as_the_reason() {
        let request = |number, standing| Pr { number, standing };
        assert_eq!(
            settled(&[request(12, Standing::Merged)]).as_deref(),
            Some("#12 merged")
        );
        assert_eq!(
            settled(&[request(12, Standing::Closed)]).as_deref(),
            Some("#12 closed")
        );
        assert_eq!(
            settled(&[request(12, Standing::Ready), request(9, Standing::Merged)]).as_deref(),
            Some("#9 merged",),
            "a branch read twice is swept for the attempt that landed"
        );
        assert_eq!(
            settled(&[request(12, Standing::Failing)]),
            None,
            "a request still going is work somebody is still doing"
        );
        assert_eq!(settled(&[]), None, "and so is a branch with no request");
    }

    #[test]
    fn sweep_reads_a_branch_the_origin_no_longer_has_as_work_that_landed() {
        // What a squash merge leaves behind: the forge took the work under a
        // commit this branch does not hold, so `--merged` says no, and then it
        // deleted the branch. Read without a fetch of its own, so the view can
        // ask it too.
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let origin = an_origin(repo.path());
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        let tree = meta.worktree.clone().unwrap();
        std::fs::write(tree.join("login.rs"), "fn login() {}\n").unwrap();
        git(&tree, &["add", "login.rs"]);
        git(&tree, &["commit", "-m", "the agent's own commit"]);
        git(&tree, &["push", "-q", "-u", "origin", "amx/fix-login-a1b"]);

        let why = || {
            let views = derive::views(root.path(), store::now()).unwrap();
            let view = views.into_iter().find(|view| view.id() == meta.id);
            why_landed(&view.expect("the agent's own view"))
        };
        assert_eq!(
            why(),
            None,
            "the origin holds the branch and main does not hold the commit"
        );

        git(origin.path(), &["branch", "-D", "amx/fix-login-a1b"]);
        assert_eq!(
            why(),
            None,
            "and the delete is not a fact until git fetches"
        );

        worktree::prune_origin(repo.path()).unwrap();
        assert_eq!(
            why().as_deref(),
            Some("amx/fix-login-a1b gone from origin"),
            "which is the sweep's own doing, once per repository"
        );
    }

    #[test]
    fn sweep_says_what_it_would_take_and_takes_none_of_it_unasked() {
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");

        let said = swept(root.path(), false, "n\n");
        assert!(
            said.contains("fix-login-a1b  amx/fix-login-a1b merged into main"),
            "the agent and why it is on the list: {said}"
        );
        assert!(said.contains("sweep 1? [y/N]"), "{said}");
        assert!(said.contains("nothing swept"), "{said}");

        assert!(
            Agent::open(root.path(), &meta.id).is_ok(),
            "and the record is where it was"
        );
        assert!(meta.worktree.as_deref().unwrap().exists(), "and the tree");
        assert_eq!(
            git(
                repo.path(),
                &[
                    "branch",
                    "--list",
                    "amx/fix-login-a1b",
                    "--format=%(refname:short)"
                ]
            ),
            "amx/fix-login-a1b",
            "and the branch"
        );
    }

    #[test]
    fn sweep_takes_the_tree_the_branch_and_the_record_of_an_agent_agreed_to() {
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        let tree = meta.worktree.clone().unwrap();

        let said = swept(root.path(), false, "y\n");
        assert!(said.contains("fix-login-a1b stopped"), "{said}");
        assert!(
            said.contains(&format!("removed {}", tree.display())),
            "{said}"
        );
        assert!(said.contains("deleted amx/fix-login-a1b"), "{said}");
        assert!(said.contains("removed fix-login-a1b's record"), "{said}");

        assert!(!tree.exists(), "the tree is gone: {said}");
        assert_eq!(
            git(
                repo.path(),
                &[
                    "branch",
                    "--list",
                    "amx/fix-login-a1b",
                    "--format=%(refname:short)"
                ]
            ),
            "",
            "and the branch"
        );
        assert!(
            Agent::open(root.path(), &meta.id).is_err(),
            "and the record with them"
        );
    }

    #[test]
    fn sweep_hands_out_the_same_reader_and_taker_the_verb_runs_on() {
        // What the view presses `c` for is these two and nothing beside them:
        // one asks of an agent why it is on the list, the other takes that one
        // agent, with no list and no question in between.
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        let tree = meta.worktree.clone().unwrap();

        let views = derive::views(root.path(), store::now()).unwrap();
        let view = views.iter().find(|view| view.id() == meta.id).unwrap();
        assert_eq!(
            why_landed(view).as_deref(),
            Some("amx/fix-login-a1b merged into main")
        );

        let mut out = Vec::new();
        take_landed(root.path(), &view.meta, &mut out).unwrap();
        assert!(!tree.exists(), "the tree is gone");
        assert!(
            Agent::open(root.path(), &meta.id).is_err(),
            "and the record with it"
        );
    }

    #[test]
    fn sweep_keeps_a_tree_that_holds_work_no_commit_has() {
        // The one law an answer does not move, the same law `stop` keeps: the
        // record goes with the tree, because the record is where the tree and
        // the branch are written down.
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        let tree = meta.worktree.clone().unwrap();
        std::fs::write(tree.join("login.rs"), "fn login() {}\n").unwrap();

        let said = swept(root.path(), true, "");
        assert!(
            said.contains(&format!(
                "kept fix-login-a1b: {} holds work no commit has",
                tree.display()
            )),
            "{said}"
        );
        assert!(tree.exists(), "the tree stands: {said}");
        assert!(
            Agent::open(root.path(), &meta.id).is_ok(),
            "and the record that names it"
        );
    }

    #[test]
    fn sweep_asks_nothing_of_a_wall_with_nothing_on_it_to_sweep() {
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        // Work nothing has taken yet, which is the ordinary state of an agent
        // that has just finished.
        let tree = meta.worktree.as_deref().unwrap();
        std::fs::write(tree.join("login.rs"), "fn login() {}\n").unwrap();
        git(tree, &["add", "login.rs"]);
        git(tree, &["commit", "-m", "the agent's own commit"]);

        let said = swept(root.path(), false, "y\n");
        assert_eq!(said, "nothing to sweep\n");
    }

    #[test]
    fn sweep_takes_a_shrug_for_a_no() {
        for typed in ["\n", "maybe\n", "no\n", ""] {
            let mut out = Vec::new();
            assert!(
                !agreed(2, &mut typed.as_bytes(), &mut out).unwrap(),
                "{typed:?}"
            );
        }
        for typed in ["y\n", "yes\n", "Y\n"] {
            let mut out = Vec::new();
            assert!(
                agreed(2, &mut typed.as_bytes(), &mut out).unwrap(),
                "{typed:?}"
            );
        }
    }
}
