//! `amx diff` — what an agent has done to its tree, while it is still doing it,
//! or with `--stat` the shape of it: a file per line and the totals under them,
//! which is what somebody wants when the question is how far along it is.
//!
//! The work is measured from the commit the tree was cut from, recorded when it
//! was cut. Not from the repository's HEAD, which has moved on since, and not
//! from the agent's own HEAD, which would hide everything it has committed —
//! what a person wants to see is the whole of this agent's work. A tree whose
//! history has moved off that commit is measured from the last commit the two
//! still share, which is [`worktree::diff`]'s own business.
//!
//! An agent working directly in a directory has nothing to compare, and a tree
//! somebody has removed is not there to read. Both are ordinary answers to an
//! ordinary question, so both say what happened rather than failing at git.
//!
//! A patch is also something a person reads, and the `diff` key names what they
//! read it with. At a terminal that command gets git's patch and the screen;
//! down a pipe, and under `--stat`, the patch is git's own, because a caller
//! processing one is not somebody looking at one.

use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::store::{Agent, Meta};
use crate::{complain, exit, paths, worktree};

/// Run the verb against the machine.
///
/// A patch on a terminal is something to read, and a patch down a pipe is
/// something to process: the viewer the config names is for the first of them
/// alone, so `amx diff fix-login-a1b | head` is the same patch it always was.
/// `--stat` is not a patch at all, and a viewer handed one has nothing to
/// colour.
pub fn from_env(id: &str, stat: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let reading = !stat && std::io::stdout().is_terminal();
    match reading.then(|| viewer(&root, id)).flatten() {
        Some(viewer) => in_viewer(&root, id, &viewer),
        None => run(&root, id, stat, &mut std::io::stdout().lock()),
    }
}

/// What the config names to read this agent's patch with, if anything.
///
/// Read under the project the agent works in, so a repository whose patches
/// want a viewer of their own says so in its own file, over whatever the person
/// set. An id naming no record answers nothing here and is refused below in the
/// verb's own words.
fn viewer(root: &Path, id: &str) -> Option<String> {
    let meta = Agent::open(root, id).ok()?.meta().ok()?;
    crate::config::for_project(&meta.dir).diff.clone()
}

/// The verb, with the state directory named.
pub fn run(root: &Path, id: &str, stat: bool, out: &mut impl Write) -> Result<i32> {
    let meta = Agent::open(root, id)?.meta()?;
    let (tree, base) = work_of(&meta, id)?;

    worktree::diff(tree, base, stat, out)?;
    Ok(exit::OK)
}

/// The patch through the viewer the config names, on the terminal amx was
/// asked from.
///
/// `sh -c`, because what the key holds is a command line a person wrote — a
/// pager on the end of it, flags, a pipe — and not a program and its argv. It
/// runs in the tree, so a viewer that opens a file it was shown, or reads the
/// repository's own configuration, finds them where the work is.
///
/// The terminal is the viewer's own: whatever it draws, pages and asks is
/// between it and the person, and amx is done when it is.
pub fn in_viewer(root: &Path, id: &str, viewer: &str) -> Result<i32> {
    let meta = Agent::open(root, id)?.meta()?;
    let (tree, base) = work_of(&meta, id)?;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(viewer)
        .current_dir(tree)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("running `{viewer}`"))?;

    let mut stdin = child.stdin.take().expect("stdin was asked for");
    let handed = worktree::diff(tree, base, false, &mut stdin);
    // The write end goes before the wait, or a viewer reading to the end of the
    // patch would wait for an end that never comes.
    drop(stdin);
    let ended = child.wait().context("waiting for the viewer")?;

    match ended.code() {
        Some(exit::OK) => {
            // Only now: a viewer quit halfway through is a write that failed
            // on a pipe nobody is reading, which is the reader having what it
            // came for rather than anything to report.
            handed?;
            Ok(exit::OK)
        }
        // What went wrong the viewer has already said on the terminal it was
        // given, so what is left is which command it was.
        Some(code) => {
            complain!("amx diff: {viewer} exited {code}");
            Ok(exit::FAILURE)
        }
        // A signal took the viewer down, and a signal is not a code to report
        // as one.
        None => Ok(exit::FAILURE),
    }
}

/// The tree an agent's work is in and the commit it is measured from, or the
/// ordinary answer to why there is neither.
fn work_of<'a>(meta: &'a Meta, id: &str) -> Result<(&'a Path, &'a str)> {
    let (Some(tree), Some(base)) = (&meta.worktree, &meta.base) else {
        bail!(
            "`{id}` has no worktree of its own; it works in {}, \
             so there is nothing to compare it against",
            meta.dir.display()
        );
    };

    if !tree.exists() {
        match &meta.branch {
            Some(branch) => bail!("{} is gone; what `{id}` did is on {branch}", tree.display()),
            None => bail!("{} is gone", tree.display()),
        }
    }

    Ok((tree, base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Meta, now};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A record of an agent, with its tree wherever the test wants it.
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

    /// git as these tests run it: none of the developer's own configuration,
    /// and an identity of its own.
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

    /// An agent whose tree has a commit behind it and a change on top of it,
    /// which is everything `git diff` needs to have something to say.
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

    fn refused(root: &Path, id: &str) -> String {
        let mut out = Vec::new();
        format!("{:#}", run(root, id, false, &mut out).unwrap_err())
    }

    #[test]
    fn diff_has_nothing_to_compare_for_an_agent_without_a_tree() {
        let root = TempDir::new().unwrap();
        record(root.path(), "no-tree-b2c", None);

        let said = refused(root.path(), "no-tree-b2c");
        assert!(said.contains("no worktree"), "{said}");
        assert!(
            said.contains("/srv/app"),
            "and it names where the work happens instead: {said}"
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
        // The viewer has said whatever it had to say on the terminal it was
        // given, and what is left for amx is which command it was.
        let root = TempDir::new().unwrap();
        let _tree = a_tree_with_work(root.path(), "fix-login-a1b");

        let code = in_viewer(root.path(), "fix-login-a1b", "exit 3").expect("the viewer");
        assert_eq!(code, exit::FAILURE);
    }

    #[test]
    fn nothing_is_run_for_an_agent_there_is_no_patch_of() {
        // The same refusals the patch itself gets: a viewer started for a row
        // with nothing to compare would take the terminal to show nothing.
        let root = TempDir::new().unwrap();
        record(root.path(), "no-tree-b2c", None);
        let said = format!(
            "{:#}",
            in_viewer(root.path(), "no-tree-b2c", "cat").unwrap_err()
        );
        assert!(said.contains("no worktree"), "{said}");

        let said = format!(
            "{:#}",
            in_viewer(root.path(), "never-made-abc", "cat").unwrap_err()
        );
        assert!(said.contains("no agent"), "{said}");
    }

    #[test]
    fn diff_is_never_taken_through_something_that_is_not_an_id() {
        // `root.join(id)` is not a lookup: an id shaped like a path would name
        // a record anywhere on the machine, and then a tree to run git in.
        let root = TempDir::new().unwrap();
        assert!(refused(root.path(), "../elsewhere").contains("no agent"));
        assert!(refused(root.path(), "never-made-abc").contains("no agent"));
    }
}
