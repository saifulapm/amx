//! `amx sweep`: remove the record, tree and branch of agents whose work landed.
//!
//! An ended or idle agent on a branch is a candidate when one of three outside
//! facts holds: its pull request is merged or closed, git reads the branch as
//! merged into the main line, or the origin no longer has the branch (which is
//! how a squash merge shows). The verb asks the forge where the written-down
//! state is stale and fetches each repository once; the view's `c` reads only
//! what is written down and never waits on the network. Candidates are listed
//! with their reason and nothing is taken until the list is agreed to.
//!
//! - A worktree holding uncommitted work is never removed, and neither is the
//!   record that names it.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::cli::{Disposition, StopArgs};
use crate::derive::{self, View};
use crate::pr::{self, Pr};
use crate::store::{Meta, Phase};
use crate::verbs::{clear, stop};
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

/// Why this agent's work landed, if it did, without touching the network.
///
/// What the view's `c` asks. Requests come from what the last look wrote down
/// and the upstream from what git last fetched; the view keeps both current.
pub fn why_landed(view: &View) -> Option<String> {
    landed(view, pr::written)
}

/// [`why_landed`] for the verb, asking the forge where the written-down state
/// is stale.
///
/// Without the view running, no `pr.json` is ever written, so the verb has to
/// ask. The upstream is current because [`run`] fetches every repository first.
pub fn why_landed_asking(view: &View) -> Option<String> {
    landed(view, pr::asked_now)
}

/// The reason an ended or idle agent's branch landed, trying the three facts
/// cheapest first, with requests read through `requests`.
fn landed(view: &View, requests: fn(&Meta) -> Vec<Pr>) -> Option<String> {
    if !finished(view.phase()) {
        return None;
    }
    let branch = view.meta.branch.as_deref()?;
    settled(&requests(&view.meta))
        .or_else(|| in_the_main_line(&view.meta, branch))
        .or_else(|| gone_from_origin(&view.meta, branch))
}

/// Fetch and prune origin once in every repository a candidate could come
/// from, since git sees a deleted upstream branch only after a fetch.
///
/// A failed fetch (no network, a credential prompt) only loses the "gone from
/// origin" check, so it is a warning. A repository with no origin is skipped
/// silently by [`prune_origin`](worktree::prune_origin).
fn fetch_origins(views: &[View]) {
    for repo in repositories(views) {
        if let Err(e) = worktree::prune_origin(&repo) {
            warn!(
                "amx sweep: could not fetch origin in {}: {e:#}",
                repo.display()
            );
        }
    }
}

/// The repositories of the views that could be candidates, without duplicates.
fn repositories(views: &[View]) -> Vec<PathBuf> {
    let mut fetched = BTreeSet::new();
    views
        .iter()
        .filter(|view| finished(view.phase()) && view.meta.branch.is_some())
        .map(|view| repository(&view.meta))
        .filter(|repo| fetched.insert(repo.clone()))
        .collect()
}

/// The minimum time between two background fetches of one repository.
const PRUNE_EVERY: Duration = Duration::from_secs(300);

/// When each repository was last fetched for the view.
static PRUNED: Mutex<BTreeMap<PathBuf, Instant>> = Mutex::new(BTreeMap::new());

/// The view's version of [`fetch_origins`]: fetch each candidate repository
/// on a background thread, at most once every [`PRUNE_EVERY`], so `c` has a
/// current upstream to read.
///
/// Nothing waits on the threads and failures are ignored; the view has no
/// stderr to report them on.
pub fn fetch_origins_again(views: &[View]) {
    for repo in prune_due(views, Instant::now()) {
        let _ = std::thread::Builder::new()
            .name("amx-prune".to_string())
            .spawn(move || {
                let _ = worktree::prune_origin(&repo);
            });
    }
}

/// The repositories due a fetch at `now`, marked as fetched on the way out.
///
/// Marked before the fetch runs, so the next reading does not start a second
/// one. A failed fetch leaves the repository alone until the next interval.
fn prune_due(views: &[View], now: Instant) -> Vec<PathBuf> {
    let Ok(mut pruned) = PRUNED.lock() else {
        return Vec::new();
    };
    repositories(views)
        .into_iter()
        .filter(|repo| match pruned.get(repo) {
            Some(last) if now.saturating_duration_since(*last) < PRUNE_EVERY => false,
            _ => {
                pruned.insert(repo.clone(), now);
                true
            }
        })
        .collect()
}

/// Ended or idle. A parked agent reads idle and counts.
fn finished(phase: Phase) -> bool {
    phase.is_terminal() || phase == Phase::Idle
}

/// The first merged or closed request, worded the way a row shows it.
fn settled(prs: &[Pr]) -> Option<String> {
    prs.iter()
        .find(|pr| pr.standing.settled())
        .map(|pr| format!("{} {}", pr.label(), pr.standing.says()))
}

/// The branch was merged into the main line without a request.
fn in_the_main_line(meta: &Meta, branch: &str) -> Option<String> {
    let repo = repository(meta);
    worktree::is_merged(&repo, branch)
        .ok()?
        .then(|| format!("{branch} merged into {}", worktree::main_branch(&repo)))
}

/// The origin deleted the branch. The only check that catches a squash merge,
/// whose commits git cannot see as merged.
///
/// Reads git's recorded upstream, so it costs no network; [`run`] fetches
/// first to make that current.
fn gone_from_origin(meta: &Meta, branch: &str) -> Option<String> {
    worktree::upstream_gone(&repository(meta), branch)
        .ok()?
        .then(|| format!("{branch} gone from origin"))
}

/// The repository to ask git about this agent's branch.
///
/// The repository, not the tree, since the tree may be about to go or already
/// gone. An agent without a tree runs in its repository.
fn repository(meta: &Meta) -> PathBuf {
    let Some(tree) = &meta.worktree else {
        return meta.dir.clone();
    };
    // The tree's `.git` file first: the view asks this of every finished agent
    // on every reading, and `main_repo` runs git.
    worktree::repo_of_linked(tree)
        .or_else(|| worktree::main_repo(tree).ok())
        .or_else(|| worktree::repo_of(tree))
        .unwrap_or_else(|| meta.dir.clone())
}

/// Ask once for the whole list. Anything but yes, including no input, is no.
fn agreed(count: usize, input: &mut impl BufRead, out: &mut impl Write) -> Result<bool> {
    stop::confirm(&format!("sweep {count}?"), false, input, out)
}

/// Take one candidate: its pane if it still has one, tree, branch and record.
///
/// This is `stop --force --delete --worktree delete --branch delete`, so the
/// ending, the removals and their output all come from `stop`.
pub fn take_landed(root: &Path, meta: &Meta, out: &mut impl Write) -> Result<()> {
    // Checked before `stop`: a tree holding uncommitted work keeps the whole
    // agent, record included, since the record names the tree and branch.
    if let Some(tree) = clear::holding(meta) {
        writeln!(
            out,
            "kept {}: {} holds work no commit has",
            meta.id,
            tree.display()
        )?;
        return Ok(());
    }

    // Only a branch amx named is deleted. A branch the person named (e.g.
    // `--branch develop`) reads as landed whenever main catches up with it.
    let branch = match &meta.branch {
        Some(branch) if !worktree::named_by_amx(&meta.id, branch) => {
            writeln!(out, "kept {branch}: not amx's to delete")?;
            Disposition::Keep
        }
        _ => Disposition::Delete,
    };
    stop::run(
        root,
        &StopArgs {
            id: meta.id.clone(),
            force: true,
            delete: true,
            worktree: Some(Disposition::Delete),
            branch: Some(branch),
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

    /// Run git with no user or system config and a fixed identity.
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

    /// A bare origin for `repo`, with main pushed to it.
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

    /// A done agent with a fresh tree cut in `repo`. Its branch has no commits
    /// of its own, so git reads it as merged into main.
    fn a_swept_agent(root: &Path, repo: &Path, id: &str) -> Meta {
        let tree = worktree::create(repo, id, None).unwrap();
        let meta = Meta {
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
            branch: Some(tree.branch.clone()),
            base: Some(tree.base.clone()),
            // No server listens here, so no real pane can be signalled.
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

    /// Write a done state straight to disk.
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

    /// Run the verb and return what it printed.
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
        // A squash merge: main does not hold the branch's commit, so
        // `--merged` says no, then the origin deletes the branch. The check
        // itself never fetches, so the view can ask it too.
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
    fn sweep_fetches_a_repository_for_the_view_once_in_five_minutes() {
        // The clock is an argument, so the interval is not waited out.
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        a_swept_agent(root.path(), repo.path(), "tidy-b2c");
        let views = derive::views(root.path(), store::now()).unwrap();

        let walk = repositories(&views);
        assert_eq!(walk.len(), 1, "two agents in one repository are one fetch");

        let at = Instant::now();
        assert_eq!(prune_due(&views, at), walk, "the first reading fetches");
        assert!(
            prune_due(&views, at).is_empty(),
            "and a reading a moment later does not, or a wall of agents would \
             put a fetch behind every reading"
        );
        assert!(
            prune_due(&views, at + PRUNE_EVERY - Duration::from_secs(1)).is_empty(),
            "nor does one just inside the five minutes"
        );
        assert_eq!(
            prune_due(&views, at + PRUNE_EVERY),
            walk,
            "and the one after them fetches again"
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
        // The view's `c` uses these two directly, with no list and no prompt.
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
        // Even with --force, the tree and the record that names it stay.
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
        // A commit main does not have yet: the usual state of a just-finished
        // agent.
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

    #[test]
    fn sweep_names_the_repository_a_tree_was_cut_in_as_git_does() {
        let root = TempDir::new().unwrap();
        let repo = a_repo();
        let meta = a_swept_agent(root.path(), repo.path(), "fix-login-a1b");
        let tree = meta.worktree.clone().unwrap();
        assert_eq!(repository(&meta), worktree::main_repo(&tree).unwrap());

        // Once the tree is gone, the repository comes from its path.
        let named = repository(&meta);
        worktree::remove(&named, &tree).unwrap();
        assert_eq!(repository(&meta), worktree::repo_of(&tree).unwrap());
    }
}
