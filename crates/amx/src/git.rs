//! git, run as argv (D-M3-10).
//!
//! The server learns no git. `amx work` composes three capabilities that already
//! exist — `git worktree add`, `workspace.create`, `agent.start` — and this
//! module is the first of them: every git invocation amx makes, and the only
//! place in the tree that runs a command which can delete a directory somebody
//! is working in.
//!
//! # Argv, never a shell string
//!
//! A branch name is user data. It travels here as one element of an argument
//! vector and is handed to `execve` as one element, so there is no stage at
//! which a shell could see it — the same rule [`crate::update::fetch`] applies
//! to a URL and the agent registry applies to a stanza's `start`. Nothing in
//! this file builds a string and hands it to `sh`, and nothing in it may.
//!
//! Two further precautions, because argv-as-data is not by itself enough when
//! the program being run has its own flag parser:
//!
//! - A branch name is refused before it is passed to anything if it begins with
//!   `-`, because git would read it as an option rather than as a ref.
//! - Every other branch name is checked by `git check-ref-format --branch`,
//!   which is git's own answer to "is this a branch name" — verified to refuse
//!   `a..b` and `../etc` on git 2.55.0. amx does not re-implement that grammar;
//!   re-implementing it is how a rule ends up stricter in one place than in the
//!   other.
//!
//! # What each subcommand does, read from `git worktree`(1)
//!
//! - `worktree add <path> <branch>` checks an **existing** branch out at `path`,
//!   and refuses if that branch is already checked out in another worktree.
//! - `worktree add -b <branch> <path>` creates the branch from `HEAD` first.
//!   The manual's convenience rules — infer the branch from the path's basename,
//!   track a same-named remote branch — are deliberately not relied on: amx asks
//!   whether the ref exists ([`Repo::has_branch`]) and then says which of the two
//!   commands it means, so the outcome does not depend on how many remotes the
//!   repository has.
//! - `worktree remove <path>` refuses a tree with modified or untracked files
//!   unless `--force`. amx checks for itself first ([`Repo::is_dirty`]) so the
//!   refusal arrives *before* the workspace is killed; git's own refusal stays as
//!   the backstop it should be.
//! - `worktree list --porcelain` names every registered worktree of the
//!   repository, main first, from anywhere inside it. It is how
//!   [`Repo::discover`] finds the main checkout even when the caller's directory
//!   is itself a linked worktree, and how `amx work done` confirms the path it is
//!   about to remove is one this repository actually owns.
//! - `worktree prune` drops the administrative files of a worktree whose
//!   directory has been removed by hand — the tidy-up for the case restore
//!   reports.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context as _, anyhow, bail};
use tokio::process::Command;

/// One repository, addressed by its main working tree.
///
/// Every command runs with `-C <root>` rather than relying on the process's
/// directory: `amx work done` is driven by a workspace's recorded membership,
/// which names a repository that may have nothing to do with where the user
/// happens to be standing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// The repository containing `cwd`, addressed by its main working tree.
    ///
    /// The main tree and not the current one: the default `work.dir` template is
    /// a *sibling* of the repository, and `amx work` run from inside one
    /// worktree must put the next one beside the repository rather than beside
    /// its sibling. `git worktree list --porcelain` lists the main worktree
    /// first from anywhere inside the repository, which is exactly that answer.
    ///
    /// # Errors
    ///
    /// If `cwd` is not inside a git repository, if `git` cannot be run, or if
    /// the list comes back without a worktree in it.
    pub async fn discover(cwd: &Path) -> anyhow::Result<Self> {
        let listed = run(cwd, &["worktree", "list", "--porcelain"])
            .await
            .with_context(|| format!("{} is not inside a git repository", cwd.display()))?;
        let root = worktree_paths(&listed)
            .into_iter()
            .next()
            .with_context(|| format!("git listed no worktree for {}", cwd.display()))?;
        Ok(Self { root })
    }

    /// A repository at a known main working tree, from a recorded membership.
    ///
    /// No discovery and no `git` call: the path came out of a workspace's
    /// worktree block, which the server refused to record unless it was
    /// absolute. What it is *not* is a check that the directory is still a
    /// repository — the first command to need one says so itself, which keeps
    /// the failure attached to the operation it stops.
    #[must_use]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The repository's main working tree.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `refs/heads/<branch>` exists in this repository.
    ///
    /// Asked so [`Repo::add_worktree`] can say which of the two `worktree add`
    /// shapes it means instead of leaning on the manual's inference rules.
    ///
    /// # Errors
    ///
    /// If `git` cannot be run at all. A ref that is absent is an answer, not an
    /// error: `rev-parse --verify --quiet` exits non-zero and prints nothing.
    pub async fn has_branch(&self, branch: &str) -> anyhow::Result<bool> {
        check_branch_shape(branch)?;
        let out = command(&self.root)
            .args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .stdin(Stdio::null())
            .output()
            .await
            .context("run git rev-parse")?;
        Ok(out.status.success())
    }

    /// Add a worktree at `path` for `branch`, creating the branch when `create`.
    ///
    /// # Errors
    ///
    /// If the branch name is not one git accepts, if `git` cannot be run, or if
    /// git refuses — a path that is in the way, a branch already checked out in
    /// another worktree. git's own sentence is what the caller gets: amx has no
    /// better account of why a checkout failed than the tool that tried it.
    pub async fn add_worktree(
        &self,
        path: &Path,
        branch: &str,
        create: bool,
    ) -> anyhow::Result<()> {
        check_branch_shape(branch)?;
        let mut command = command(&self.root);
        command.args(["worktree", "add"]);
        if create {
            command.arg("-b").arg(branch).arg(path);
        } else {
            command.arg(path).arg(branch);
        }
        finish(command, "git worktree add").await
    }

    /// Remove the worktree at `path`, forcing past a dirty tree when `force`.
    ///
    /// # Errors
    ///
    /// If `git` cannot be run, or if git refuses — a path that is not a working
    /// tree of this repository, a locked worktree, or (without `force`) one with
    /// modified or untracked files.
    pub async fn remove_worktree(&self, path: &Path, force: bool) -> anyhow::Result<()> {
        let mut command = command(&self.root);
        command.args(["worktree", "remove"]);
        if force {
            command.arg("--force");
        }
        command.arg(path);
        finish(command, "git worktree remove").await
    }

    /// Drop the administrative files of worktrees whose directories are gone.
    ///
    /// # Errors
    ///
    /// If `git` cannot be run, or if it refuses.
    pub async fn prune_worktrees(&self) -> anyhow::Result<()> {
        let mut command = command(&self.root);
        command.args(["worktree", "prune"]);
        finish(command, "git worktree prune").await
    }

    /// Every worktree this repository has registered, main first.
    ///
    /// # Errors
    ///
    /// If `git` cannot be run, or if it refuses.
    pub async fn worktrees(&self) -> anyhow::Result<Vec<PathBuf>> {
        let listed = run(&self.root, &["worktree", "list", "--porcelain"]).await?;
        Ok(worktree_paths(&listed))
    }

    /// Whether the working tree at `path` has modified or untracked files.
    ///
    /// The same question `git worktree remove` asks itself, asked early: the
    /// refusal belongs before `amx work done` kills a workspace, not after.
    /// `status --porcelain` says nothing at all about a clean tree, which is the
    /// whole of the test.
    ///
    /// # Errors
    ///
    /// If `git` cannot be run, or if `path` is not a working tree.
    pub async fn is_dirty(&self, path: &Path) -> anyhow::Result<bool> {
        let out = run(path, &["status", "--porcelain"]).await?;
        Ok(!out.trim().is_empty())
    }
}

/// Refuse a branch name git would not take, or would take as an option.
///
/// Asked once, by the verb, before the name reaches any of [`Repo`]'s methods —
/// they check the shape again for themselves, which costs nothing, and leave the
/// grammar to the one call that pays for a process.
///
/// # Errors
///
/// If the name is empty, begins with `-`, or is one `git check-ref-format
/// --branch` rejects.
pub async fn check_branch_name(branch: &str) -> anyhow::Result<()> {
    check_branch_shape(branch)?;
    // No `-C`: the grammar of a ref name is not a property of a repository, and
    // asking inside one would tie the check to a directory that may not exist.
    let out = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .stdin(Stdio::null())
        .output()
        .await
        .context("run git check-ref-format")?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if said.is_empty() {
        bail!("git will not take {branch:?} as a branch name");
    }
    bail!("{said}");
}

/// The half of [`check_branch_name`] that runs no process.
///
/// Separate because it has to hold *before* the name reaches any argument
/// vector: `git check-ref-format --branch -x` would read `-x` as an option of
/// the checker, so the leading-dash rule cannot be delegated to the checker.
fn check_branch_shape(branch: &str) -> anyhow::Result<()> {
    if branch.is_empty() {
        bail!("a branch name cannot be empty");
    }
    if branch.starts_with('-') {
        bail!("a branch name cannot begin with '-': {branch:?} would be read as an option");
    }
    Ok(())
}

/// The `worktree` lines of `git worktree list --porcelain`, in order.
///
/// The format is one attribute per line and a blank line between records, with
/// `worktree <path>` first in each; main worktree first overall. Only the paths
/// are read, because they are all amx joins on.
fn worktree_paths(listed: &str) -> Vec<PathBuf> {
    listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

/// A `git` invocation rooted at `dir`, with nothing on its stdin.
///
/// `-C` rather than `Command::current_dir` so the directory is part of the
/// argument vector the failure message can quote.
fn command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).stdin(Stdio::null());
    command
}

/// Run a git command that answers on stdout, and hand back what it said.
async fn run(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = command(dir)
        .args(args)
        .output()
        .await
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !out.status.success() {
        return Err(refusal(&format!("git {}", args.join(" ")), &out));
    }
    String::from_utf8(out.stdout).with_context(|| format!("git {} said non-UTF-8", args.join(" ")))
}

/// Run a git command whose only answer is whether it worked.
async fn finish(mut command: Command, what: &str) -> anyhow::Result<()> {
    let out = command
        .output()
        .await
        .with_context(|| format!("run {what}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(refusal(what, &out))
}

/// What git said it refused for, or its exit status when it said nothing.
///
/// git's own sentence, verbatim: it names the path, the branch and the worktree
/// that is already holding it, and an amx paraphrase of any of that would be a
/// guess about a tree amx cannot see.
fn refusal(what: &str, out: &std::process::Output) -> anyhow::Error {
    let said = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if said.is_empty() {
        return match out.status.code() {
            Some(code) => anyhow!("{what} failed with status {code}"),
            None => anyhow!("{what} was killed by a signal"),
        };
    }
    anyhow!("{said}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{check_branch_shape, worktree_paths};

    #[test]
    fn a_branch_name_that_would_be_read_as_an_option_is_refused_before_any_argv() {
        assert!(check_branch_shape("feature/x").is_ok());
        assert!(check_branch_shape("").is_err());
        for option in ["-f", "--force", "-"] {
            assert!(
                check_branch_shape(option).is_err(),
                "{option:?} must never reach an argument vector",
            );
        }
    }

    #[test]
    fn the_porcelain_list_reads_main_first_and_ignores_every_other_attribute() {
        let listed = "\
worktree /src/repo
HEAD 4b0e1cf1a3ab253034d20176daa3e1612421e588
branch refs/heads/main

worktree /src/repo--feat
HEAD 4b0e1cf1a3ab253034d20176daa3e1612421e588
branch refs/heads/feat
";
        assert_eq!(
            worktree_paths(listed),
            vec![PathBuf::from("/src/repo"), PathBuf::from("/src/repo--feat"),],
        );
        assert!(worktree_paths("").is_empty());
    }
}
