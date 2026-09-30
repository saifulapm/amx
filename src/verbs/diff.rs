//! `amx diff`: an agent's work so far, as a patch or with `--stat` a summary.
//!
//! The work is measured from the base on the record: the commit the tree was
//! cut from, or the commit the directory was on when the session started. That
//! includes everything the agent has committed. A record with no base (an
//! adopted agent, or an older record) is measured from where its branch left
//! the main line. A directory outside a repository and a removed tree are
//! reported as errors naming the reason.
//!
//! At a terminal the patch goes through the `diff` viewer from the config, if
//! one is set. Down a pipe, and with `--stat`, git's output is printed as is.

use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::store::{Agent, Meta};
use crate::{complain, exit, paths, worktree};

/// Run the verb against the machine, through the configured viewer when
/// stdout is a terminal and `--stat` is not set.
pub fn from_env(id: &str, stat: bool, from: Option<&str>) -> Result<i32> {
    let root = paths::state_root()?;
    let reading = !stat && std::io::stdout().is_terminal();
    match reading.then(|| viewer(&root, id)).flatten() {
        Some(viewer) => in_viewer_with(&root, id, &viewer, from),
        None => run_with(&root, id, stat, from, &mut std::io::stdout().lock()),
    }
}

/// The `diff` viewer configured for this agent's project, if any.
///
/// An unknown id returns `None` here and is refused later by the verb.
fn viewer(root: &Path, id: &str) -> Option<String> {
    let meta = Agent::open(root, id).ok()?.meta().ok()?;
    crate::config::for_project(&meta.dir).diff.clone()
}

/// The verb, with the state directory named, measured from the record's base.
pub fn run(root: &Path, id: &str, stat: bool, out: &mut impl Write) -> Result<i32> {
    run_with(root, id, stat, None, out)
}

/// [`run`], measured from `from` when given.
fn run_with(
    root: &Path,
    id: &str,
    stat: bool,
    from: Option<&str>,
    out: &mut impl Write,
) -> Result<i32> {
    let meta = Agent::open(root, id)?.meta()?;
    let (tree, base) = work_of(&meta, id, from)?;

    worktree::diff(tree, &base, stat, out)?;
    Ok(exit::OK)
}

/// Pipe the patch into `viewer` on this terminal.
///
/// The viewer is a shell command line, run with `sh -c` in the agent's tree so
/// it can read the repository's files and config.
pub fn in_viewer(root: &Path, id: &str, viewer: &str) -> Result<i32> {
    in_viewer_with(root, id, viewer, None)
}

/// [`in_viewer`], measured from `from` when given.
fn in_viewer_with(root: &Path, id: &str, viewer: &str, from: Option<&str>) -> Result<i32> {
    let meta = Agent::open(root, id)?.meta()?;
    let (tree, base) = work_of(&meta, id, from)?;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(viewer)
        .current_dir(tree)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("running `{viewer}`"))?;

    let mut stdin = child.stdin.take().expect("stdin was asked for");
    let handed = worktree::diff(tree, &base, false, &mut stdin);
    // Close stdin before waiting, or a viewer reading to EOF never finishes.
    drop(stdin);
    let ended = child.wait().context("waiting for the viewer")?;

    match ended.code() {
        Some(exit::OK) => {
            // A write error only matters if the viewer succeeded: one that
            // quit early closed the pipe on purpose.
            handed?;
            Ok(exit::OK)
        }
        // The viewer has reported its own error on the terminal.
        Some(code) => {
            complain!("amx diff: {viewer} exited {code}");
            Ok(exit::FAILURE)
        }
        // Killed by a signal.
        None => Ok(exit::FAILURE),
    }
}

/// The tree the agent works in and the commit to measure from.
///
/// The base is `from` if given, else the record's base, else the fork point of
/// the tree's branch from the main line.
fn work_of<'a>(meta: &'a Meta, id: &str, from: Option<&str>) -> Result<(&'a Path, String)> {
    let tree = meta.worktree.as_deref().unwrap_or(&meta.dir);

    if !tree.exists() {
        match &meta.branch {
            Some(branch) => bail!("{} is gone; what `{id}` did is on {branch}", tree.display()),
            None => bail!("{} is gone", tree.display()),
        }
    }

    let base = match from {
        Some(from) => from.to_string(),
        None => match &meta.base {
            Some(base) => base.clone(),
            None => match worktree::fork_point(tree)? {
                Some(base) => base,
                None => bail!(
                    "`{id}` works in {}, which is no git worktree, \
                     so there is nothing to compare it against",
                    tree.display()
                ),
            },
        },
    };

    Ok((tree, base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, now};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A record of an agent with its worktree at `worktree`.
    fn record(root: &Path, id: &str, worktree: Option<&Path>) -> Agent {
        Agent::create(
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
                dir: worktree
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/srv/app")),
                worktree: worktree.map(Path::to_path_buf),
                branch: worktree.map(|_| worktree::branch_for(id)),
                base: worktree.map(|_| "0f1e2d3".to_string()),
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: now(),
            },
        )
        .expect("the record")
    }

    /// A record of an agent running in `dir` with no worktree, branch or base,
    /// as an adopted agent has.
    fn record_in(root: &Path, id: &str, dir: &Path) -> Agent {
        Agent::create(
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
                dir: dir.to_path_buf(),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: now(),
            },
        )
        .expect("the record")
    }

    /// git with no user or system config and a fixed identity.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
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

    /// An agent whose tree has one commit and an uncommitted change on it.
    fn a_tree_with_work(root: &Path, id: &str) -> TempDir {
        let tree = TempDir::new().unwrap();
        git(tree.path(), &["init", "-b", "main"]);
        std::fs::write(tree.path().join("README.md"), "before\n").unwrap();
        git(tree.path(), &["add", "README.md"]);
        git(tree.path(), &["commit", "-m", "first"]);
        let base = git(tree.path(), &["rev-parse", "HEAD"]);
        std::fs::write(tree.path().join("README.md"), "after\n").unwrap();

        record(root, id, Some(tree.path()))
            .writer()
            .unwrap()
            .update_meta(|meta| meta.base = Some(base.clone()))
            .unwrap();
        tree
    }

    /// A record with no base, in a checkout whose branch has one commit past
    /// main, so the fork point is the base.
    fn a_record_with_no_base(root: &Path, id: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "before\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "first"]);
        git(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("README.md"), "after\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);

        record_in(root, id, dir.path());
        dir
    }

    fn refused(root: &Path, id: &str) -> String {
        let mut out = Vec::new();
        format!("{:#}", run(root, id, false, &mut out).unwrap_err())
    }

    #[test]
    fn diff_resolves_a_base_for_an_agent_whose_record_has_none() {
        let root = TempDir::new().unwrap();
        let _dir = a_record_with_no_base(root.path(), "adopted-b2c");

        let mut out = Vec::new();
        run(root.path(), "adopted-b2c", false, &mut out).expect("a patch");
        let patch = String::from_utf8(out).unwrap();
        assert!(
            patch.contains("-before") && patch.contains("+after"),
            "the branch's own work, measured from where it left main: {patch}"
        );
    }

    #[test]
    fn diff_has_nothing_to_compare_for_a_directory_outside_a_repository() {
        let root = TempDir::new().unwrap();
        let plain = TempDir::new().unwrap();
        record_in(root.path(), "no-repo-b2c", plain.path());

        let said = refused(root.path(), "no-repo-b2c");
        assert!(said.contains("no git worktree"), "{said}");
        assert!(
            said.contains(&plain.path().display().to_string()),
            "and it names the directory: {said}"
        );
    }

    #[test]
    fn diff_says_where_the_work_went_when_the_tree_is_gone() {
        let root = TempDir::new().unwrap();
        record(
            root.path(),
            "fix-login-a1b",
            Some(Path::new("/srv/app/.amx/worktrees/fix-login-a1b")),
        );

        let said = refused(root.path(), "fix-login-a1b");
        assert!(said.contains("gone"), "{said}");
        assert!(said.contains("amx/fix-login-a1b"), "{said}");
    }

    #[test]
    fn the_viewer_reads_git_s_patch_and_runs_where_the_work_is() {
        let root = TempDir::new().unwrap();
        let tree = a_tree_with_work(root.path(), "fix-login-a1b");
        let elsewhere = TempDir::new().unwrap();
        let handed = elsewhere.path().join("handed");

        let code = in_viewer(
            root.path(),
            "fix-login-a1b",
            &format!("{{ pwd; cat; }} > {}", handed.display()),
        )
        .expect("the viewer");
        assert_eq!(code, exit::OK);

        let said = std::fs::read_to_string(&handed).expect("what the viewer was handed");
        let (where_it_ran, patch) = said.split_once('\n').expect("a line and a patch");
        assert_eq!(
            std::fs::canonicalize(where_it_ran).unwrap(),
            std::fs::canonicalize(tree.path()).unwrap(),
            "the patch is read where the work is: {where_it_ran}"
        );
        assert!(
            patch.contains("-before") && patch.contains("+after"),
            "{patch}"
        );
    }

    #[test]
    fn a_viewer_that_failed_is_a_failure_of_its_own() {
        let root = TempDir::new().unwrap();
        let _tree = a_tree_with_work(root.path(), "fix-login-a1b");

        let code = in_viewer(root.path(), "fix-login-a1b", "exit 3").expect("the viewer");
        assert_eq!(code, exit::FAILURE);
    }

    #[test]
    fn nothing_is_run_for_an_agent_there_is_no_patch_of() {
        // The viewer gets the same refusals as the patch.
        let root = TempDir::new().unwrap();
        let plain = TempDir::new().unwrap();
        record_in(root.path(), "no-repo-b2c", plain.path());
        let said = format!(
            "{:#}",
            in_viewer(root.path(), "no-repo-b2c", "cat").unwrap_err()
        );
        assert!(said.contains("no git worktree"), "{said}");

        let said = format!(
            "{:#}",
            in_viewer(root.path(), "never-made-abc", "cat").unwrap_err()
        );
        assert!(said.contains("no agent"), "{said}");
    }

    #[test]
    fn diff_is_never_taken_through_something_that_is_not_an_id() {
        // An id shaped like a path must not reach a record outside the root.
        let root = TempDir::new().unwrap();
        assert!(refused(root.path(), "../elsewhere").contains("no agent"));
        assert!(refused(root.path(), "never-made-abc").contains("no agent"));
    }
}
