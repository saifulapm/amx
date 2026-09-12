//! A tree of the repository for the agent to work in.
//!
//! An agent gets its own worktree by default: `<repo>/.amx/worktrees/<id>` on
//! branch `amx/<id>`, cut from the commit that was checked out when it
//! started, or from whatever ref `--base` names instead. Two consequences
//! that shape the rest of amx: several agents can work in one repository
//! without treading on each other, and `diff` has something exact to measure
//! from — the base the tree was cut from, not whatever HEAD has since become.
//!
//! The worktrees live inside the repository so they are easy to find, and are
//! kept out of its status through `.git/info/exclude` rather than
//! `.gitignore`: the ignore is amx's business and does not belong in a file
//! the repository's own commits carry.

use anyhow::{Context, Result, anyhow, bail};
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
    /// The commit it was cut from, recorded at creation and never re-read.
    /// What `diff` measures from, through the last commit it and the tree's own
    /// history still share.
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

/// The same question as [`main_repo`], answered off the path alone.
///
/// [`main_repo`] asks git, from inside the tree — which answers nothing once
/// somebody has deleted the directory, and that is exactly when the repository
/// still has to be named: a tree git is holding a record of, and a branch, both
/// outlive it. The layout [`path_for`] lays down is the other way to the same
/// answer, and it needs nothing on disk. Only for a tree amx cut, since the
/// layout is the whole of the reasoning.
pub fn repo_of(worktree: &Path) -> Option<PathBuf> {
    // <repo>/.amx/worktrees/<id>: three steps back up from the id.
    is_amx_tree(worktree).then(|| worktree.ancestors().nth(3).map(Path::to_path_buf))?
}

/// The branch amx gives an agent's tree.
pub fn branch_for(id: &str) -> String {
    format!("amx/{id}")
}

/// Where an agent's tree goes.
pub fn path_for(repo: &Path, id: &str) -> PathBuf {
    repo.join(WORKTREES).join(id)
}

/// Whether `path` is a tree amx made: `<repo>/.amx/worktrees/<id>`, spelled
/// out from the root.
///
/// The question anything acting on a tree's behalf has to answer first. amx
/// speaks for the trees it cut and for nothing else — not for the repository
/// they sit in, and not for a directory somebody happened to point it at.
pub fn is_amx_tree(path: &Path) -> bool {
    path.is_absolute()
        && path
            .file_name()
            .and_then(|id| id.to_str())
            .is_some_and(crate::ids::is_valid)
        && path
            .parent()
            .is_some_and(|holds| holds.ends_with(WORKTREES))
}

/// Cut a tree for `id` from the commit `from` names, or from the repository's
/// current commit when it names nothing.
///
/// The ref is resolved before anything is made, so a name this repository does
/// not know is a refusal rather than a tree on the wrong commit.
pub fn create(repo: &Path, id: &str, from: Option<&str>) -> Result<Worktree> {
    let base = match from {
        Some(named) => commit_of(repo, named)?,
        None => git(repo, &["rev-parse", "HEAD"])
            .context("this repository has no commit to cut a worktree from yet")?,
    };
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

/// Cut a tree for `id` on `branch`, which the ref `fetch` names in the origin
/// and this checkout may not have at all.
///
/// What a pull request is: work that lives on the forge. There is no local
/// branch to cut from until one is fetched, and the fetch is what makes the
/// name a branch rather than a commit nobody can push from — so this is
/// [`create`]'s shape with the branch arriving instead of being made. The
/// caller picks the name, because the head ref's own name is sometimes taken.
///
/// `+` on the refspec so a branch fetched once and fetched again moves to the
/// commit the request is at now rather than refusing; no `-b` on the add,
/// since after the fetch the branch is already there. The base is read back
/// off the branch rather than taken from the caller's answer about it: what
/// the tree actually holds is what `diff` has to measure from.
pub fn create_on(repo: &Path, id: &str, branch: &str, fetch: &str) -> Result<Worktree> {
    ensure_excluded(repo)?;
    git(
        repo,
        &["fetch", "origin", &format!("+{fetch}:refs/heads/{branch}")],
    )?;

    let path = path_for(repo, id);
    git(repo, &["worktree", "add", &path.to_string_lossy(), branch])?;
    let base = commit_of(repo, branch)?;

    Ok(Worktree {
        path,
        branch: branch.to_string(),
        base,
    })
}

/// Whether some tree in this repository already has `branch` checked out.
///
/// git allows one tree per branch, so the question a name has to answer before
/// it is used: an agent already working on a request's branch is a reason to
/// cut the next tree under another name, not a reason to refuse the spawn.
pub fn checked_out(repo: &Path, branch: &str) -> Result<bool> {
    let listed = git(repo, &["worktree", "list", "--porcelain"])?;
    let named = format!("branch refs/heads/{branch}");
    Ok(listed.lines().any(|line| line.trim_end() == named))
}

/// Whether every commit on `branch` is already in the repository's main line.
///
/// The other way an agent's work can be finished with. A request that was
/// merged says so on the forge, but plenty of work goes in without one — a
/// person pulling the branch and merging it themselves — and afterwards the
/// branch and the tree are a copy of history nobody needs. A branch git does
/// not have is in nothing, which is what `--list` answering with nothing says.
pub fn is_merged(repo: &Path, branch: &str) -> Result<bool> {
    let main = main_branch(repo);
    Ok(!git(repo, &["branch", "--merged", &main, "--list", branch])?.is_empty())
}

/// What this repository calls its main line.
///
/// The origin's own answer first, since that is the branch the forge merges
/// into and the only one of the three that is a fact rather than a convention.
/// Then `main` where there is one, then `master`. A repository with neither is
/// one git has no main line to be asked about, and the question above hands
/// that back as the failure git called it.
///
/// Public because a sweep says why it is about to take an agent, and the name
/// is half of that sentence: `merged into main` is a reason somebody can check,
/// and `merged` on its own is amx asking to be trusted.
pub fn main_branch(repo: &Path) -> String {
    if let Ok(named) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let branch = named.strip_prefix("origin/").unwrap_or(&named);
        if !branch.is_empty() {
            return branch.to_string();
        }
    }
    match git(repo, &["rev-parse", "--verify", "refs/heads/main"]) {
        Ok(_) => "main".to_string(),
        Err(_) => "master".to_string(),
    }
}

/// The commit a ref names, whatever kind of ref it is: a branch, a tag, a
/// remote-tracking name, or a commit written out.
///
/// `^{commit}` is what makes a tag answer with the commit it points at rather
/// than with the tag object, and `--verify` is what makes a name git cannot
/// resolve a failure rather than the word itself handed back. git's own
/// sentence about it says nothing a person typing a branch name needs, so the
/// refusal is amx's own and names what was typed.
fn commit_of(repo: &Path, named: &str) -> Result<String> {
    git(
        repo,
        &["rev-parse", "--verify", &format!("{named}^{{commit}}")],
    )
    .map_err(|_| anyhow!("{named} is no commit to cut a worktree from"))
}

/// The tree a setup command is run in.
pub const WORKTREE_ENV: &str = "AMX_WORKTREE";

/// The repository that tree was cut from.
pub const REPO_ENV: &str = "AMX_REPO";

/// Furnish a tree that has just been cut: the files `copy` names taken from
/// the repository, the directories `link` names pointed at the repository's
/// own, and then `setup`, command by command, in the tree.
///
/// What a fresh checkout is missing is exactly what git is right not to carry —
/// the `.env` nobody commits, the install that takes four minutes — so without
/// this an agent's first turn goes on an install or on a failed test rather
/// than on the task.
///
/// The answer is the paths that were not in the repository, said by name: a key
/// naming a file this repository does not have is a config file outliving one
/// of somebody's projects, not a reason to refuse them an agent. A setup
/// command that fails is the other way round and is an error carrying what it
/// said, because the tree is what the agent was going to work in and one that
/// is half furnished is worse than none.
///
/// `env` is what the caller knows and this does not: which agent this is, and
/// where it may scribble. The tree and the repository are set from the
/// arguments themselves.
pub fn furnish(
    repo: &Path,
    tree: &Path,
    copy: &[String],
    link: &[String],
    setup: &[String],
    env: &[(String, String)],
) -> Result<Vec<String>> {
    let mut missing = Vec::new();

    for path in copy {
        let from = repo.join(path);
        if !from.exists() {
            missing.push(format!("{path} is not in {}", repo.display()));
            continue;
        }
        let to = tree.join(path);
        make_way_for(&to)?;
        std::fs::copy(&from, &to)
            .with_context(|| format!("copying {path} into {}", tree.display()))?;
    }

    for path in link {
        let from = repo.join(path);
        if !from.exists() {
            missing.push(format!("{path} is not in {}", repo.display()));
            continue;
        }
        let at = tree.join(path);
        make_way_for(&at)?;
        std::os::unix::fs::symlink(&from, &at)
            .with_context(|| format!("linking {path} into {}", tree.display()))?;
    }

    for command in setup {
        run_setup(repo, tree, command, env)?;
    }

    Ok(missing)
}

/// The directory a copy or a link is about to go in, since a path is exact and
/// `config/local.toml` names one the tree may not have.
fn make_way_for(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))
}

/// One setup command, run in the tree through `sh -c`.
///
/// What it prints goes nowhere: `new` prints the id and nothing else, and an
/// install's progress is not the id. Its stderr is kept for the refusal, which
/// is the only place any of it is ever said.
fn run_setup(repo: &Path, tree: &Path, command: &str, env: &[(String, String)]) -> Result<()> {
    let out = Command::new("sh")
        .current_dir(tree)
        .args(["-c", command])
        .env(WORKTREE_ENV, tree)
        .env(REPO_ENV, repo)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running setup {command:?}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        match said.trim() {
            "" => bail!("setup {command:?} failed and said nothing"),
            said => bail!("setup {command:?}: {said}"),
        }
    }
    Ok(())
}

/// Whether `dir` holds tracked work to move into a tree: what is staged and
/// what is not, against the commit it has checked out.
///
/// Not [`is_dirty`]'s question. That one counts a file git has never heard of,
/// because removing a tree over one would lose it; this one is about the work
/// `--with-changes` moves, and a file git is not tracking is left where it was
/// made.
///
/// Somewhere that is not a repository has nothing tracked to move, which is
/// the answer a directory whose work is all committed gives too.
pub fn has_changes_to_carry(dir: &Path) -> Result<bool> {
    if repo_root(dir)?.is_none() {
        return Ok(false);
    }
    Ok(!git(dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty())
}

/// Move the work no commit holds out of `from` and into `tree`, answering
/// whether there was any.
///
/// The half hour you had already spent when you thought to start an agent on
/// it: without this it stays in the directory you typed the command in, where
/// the agent working in the tree cannot see it.
///
/// The work is put in the tree before it is taken out of `from`, so the moment
/// where it is in neither never happens. A stash that will not apply — onto a
/// base the work was not written against, over a file furnishing copied in —
/// leaves the directory exactly as it was.
///
/// Files git is not tracking stay: an untracked file is as likely to be a
/// build's output or a scratch note as work, and the two are told apart by the
/// person who made them rather than by amx.
pub fn carry_changes(from: &Path, tree: &Path) -> Result<bool> {
    // `stash create` writes the commit and nothing else: no entry on the stack
    // for another spawn to pop by mistake, and an empty answer where there is
    // nothing tracked to move. The tree reads the commit out of the object
    // store the two of them share.
    let stashed = git(from, &["stash", "create"])?;
    if stashed.is_empty() {
        return Ok(false);
    }
    git(tree, &["stash", "apply", &stashed]).with_context(|| {
        format!(
            "moving the work in {} into {}",
            from.display(),
            tree.display()
        )
    })?;
    git(from, &["reset", "--hard", "HEAD"])?;
    Ok(true)
}

/// Whether the tree holds work that no commit has: changes to tracked files,
/// and files git has never heard of alike. Untracked files count — an agent's
/// first act is usually a new file, and deleting one because git did not know
/// about it is the kind of loss amx cannot undo.
///
/// It reads the tree, so it runs with [`nothing_to_run`] too: git refreshes
/// the index to answer this, and a refresh runs a hook and hashes what it
/// cannot vouch for through whatever filter an attribute names.
pub fn is_dirty(worktree: &Path) -> Result<bool> {
    let safe = nothing_to_run(worktree);
    Ok(!git_with(worktree, &safe, &["status", "--porcelain"])?.is_empty())
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

/// Take a tree back out with the branch it was cut on, whatever is in it.
///
/// The undo for a tree nobody has worked in yet: [`furnish`] failed in it, so
/// everything it holds amx put there and there is nothing to lose. [`remove`]'s
/// refusal is for the other tree, the one with an agent's afternoon in it. The
/// branch goes too, because it was cut a moment ago and holds no commit of its
/// own, and leaving it behind would refuse the next spawn under the same name.
pub fn discard(repo: &Path, worktree: &Path, branch: &str) -> Result<()> {
    git(
        repo,
        &["worktree", "remove", "--force", &worktree.to_string_lossy()],
    )?;
    delete_branch(repo, branch)
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

/// Write what the agent has done to its tree, since the commit it started
/// from, while it is still doing it. With `stat`, the shape of that work
/// rather than the work: a file per line and the totals under them.
///
/// Measured from [`work_began_at`] rather than from `base` itself, so a tree
/// whose history has moved off the recorded commit still reads as the agent's
/// work.
///
/// The `add -N` is the trick: an agent's first act is usually a *new* file,
/// and `git diff` alone says nothing about a file git has never heard of.
/// Recording the intent to add it makes it a diff against nothing, and records
/// nothing else — the agent's own staged work is left as it is. It is done for
/// the summary too, since a summary that leaves out the new files is a summary
/// of the wrong afternoon.
///
/// Both halves run with [`nothing_to_run`]: reading an agent's work must not
/// run the agent's work.
pub fn diff(worktree: &Path, base: &str, stat: bool, out: &mut impl Write) -> Result<()> {
    let safe = nothing_to_run(worktree);
    git_with(worktree, &safe, &["add", "-N", "."])?;
    let from = work_began_at(worktree, base, &safe);

    // `--no-ext-diff` and `--no-textconv` are the same refusal as the
    // overrides, in the form git offers for the two of them it has a flag for.
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv"];
    if stat {
        args.push("--stat");
    }
    args.push(&from);

    // A day's work is a long patch, so it is copied out as git writes it
    // rather than held whole.
    let mut child = command(worktree, &safe, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `git diff`")?;
    let mut printed = child.stdout.take().expect("stdout was asked for");
    std::io::copy(&mut printed, out).context("reading the diff")?;

    let finished = child.wait_with_output().context("waiting for `git diff`")?;
    if !finished.status.success() {
        bail!(
            "git diff {from}: {}",
            String::from_utf8_lossy(&finished.stderr).trim()
        );
    }
    Ok(())
}

/// Where the agent's work began: the last commit its tree and the base it was
/// cut from still share.
///
/// The base is written down when the tree is cut and never re-read, and a tree
/// whose history has since moved off that commit — rebased onto another line,
/// or onto a base somebody rewrote underneath it — is not a tree that commit
/// describes any more. Measured from it, an answer then carries the base's own
/// work backwards: a file it added deleted, a line it changed changed back,
/// none of it the agent's. The commit the two histories still share is where
/// the agent's work actually starts, and for the tree that has stayed on top of
/// its base — which is most of them — that commit is the base itself.
///
/// Read every time rather than recorded, because it is the tree's own history
/// that moves and the record is of the commit it was cut from.
///
/// A base this tree shares no history with is measured from as it was recorded,
/// which leaves what git says about it to git: an answer taken from somewhere
/// else instead would be a patch that looks right and is not.
fn work_began_at(worktree: &Path, base: &str, safe: &[String]) -> String {
    match git_with(worktree, safe, &["merge-base", base, "HEAD"]) {
        Ok(shared) if !shared.is_empty() => shared,
        _ => base.to_string(),
    }
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

/// The overrides that leave a tree amx is only reading with nothing to run.
///
/// A repository's config is a list of programs: `diff.external` for the patch
/// itself, a `textconv` or a `clean` filter for whichever paths an attribute
/// picks out, a hook for the index git refreshes on its way past. Every
/// one of them can be written by the agent whose work is about to be read,
/// from inside the tree it works in, and every one of them then runs as the
/// person reading it. `-c` beats every config file, so each key goes there
/// with nothing in it.
///
/// The filter drivers have to be named one at a time, since there is no
/// wildcard to blank them with, and `required` goes with them: a required
/// filter that has been blanked is a fatal error rather than a plain diff.
/// What that costs is a filtered file compared as git stored it rather than as
/// the filter would have rendered it, and git only hashes a file whose stat
/// data moved — so the files this can read differently are the ones the agent
/// touched, which are the ones the answer was going to name anyway.
fn nothing_to_run(dir: &Path) -> Vec<String> {
    let mut safe = vec!["-c".to_string(), "core.hooksPath=/dev/null".to_string()];
    for driver in filter_drivers(dir) {
        for key in ["clean", "smudge", "process"] {
            safe.push("-c".to_string());
            safe.push(format!("filter.{driver}.{key}="));
        }
        safe.push("-c".to_string());
        safe.push(format!("filter.{driver}.required=false"));
    }
    safe
}

/// The filter drivers this repository's config declares, once each.
///
/// A value read: `--get-regexp` exits as a failure when nothing matches, which
/// is what most repositories answer, and a config git will not list is a
/// config amx has nothing to blank.
fn filter_drivers(dir: &Path) -> Vec<String> {
    let listed = git(
        dir,
        &["config", "--name-only", "--get-regexp", r"^filter\."],
    )
    .unwrap_or_default();
    drivers_in(&listed)
}

/// The driver out of each `filter.<driver>.<key>` line, once each.
///
/// The name is what lies between the two ends, dots and all: a driver may be
/// called `git-lfs.2` and the key after it is what says where it stops. The
/// same driver arrives once per key and once per file that declares it.
fn drivers_in(listed: &str) -> Vec<String> {
    let mut drivers: Vec<String> = listed
        .lines()
        .filter_map(|key| key.trim().strip_prefix("filter."))
        .filter_map(|rest| rest.rsplit_once('.'))
        .filter(|(driver, _)| !driver.is_empty())
        .map(|(driver, _)| driver.to_string())
        .collect();
    drivers.sort();
    drivers.dedup();
    drivers
}

/// One git command, with its output as the answer.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_with(dir, &[], args)
}

/// The same, with config overrides in front of the subcommand.
fn git_with(dir: &Path, overrides: &[String], args: &[&str]) -> Result<String> {
    let out = command(dir, overrides, args)
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

/// git, pointed at `dir` and ready to run.
///
/// `core.fsmonitor` is blanked on every command, not only the reading ones: it
/// names a program git starts before it will so much as look at a file, and a
/// repository amx is asking a question of does not get to start one. It is a
/// cache and nothing amx asks for depends on it.
fn command(dir: &Path, overrides: &[String], args: &[&str]) -> Command {
    let mut git = Command::new("git");
    git.current_dir(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(overrides)
        .args(args)
        .stdin(Stdio::null())
        // The overrides blank the keys amx knows to blank; the machine's own
        // /etc/gitconfig can name programs under keys nobody thought of, and
        // nothing amx asks git for depends on it.
        .env("GIT_CONFIG_NOSYSTEM", "1");
    git
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
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

    /// A bare repository beside this one, added as its `origin`, standing in
    /// for the forge a request would be fetched from.
    fn an_origin(repo: &Path) -> TempDir {
        let bare = TempDir::new().unwrap();
        setup(bare.path(), &["init", "--bare", "-b", "main"]);
        setup(
            repo,
            &["remote", "add", "origin", &bare.path().to_string_lossy()],
        );
        setup(repo, &["push", "-q", "origin", "main"]);
        bare
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

        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
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
    fn worktree_is_cut_from_the_ref_it_was_given() {
        // Whatever kind of ref it is: a branch, a tag and a commit written out
        // are three spellings of one commit, and the tree holds that commit's
        // work rather than whatever HEAD has become.
        let repo = a_repo();
        let first = setup(repo.path(), &["rev-parse", "HEAD"]);
        setup(repo.path(), &["tag", "v1"]);
        setup(repo.path(), &["branch", "release"]);
        std::fs::write(repo.path().join("README.md"), "after\n").unwrap();
        setup(repo.path(), &["commit", "-am", "second"]);

        for (id, named) in [
            ("fix-login-a1b", "release"),
            ("fix-login-a2b", "v1"),
            ("fix-login-a3b", &first[..8]),
        ] {
            let tree = create(repo.path(), id, Some(named)).unwrap();
            assert_eq!(tree.base, first, "cut from {named}");
            assert_eq!(
                std::fs::read_to_string(tree.path.join("README.md")).unwrap(),
                "before\n",
                "the work of the commit {named} names, not of HEAD"
            );
        }
    }

    #[test]
    fn worktree_is_cut_on_a_branch_fetched_from_a_ref_nobody_has_locally() {
        // A pull request's head is a ref in the origin and nothing in this
        // checkout, so the branch has to be fetched into existence before
        // there is anything to cut a tree on.
        let repo = a_repo();
        let _origin = an_origin(repo.path());
        setup(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("login.rs"), "fn login() {}\n").unwrap();
        setup(repo.path(), &["add", "login.rs"]);
        setup(repo.path(), &["commit", "-m", "the request's own work"]);
        let head = setup(repo.path(), &["rev-parse", "HEAD"]);
        setup(
            repo.path(),
            &["push", "-q", "origin", "HEAD:refs/pull/7/head"],
        );
        setup(repo.path(), &["checkout", "-q", "main"]);
        setup(repo.path(), &["branch", "-D", "feature"]);

        let tree = create_on(repo.path(), "review-7-a1b", "feature", "refs/pull/7/head").unwrap();

        assert_eq!(tree.branch, "feature", "the request's own head ref");
        assert_eq!(tree.base, head, "recorded at the commit that head is");
        assert_eq!(tree.path, repo.path().join(".amx/worktrees/review-7-a1b"));
        assert_eq!(
            setup(&tree.path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature",
            "and the tree is on the branch rather than on a detached head"
        );
        assert_eq!(setup(&tree.path, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(tree.path.join("login.rs")).unwrap(),
            "fn login() {}\n",
            "so the work under review is what the agent opens"
        );
        assert_eq!(
            setup(repo.path(), &["status", "--porcelain"]),
            "",
            "and this tree is kept out of the status like any other"
        );
    }

    #[test]
    fn worktree_refuses_a_ref_the_origin_does_not_have_and_leaves_nothing() {
        let repo = a_repo();
        let _origin = an_origin(repo.path());

        let refused =
            create_on(repo.path(), "review-9-a1b", "pr-9", "refs/pull/9/head").unwrap_err();
        assert!(
            format!("{refused:#}").contains("refs/pull/9/head"),
            "the ref that was asked for: {refused:#}"
        );
        assert!(
            !repo.path().join(".amx/worktrees/review-9-a1b").exists(),
            "and no tree was cut for it"
        );
        assert_eq!(
            setup(repo.path(), &["branch", "--list", "pr-9"]),
            "",
            "nor a branch"
        );
    }

    #[test]
    fn worktree_says_which_branches_some_tree_already_holds() {
        // Two trees cannot hold one branch, so a name that is taken is a name
        // a request has to be cut under some other one.
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        setup(repo.path(), &["branch", "release"]);

        assert!(checked_out(repo.path(), &tree.branch).unwrap());
        assert!(
            checked_out(repo.path(), "main").unwrap(),
            "the repository's own checkout holds one too"
        );
        assert!(
            !checked_out(repo.path(), "release").unwrap(),
            "a branch no tree is on is a name that is free"
        );
        assert!(
            !checked_out(repo.path(), "mai").unwrap(),
            "and the name is the whole name, not the start of one"
        );
    }

    #[test]
    fn worktree_says_whether_a_branchs_work_is_in_the_main_line() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        setup(&tree.path, &["add", "login.rs"]);
        setup(&tree.path, &["commit", "-m", "the agent's own commit"]);

        assert!(
            !is_merged(repo.path(), &tree.branch).unwrap(),
            "work nothing has taken yet"
        );

        setup(repo.path(), &["merge", "-q", &tree.branch]);
        assert!(
            is_merged(repo.path(), &tree.branch).unwrap(),
            "and the same branch once main holds every commit on it"
        );

        assert!(
            !is_merged(repo.path(), "amx/never-cut-b2c").unwrap(),
            "a branch git does not have is in nothing"
        );
    }

    #[test]
    fn worktree_asks_the_origin_what_the_main_line_is_before_it_guesses() {
        // The name is the forge's to say: a repository whose default branch is
        // `trunk` would otherwise have every branch read as unmerged, and a
        // sweep that believes that never sweeps anything.
        let repo = a_repo();
        assert_eq!(main_branch(repo.path()), "main");

        setup(repo.path(), &["branch", "trunk"]);
        setup(
            repo.path(),
            &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
        );
        setup(
            repo.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        );
        assert_eq!(main_branch(repo.path()), "trunk");

        // And where the origin says nothing, whichever of the two names this
        // repository actually has.
        let old = a_repo();
        setup(old.path(), &["branch", "-m", "master"]);
        assert_eq!(main_branch(old.path()), "master");
    }

    #[test]
    fn worktree_refuses_a_ref_that_is_no_commit_before_anything_is_made() {
        let repo = a_repo();

        let refused = create(repo.path(), "fix-login-a1b", Some("release")).unwrap_err();
        let said = format!("{refused:#}");
        assert!(said.contains("release"), "the ref that was typed: {said}");
        assert!(said.contains("no commit"), "{said}");
        assert!(
            !repo.path().join(".amx").exists(),
            "and no tree was cut for it"
        );
        assert_eq!(
            setup(repo.path(), &["branch", "--list", "amx/fix-login-a1b"]),
            "",
            "nor a branch"
        );

        // A ref git resolves to something that is not a commit is no base
        // either: `^{commit}` is what asks that question of it.
        assert!(create(repo.path(), "fix-login-a1b", Some("HEAD:README.md")).is_err());
    }

    #[test]
    fn worktree_knows_the_repository_it_belongs_to() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

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
    fn worktree_names_its_repository_after_the_directory_has_gone() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        std::fs::remove_dir_all(&tree.path).unwrap();

        assert!(
            main_repo(&tree.path).is_err(),
            "git has nowhere to be asked from"
        );
        assert_eq!(
            repo_of(&tree.path).unwrap(),
            repo.path(),
            "and the layout answers without it"
        );

        // The same refusal is_amx_tree makes: amx speaks for the trees it cut.
        assert_eq!(repo_of(Path::new("/src/app/worktrees/fix-a1b")), None);
        assert_eq!(repo_of(Path::new(".amx/worktrees/fix-a1b")), None);
    }

    #[test]
    fn worktree_records_the_base_even_after_the_repository_moves_on() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

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
        create(repo.path(), "fix-login-a1b", None).unwrap();
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
        create(repo.path(), "port-importer-c3d", None).unwrap();
        let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(EXCLUDE_LINE).count(), 1, "{exclude}");
    }

    #[test]
    fn worktree_diff_shows_a_file_git_has_never_heard_of() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

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
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

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
    fn worktree_diff_is_taken_from_the_last_commit_the_base_and_the_tree_share() {
        // The tree was cut from `second`, and the agent's commit then went on
        // the release line, which `second` is not on. Measured from `second`
        // itself the answer would carry that commit's own work backwards -- the
        // file it added deleted, the line it changed changed back -- all of it
        // reading as the agent's.
        let repo = a_repo();
        setup(repo.path(), &["branch", "release"]);
        std::fs::write(repo.path().join("README.md"), "after\n").unwrap();
        std::fs::write(repo.path().join("shipped.rs"), "fn shipped() {}\n").unwrap();
        setup(repo.path(), &["add", "shipped.rs"]);
        setup(repo.path(), &["commit", "-am", "second"]);
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        setup(&tree.path, &["add", "login.rs"]);
        setup(&tree.path, &["commit", "-m", "the agent's own commit"]);
        setup(&tree.path, &["rebase", "--onto", "release", "main"]);

        let diff = shown(&tree.path, &tree.base, false);
        assert!(diff.contains("+fn login() {}"), "the agent's work: {diff}");
        assert!(
            !diff.contains("shipped.rs") && !diff.contains("-after"),
            "and not the base's own, undone: {diff}"
        );
        assert_eq!(
            tree.base,
            setup(repo.path(), &["rev-parse", "HEAD"]),
            "the record still holds the commit the tree was cut from"
        );

        let summary = shown(&tree.path, &tree.base, true);
        assert!(summary.contains("1 file changed"), "{summary}");
    }

    #[test]
    fn worktree_diff_says_what_git_says_about_a_base_that_is_not_in_the_tree() {
        // Nothing shares a commit with a base this tree has never held, and an
        // answer measured from somewhere else instead would be a patch that
        // looks right and is not.
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let mut out = Vec::new();
        let refused = diff(&tree.path, "0f1e2d3", false, &mut out).unwrap_err();
        assert!(format!("{refused:#}").contains("0f1e2d3"), "{refused:#}");
    }

    #[test]
    fn clibatch_diff_stat_answers_with_the_shape_of_the_work() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

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
    fn hardening_every_git_is_run_with_the_system_config_shut_out() {
        // The per-key overrides blank what amx knows to blank; the machine's
        // own /etc/gitconfig can name programs under keys nobody thought of.
        // GIT_CONFIG_NOSYSTEM is the guard for the whole file at once.
        let dir = TempDir::new().unwrap();
        let git = command(dir.path(), &[], &["status"]);
        let guard = git
            .get_envs()
            .find(|(name, _)| *name == "GIT_CONFIG_NOSYSTEM")
            .and_then(|(_, value)| value);
        assert_eq!(
            guard,
            Some(std::ffi::OsStr::new("1")),
            "every git amx runs, not only the diff"
        );
    }

    #[test]
    fn hardening_a_diff_runs_nothing_the_tree_it_reads_names() {
        // Every one of these is a config key naming a program, and every one
        // of them can be written from inside the tree by the agent whose work
        // is about to be read. The attributes that pick a driver go in
        // `.git/info/attributes`, which is the copy no attribute source can be
        // pointed away from.
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let traps = TempDir::new().unwrap();
        let ran = traps.path().join("ran");
        let program = traps.path().join("trap.sh");
        std::fs::write(
            &program,
            format!("#!/bin/sh\necho ran >> {}\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hooks = traps.path().join("hooks");
        std::fs::create_dir(&hooks).unwrap();
        std::fs::copy(&program, hooks.join("post-index-change")).unwrap();

        let named = program.to_string_lossy().into_owned();
        let hooked = hooks.to_string_lossy().into_owned();
        for (key, value) in [
            ("diff.external", named.as_str()),
            ("diff.trap.textconv", &named),
            ("filter.trap.clean", &named),
            ("filter.trap.required", "true"),
            ("core.fsmonitor", &named),
            ("core.hooksPath", &hooked),
        ] {
            setup(&tree.path, &["config", key, value]);
        }
        std::fs::write(
            repo.path().join(".git/info/attributes"),
            "* diff=trap filter=trap\n",
        )
        .unwrap();

        std::fs::write(tree.path.join("login.rs"), "fn login() {}\n").unwrap();
        std::fs::write(tree.path.join("README.md"), "after\n").unwrap();

        let diff = shown(&tree.path, &tree.base, false);
        assert!(!ran.exists(), "reading the work ran something it named");
        assert!(
            diff.contains("+fn login() {}") && diff.contains("+after"),
            "and the work is still what the diff shows: {diff}"
        );

        assert!(is_dirty(&tree.path).unwrap(), "the tree holds work");
        assert!(
            !ran.exists(),
            "and the look that says so ran nothing either"
        );
    }

    #[test]
    fn hardening_a_filter_driver_is_whatever_lies_between_the_two_ends() {
        let listed = "filter.lfs.clean\nfilter.lfs.smudge\nfilter.git-lfs.2.process\n\
                      filter.lfs.clean\ncore.fsmonitor\nfilter.\n";
        assert_eq!(drivers_in(listed), ["git-lfs.2", "lfs"]);
        assert!(drivers_in("").is_empty(), "and most repositories say this");
    }

    #[test]
    fn worktree_furnish_copies_files_links_directories_and_runs_setup() {
        let repo = a_repo();
        std::fs::write(repo.path().join(".env"), "TOKEN=hunter2\n").unwrap();
        std::fs::create_dir(repo.path().join("node_modules")).unwrap();
        std::fs::write(repo.path().join("node_modules/left-pad"), "installed\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let missing = furnish(
            repo.path(),
            &tree.path,
            &[".env".to_string()],
            &["node_modules".to_string()],
            &["echo built > built".to_string()],
            &[],
        )
        .unwrap();

        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(
            std::fs::read_to_string(tree.path.join(".env")).unwrap(),
            "TOKEN=hunter2\n",
            "the file git is right not to carry"
        );
        assert!(
            std::fs::symlink_metadata(tree.path.join("node_modules"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the directory is the repository's own rather than a second copy"
        );
        assert_eq!(
            std::fs::read_to_string(tree.path.join("node_modules/left-pad")).unwrap(),
            "installed\n"
        );
        assert_eq!(
            std::fs::read_to_string(tree.path.join("built")).unwrap(),
            "built\n",
            "and the setup command ran in the tree"
        );
    }

    #[test]
    fn worktree_furnish_runs_a_setup_command_under_the_agents_own_variables() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        furnish(
            repo.path(),
            &tree.path,
            &[],
            &[],
            &[
                r#"printf '%s\n' "$AMX_ID" "$AMX_WORKTREE" "$AMX_REPO" "$AMX_AGENT_DIR" > said"#
                    .to_string(),
            ],
            &[
                ("AMX_ID".to_string(), "fix-login-a1b".to_string()),
                (
                    "AMX_AGENT_DIR".to_string(),
                    "/state/agents/fix-login-a1b/scratch".to_string(),
                ),
            ],
        )
        .unwrap();

        let said = std::fs::read_to_string(tree.path.join("said")).unwrap();
        assert_eq!(
            said.lines().collect::<Vec<&str>>(),
            [
                "fix-login-a1b",
                tree.path.to_str().unwrap(),
                repo.path().to_str().unwrap(),
                "/state/agents/fix-login-a1b/scratch",
            ],
            "the tree and the repository from the arguments, the rest from the caller"
        );
    }

    #[test]
    fn worktree_furnish_says_what_is_not_in_the_repository_and_goes_on() {
        // A key naming a file this repository does not have is a config file
        // outliving one of somebody's projects, not a reason to refuse them an
        // agent.
        let repo = a_repo();
        std::fs::write(repo.path().join(".env"), "TOKEN=hunter2\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let missing = furnish(
            repo.path(),
            &tree.path,
            &[".env".to_string(), "config/local.toml".to_string()],
            &["node_modules".to_string()],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(missing[0].contains("config/local.toml"), "{missing:?}");
        assert!(missing[1].contains("node_modules"), "{missing:?}");
        assert!(
            !tree.path.join("config").exists() && !tree.path.join("node_modules").exists(),
            "and nothing was made for either of them"
        );
        assert!(
            tree.path.join(".env").exists(),
            "the paths that are there are furnished anyway"
        );
    }

    #[test]
    fn worktree_furnish_stops_at_the_first_setup_that_fails_and_says_what_it_said() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let refused = furnish(
            repo.path(),
            &tree.path,
            &[],
            &[],
            &[
                "echo no such lockfile >&2; exit 3".to_string(),
                "touch second".to_string(),
            ],
            &[],
        )
        .unwrap_err();

        let said = format!("{refused:#}");
        assert!(said.contains("no such lockfile"), "what it said: {said}");
        assert!(said.contains("exit 3"), "and which command said it: {said}");
        assert!(
            !tree.path.join("second").exists(),
            "the ones after it never ran"
        );
    }

    #[test]
    fn worktree_discard_takes_a_tree_nobody_has_worked_in_with_its_branch() {
        let repo = a_repo();
        std::fs::write(repo.path().join(".env"), "TOKEN=hunter2\n").unwrap();
        std::fs::create_dir(repo.path().join("node_modules")).unwrap();
        std::fs::write(repo.path().join("node_modules/left-pad"), "installed\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        furnish(
            repo.path(),
            &tree.path,
            &[".env".to_string()],
            &["node_modules".to_string()],
            &[],
            &[],
        )
        .unwrap();

        assert!(
            remove(repo.path(), &tree.path).is_err(),
            "what furnishing put there reads as work to `remove`"
        );
        discard(repo.path(), &tree.path, &tree.branch).unwrap();

        assert!(!tree.path.exists());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("node_modules/left-pad")).unwrap(),
            "installed\n",
            "a link into the repository is unlinked, never followed"
        );
        assert_eq!(
            setup(repo.path(), &["branch", "--list", &tree.branch]),
            "",
            "and the branch goes with it, so the same name can be spawned again"
        );
    }

    #[test]
    fn worktree_carries_the_tracked_work_into_the_tree_and_leaves_the_rest() {
        let repo = a_repo();
        std::fs::write(repo.path().join("login.rs"), "fn login() {}\n").unwrap();
        setup(repo.path(), &["add", "login.rs"]);
        std::fs::write(repo.path().join("README.md"), "after\n").unwrap();
        std::fs::write(repo.path().join("notes.txt"), "scratch\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        assert!(has_changes_to_carry(repo.path()).unwrap());
        assert!(carry_changes(repo.path(), &tree.path).unwrap());

        assert_eq!(
            std::fs::read_to_string(tree.path.join("README.md")).unwrap(),
            "after\n",
            "the change to a tracked file"
        );
        assert_eq!(
            std::fs::read_to_string(tree.path.join("login.rs")).unwrap(),
            "fn login() {}\n",
            "and the one that was staged"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
            "before\n",
            "the directory it was typed in is left as the last commit had it"
        );
        assert!(
            !repo.path().join("login.rs").exists(),
            "the work moved rather than being copied"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("notes.txt")).unwrap(),
            "scratch\n",
            "a file git has never heard of stays where it was made"
        );
        assert!(!tree.path.join("notes.txt").exists());
        assert!(
            !has_changes_to_carry(repo.path()).unwrap(),
            "and nothing more to move"
        );
    }

    #[test]
    fn worktree_carries_nothing_where_no_commit_is_missing_any_of_it() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        std::fs::write(repo.path().join("notes.txt"), "scratch\n").unwrap();

        // Untracked and nothing else is nothing to move, which is the answer a
        // directory with no changes at all gives.
        assert!(!has_changes_to_carry(repo.path()).unwrap());
        assert!(!carry_changes(repo.path(), &tree.path).unwrap());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("notes.txt")).unwrap(),
            "scratch\n",
            "and it was left alone"
        );

        let elsewhere = TempDir::new().unwrap();
        assert!(
            !has_changes_to_carry(elsewhere.path()).unwrap(),
            "somewhere that is not a repository has nothing tracked either"
        );
    }

    #[test]
    fn worktree_carrying_work_that_will_not_apply_leaves_the_directory_as_it_was() {
        // A tree cut from a commit the work was not written against: the stash
        // does not apply, and what it was going to move is still where it was
        // typed rather than in neither place.
        let repo = a_repo();
        std::fs::write(repo.path().join("README.md"), "second\n").unwrap();
        setup(repo.path(), &["commit", "-am", "second"]);
        let tree = create(repo.path(), "fix-login-a1b", Some("HEAD~1")).unwrap();
        std::fs::write(repo.path().join("README.md"), "third\n").unwrap();

        let refused = carry_changes(repo.path(), &tree.path).unwrap_err();
        assert!(
            format!("{refused:#}").contains("moving the work"),
            "{refused:#}"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
            "third\n",
            "the work is still there to try again with"
        );
    }

    #[test]
    fn worktree_refuses_to_remove_work_no_commit_holds() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
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
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
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
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
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

        let refused = create(dir.path(), "fix-login-a1b", None).unwrap_err();
        let said = format!("{refused:#}");
        assert!(said.contains("commit"), "{said}");
    }

    #[test]
    fn worktree_refuses_a_second_tree_for_the_same_agent() {
        let repo = a_repo();
        create(repo.path(), "fix-login-a1b", None).unwrap();
        assert!(create(repo.path(), "fix-login-a1b", None).is_err());
    }

    #[test]
    fn worktree_knows_a_tree_it_made_from_any_other_directory() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        assert!(is_amx_tree(&tree.path), "{}", tree.path.display());
        assert!(is_amx_tree(Path::new("/src/app/.amx/worktrees/port-c3d")));

        for other in [
            "/src/app",                       // the repository itself
            "/src/app/.amx/worktrees",        // where the trees live
            "/src/app/.amx",                  // amx's own directory
            "/src/app/worktrees/fix-a1b",     // somebody else's layout
            "/src/app/.amx/worktrees/../etc", // not a name amx ever mints
            ".amx/worktrees/fix-a1b",         // and never a relative one
        ] {
            assert!(!is_amx_tree(Path::new(other)), "{other}");
        }
    }
}
