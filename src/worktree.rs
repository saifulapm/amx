//! A tree of the repository for the agent to work in.
//!
//! An agent gets its own worktree by default: `<repo>/.amx/worktrees/<id>` on
//! branch `amx/<id>`, cut from the commit that was checked out when it
//! started. Two consequences that shape the rest of amx: several agents can
//! work in one repository without treading on each other, and `diff` has
//! something exact to compare against — the recorded base commit, not whatever
//! HEAD has since become.
//!
//! The worktrees live inside the repository so they are easy to find, and are
//! kept out of its status through `.git/info/exclude` rather than
//! `.gitignore`: the ignore is amx's business and does not belong in a file
//! the repository's own commits carry.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where amx puts an agent's tree, relative to the repository root.
const WORKTREES: &str = ".amx/worktrees";

/// The line that keeps all of it out of the repository's status.
const EXCLUDE_LINE: &str = "/.amx/";

/// One agent's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// The commit it was cut from. What `diff` compares against, so it is
    /// recorded at creation and never re-read.
    pub base: String,
}

/// The root of the repository `dir` is in, or `None` when it is in none.
pub fn repo_root(dir: &Path) -> Result<Option<PathBuf>> {
    // Not being in a repository is an ordinary answer — `new` falls back to
    // running the agent in the directory as it is — so it is not an error.
    match git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(path) => Ok(Some(PathBuf::from(path))),
        Err(_) => Ok(None),
    }
}

/// The repository a worktree belongs to.
///
/// Not the same question as [`repo_root`], which answers with the tree it was
/// asked in — for a linked worktree that is the worktree itself. What a branch
/// is deleted from, and what a tree is removed from, is the repository they
/// share, and it outlives both.
pub fn main_repo(worktree: &Path) -> Result<PathBuf> {
    let common = git(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Path::new(&common)
        .parent()
        .map(Path::to_path_buf)
        .with_context(|| format!("{common} is not inside a repository"))
}

/// The branch amx gives an agent's tree.
pub fn branch_for(id: &str) -> String {
    format!("amx/{id}")
}

/// Where an agent's tree goes.
pub fn path_for(repo: &Path, id: &str) -> PathBuf {
    repo.join(WORKTREES).join(id)
}

/// Cut a tree for `id` from the repository's current commit.
pub fn create(repo: &Path, id: &str) -> Result<Worktree> {
    let base = git(repo, &["rev-parse", "HEAD"])
        .context("this repository has no commit to cut a worktree from yet")?;
    ensure_excluded(repo)?;

    let path = path_for(repo, id);
    let branch = branch_for(id);
    git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            &base,
        ],
    )?;

    Ok(Worktree { path, branch, base })
}

/// Whether the tree holds work that no commit has: changes to tracked files,
/// and files git has never heard of alike. Untracked files count — an agent's
/// first act is usually a new file, and deleting one because git did not know
/// about it is the kind of loss amx cannot undo.
pub fn is_dirty(worktree: &Path) -> Result<bool> {
    Ok(!git(worktree, &["status", "--porcelain"])?.is_empty())
}

/// Remove the tree, refusing while it holds uncommitted work.
pub fn remove(repo: &Path, worktree: &Path) -> Result<()> {
    if !worktree.exists() {
        // Somebody has already deleted it; all that is left is git's own
        // record of a tree that is not there.
        git(repo, &["worktree", "prune"])?;
        return Ok(());
    }

    if is_dirty(worktree)? {
        bail!("{} holds uncommitted work", worktree.display());
    }
    git(repo, &["worktree", "remove", &worktree.to_string_lossy()])?;
    Ok(())
}

/// Put a tree back where it was, on the branch it already had.
///
/// What `remove` took away, for the agent that is being started again. The
/// branch is not created: this is a tree for work that already exists, and a
/// branch that has gone with it is a reason to say so rather than to make a
/// new one.
pub fn restore(repo: &Path, worktree: &Path, branch: &str) -> Result<()> {
    // git keeps its own record of a tree until somebody tells it the tree is
    // gone, and it refuses to add a tree it believes is already there.
    git(repo, &["worktree", "prune"])?;
    git(
        repo,
        &["worktree", "add", &worktree.to_string_lossy(), branch],
    )?;
    Ok(())
}

/// Delete a branch and whatever is on it. Only ever on request.
pub fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    git(repo, &["branch", "-D", branch])?;
    Ok(())
}

/// Write what the agent has done to its tree, against the commit it started
/// from, while it is still doing it. With `stat`, the shape of that work
/// rather than the work: a file per line and the totals under them.
///
/// The `add -N` is the trick: an agent's first act is usually a *new* file,
/// and `git diff` alone says nothing about a file git has never heard of.
/// Recording the intent to add it makes it a diff against nothing, and records
/// nothing else — the agent's own staged work is left as it is. It is done for
/// the summary too, since a summary that leaves out the new files is a summary
/// of the wrong afternoon.
pub fn diff(worktree: &Path, base: &str, stat: bool, out: &mut impl Write) -> Result<()> {
    git(worktree, &["add", "-N", "."])?;

    let mut args = vec!["diff"];
    if stat {
        args.push("--stat");
    }
    args.push(base);

    // A day's work is a long patch, so it is copied out as git writes it
    // rather than held whole.
    let mut child = Command::new("git")
        .current_dir(worktree)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `git diff`")?;
    let mut printed = child.stdout.take().expect("stdout was asked for");
    std::io::copy(&mut printed, out).context("reading the diff")?;

    let finished = child.wait_with_output().context("waiting for `git diff`")?;
    if !finished.status.success() {
        bail!(
            "git diff {base}: {}",
            String::from_utf8_lossy(&finished.stderr).trim()
        );
    }
    Ok(())
}

/// Keep amx's own directory out of the repository's status.
///
/// `.git/info/exclude` rather than `.gitignore`: the repository's ignore file
/// is versioned and shared, and where amx keeps its trees is neither.
fn ensure_excluded(repo: &Path) -> Result<()> {
    // The common directory, because a linked worktree's own `.git` is a file
    // and the exclude file belongs to the repository they all share.
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let path = Path::new(&common).join("info/exclude");

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == EXCLUDE_LINE) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(file, "{separator}{EXCLUDE_LINE}")
        .with_context(|| format!("writing {}", path.display()))
}

/// One git command, with its output as the answer.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// git as the tests run it: none of the developer's own configuration,
    /// nothing to sign with, and an identity of its own.
    fn setup(dir: &Path, args: &[&str]) -> String {
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
        setup(dir.path(), &["init", "-b", "main"]);
        setup(dir.path(), &["config", "user.name", "amx tests"]);
        setup(
            dir.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::write(dir.path().join("README.md"), "before\n").unwrap();
        setup(dir.path(), &["add", "README.md"]);
        setup(dir.path(), &["commit", "-m", "first"]);
        dir
    }

    fn shown(worktree: &Path, base: &str, stat: bool) -> String {
        let mut out = Vec::new();
        diff(worktree, base, stat, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn worktree_finds_the_repository_a_directory_is_in() {
        let repo = a_repo();
        let nested = repo.path().join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();

        let found = repo_root(&nested).unwrap().unwrap();
        assert_eq!(
            std::fs::canonicalize(found).unwrap(),
            std::fs::canonicalize(repo.path()).unwrap()
        );

        let elsewhere = TempDir::new().unwrap();
        assert_eq!(repo_root(elsewhere.path()).unwrap(), None);
    }

    #[test]
    fn worktree_is_cut_from_the_commit_that_was_checked_out() {
        let repo = a_repo();
        let head = setup(repo.path(), &["rev-parse", "HEAD"]);

        let tree = create(repo.path(), "fix-login-a1b").unwrap();
        assert_eq!(tree.base, head);
        assert_eq!(tree.branch, "amx/fix-login-a1b");
        assert_eq!(tree.path, repo.path().join(".amx/worktrees/fix-login-a1b"));
        assert_eq!(
            std::fs::read_to_string(tree.path.join("README.md")).unwrap(),
            "before\n",
            "the tree holds the repository's own work"
        );
        assert_eq!(
            setup(&tree.path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "amx/fix-login-a1b"
        );
    }

    #[test]
    fn worktree_knows_the_repository_it_belongs_to() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();

        assert_eq!(
            std::fs::canonicalize(main_repo(&tree.path).unwrap()).unwrap(),
            std::fs::canonicalize(repo.path()).unwrap(),
            "a branch is deleted from the repository, not from the tree that holds it"
        );
        assert_eq!(
            std::fs::canonicalize(repo_root(&tree.path).unwrap().unwrap()).unwrap(),
            std::fs::canonicalize(&tree.path).unwrap(),
            "which is not what the tree itself answers"
        );
    }

    #[test]
    fn worktree_records_the_base_even_after_the_repository_moves_on() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();

        std::fs::write(repo.path().join("README.md"), "after\n").unwrap();
        setup(repo.path(), &["commit", "-am", "second"]);

        assert_ne!(
            tree.base,
            setup(repo.path(), &["rev-parse", "HEAD"]),
            "the repository has moved on and the record has not"
        );
    }

    #[test]
    fn worktree_keeps_itself_out_of_the_repositorys_status() {
        let repo = a_repo();
        create(repo.path(), "fix-login-a1b").unwrap();
        assert_eq!(
            setup(repo.path(), &["status", "--porcelain"]),
            "",
            "an agent's tree must not read as work in the repository"
        );

        let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains(EXCLUDE_LINE), "{exclude}");
        assert!(
            !repo.path().join(".gitignore").exists(),
            "the repository's own ignore file is not amx's to write"
        );

        // A second tree must not write the line again.
        create(repo.path(), "port-importer-c3d").unwrap();
        let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(EXCLUDE_LINE).count(), 1, "{exclude}");
    }

    #[test]
    fn worktree_diff_shows_a_file_git_has_never_heard_of() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();

        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        std::fs::write(tree.path.join("README.md"), "after\n").unwrap();

        let diff = shown(&tree.path, &tree.base, false);
        assert!(diff.contains("+fn login() {}"), "the new file: {diff}");
        assert!(
            diff.contains("-before"),
            "the change to a tracked file: {diff}"
        );
        assert!(diff.contains("+after"), "{diff}");
    }

    #[test]
    fn worktree_diff_is_against_the_base_and_not_the_agents_own_head() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();

        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        setup(&tree.path, &["add", "login.rs"]);
        setup(&tree.path, &["commit", "-m", "the agent's own commit"]);

        let diff = shown(&tree.path, &tree.base, false);
        assert!(
            diff.contains("+fn login() {}"),
            "committed work is still work done since the base: {diff}"
        );
    }

    #[test]
    fn clibatch_diff_stat_answers_with_the_shape_of_the_work() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();

        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        std::fs::write(tree.path.join("README.md"), "after\n").unwrap();

        let summary = shown(&tree.path, &tree.base, true);
        assert!(summary.contains("login.rs"), "the new file: {summary}");
        assert!(
            summary.contains("README.md"),
            "and the changed one: {summary}"
        );
        assert!(summary.contains("2 files changed"), "{summary}");
        assert!(
            !summary.contains("+fn login() {}"),
            "a summary is not the patch: {summary}"
        );
    }

    #[test]
    fn worktree_refuses_to_remove_work_no_commit_holds() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();
        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();

        assert!(is_dirty(&tree.path).unwrap());
        let refused = remove(repo.path(), &tree.path).unwrap_err();
        assert!(
            format!("{refused:#}").contains("uncommitted"),
            "{refused:#}"
        );
        assert!(tree.path.exists(), "and it is still there");
    }

    #[test]
    fn worktree_removes_a_tree_whose_work_is_committed_and_leaves_the_branch() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();
        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        setup(&tree.path, &["add", "login.rs"]);
        setup(&tree.path, &["commit", "-m", "the agent's own commit"]);

        assert!(!is_dirty(&tree.path).unwrap());
        remove(repo.path(), &tree.path).unwrap();
        assert!(!tree.path.exists());

        // The work lives on the branch, which is not removed with the tree.
        let branches = setup(repo.path(), &["branch", "--list", &tree.branch]);
        assert!(branches.contains(&tree.branch), "{branches}");

        delete_branch(repo.path(), &tree.branch).unwrap();
        assert_eq!(setup(repo.path(), &["branch", "--list", &tree.branch]), "");
    }

    #[test]
    fn worktree_removing_one_that_is_already_gone_is_not_a_failure() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b").unwrap();
        std::fs::remove_dir_all(&tree.path).unwrap();

        remove(repo.path(), &tree.path).unwrap();
        assert_eq!(
            setup(repo.path(), &["worktree", "list", "--porcelain"])
                .matches("fix-login-a1b")
                .count(),
            0,
            "and git is no longer holding a record of it"
        );
    }

    #[test]
    fn worktree_says_so_when_the_repository_has_no_commits_to_cut_from() {
        let dir = TempDir::new().unwrap();
        setup(dir.path(), &["init", "-b", "main"]);

        let refused = create(dir.path(), "fix-login-a1b").unwrap_err();
        let said = format!("{refused:#}");
        assert!(said.contains("commit"), "{said}");
    }

    #[test]
    fn worktree_refuses_a_second_tree_for_the_same_agent() {
        let repo = a_repo();
        create(repo.path(), "fix-login-a1b").unwrap();
        assert!(create(repo.path(), "fix-login-a1b").is_err());
    }
}
