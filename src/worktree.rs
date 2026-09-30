//! Git worktrees for agents, and the git calls amx makes against them.
//!
//! An agent gets its own tree by default: `<repo>/.amx/worktrees/<id>` on
//! branch `amx/<id>`, cut from the checked-out commit or from `--base`. The
//! recorded base is what `diff` measures from. The trees live inside the
//! repository and are hidden from its status through `.git/info/exclude`, so
//! nothing is written to the versioned `.gitignore`.

use anyhow::{Context, Result, anyhow, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where agent trees go, relative to the repository root.
const WORKTREES: &str = ".amx/worktrees";

/// The exclude line that hides amx's directory from the repository's status.
const EXCLUDE_LINE: &str = "/.amx/";

/// One agent's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// The commit the tree was cut from, recorded once and never re-read.
    pub base: String,
}

/// The root of the repository `dir` is in, or `None` outside any repository.
pub fn repo_root(dir: &Path) -> Result<Option<PathBuf>> {
    // Outside a repository `new` runs the agent in the directory as it is.
    match git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(path) => Ok(Some(PathBuf::from(path))),
        Err(_) => Ok(None),
    }
}

/// The commit `dir` has checked out, or `None` outside a repository or before
/// its first commit.
///
/// The diff base for a session in a tree amx did not cut.
pub fn head_commit(dir: &Path) -> Result<Option<String>> {
    match git(dir, &["rev-parse", "--verify", "HEAD^{commit}"]) {
        Ok(commit) if !commit.is_empty() => Ok(Some(commit)),
        _ => Ok(None),
    }
}

/// The merge base of `dir`'s HEAD and the repository's main line.
///
/// The fallback diff base for a tree amx did not cut when the record has none,
/// such as an adopted agent. It includes commits the branch already carried.
/// `None` outside a repository or when the histories share nothing.
pub fn fork_point(dir: &Path) -> Result<Option<String>> {
    let Some(repo) = repo_root(dir)? else {
        return Ok(None);
    };
    let main = main_branch(&repo);
    match git(dir, &["merge-base", "HEAD", &main]) {
        Ok(shared) if !shared.is_empty() => Ok(Some(shared)),
        _ => Ok(None),
    }
}

/// The main repository a worktree belongs to.
///
/// [`repo_root`] answers with the linked tree itself; branches are deleted
/// and trees removed from the repository they share.
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

/// The repository of a tree amx cut, read off the path alone.
///
/// Works after the tree's directory is gone, when [`main_repo`] has nowhere
/// to ask git from.
pub fn repo_of(worktree: &Path) -> Option<PathBuf> {
    // <repo>/.amx/worktrees/<id>
    is_amx_tree(worktree).then(|| worktree.ancestors().nth(3).map(Path::to_path_buf))?
}

/// The repository a linked worktree belongs to, read from the tree's `.git`
/// file instead of asking git.
///
/// The view resolves every agent's project on every reading and cannot afford
/// a subprocess per row. git writes a linked tree's `.git` as
/// `gitdir: <repo>/.git/worktrees/<name>`. Only an absolute `gitdir` is read;
/// `worktree.useRelativePaths` makes it relative, and that yields `None`.
pub fn repo_of_linked(dir: &Path) -> Option<PathBuf> {
    let pointer = dir.join(".git");
    if !pointer.is_file() {
        return None;
    }
    let said = std::fs::read_to_string(&pointer).ok()?;
    let gitdir = Path::new(said.trim().strip_prefix("gitdir:")?.trim());
    // `<repo>/.git/worktrees/<name>`, walked back up.
    let holds = gitdir.parent()?;
    let git_dir = holds.parent()?;
    (gitdir.is_absolute()
        && holds.file_name()? == WORKTREES_IN_GIT
        && git_dir.file_name()? == ".git")
        .then(|| git_dir.parent())
        .flatten()
        .map(Path::to_path_buf)
}

/// The directory under `.git` where git records linked worktrees.
const WORKTREES_IN_GIT: &str = "worktrees";

/// The branch amx creates for an agent's tree.
pub fn branch_for(id: &str) -> String {
    format!("amx/{id}")
}

/// Whether `branch` is a name amx chose for agent `id`: `amx/<id>`, or the
/// `pr-<N>` used when a request's head name is taken.
///
/// Only these may be deleted by a cleanup nobody confirmed branch by branch.
/// Any other name is the person's.
pub fn named_by_amx(id: &str, branch: &str) -> bool {
    branch == branch_for(id)
        || branch
            .strip_prefix("pr-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Where an agent's tree goes.
pub fn path_for(repo: &Path, id: &str) -> PathBuf {
    repo.join(WORKTREES).join(id)
}

/// Whether `path` is an absolute `<repo>/.amx/worktrees/<id>` with a valid id.
///
/// amx acts on behalf of the trees it cut and nothing else.
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

/// Whether `dir` is a linked worktree, whoever cut it and wherever it is.
///
/// Asked of git, since other tools (`workflow run`) use their own layouts. A
/// linked tree's git dir differs from the common dir; in the main checkout
/// they are the same.
pub fn is_linked(dir: &Path) -> bool {
    let Ok(both) = git(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    ) else {
        return false;
    };
    let mut lines = both.lines();
    matches!((lines.next(), lines.next()), (Some(own), Some(shared)) if own != shared)
}

/// Cut a tree for `id` from the commit `from` names, or from HEAD.
///
/// The ref is resolved first, so an unknown name fails before anything is
/// created.
pub fn create(repo: &Path, id: &str, from: Option<&str>) -> Result<Worktree> {
    let base = match from {
        Some(named) => commit_of(repo, named)?,
        None => git(repo, &["rev-parse", "HEAD"])
            .context("this repository has no commits yet, so amx cannot create a worktree")?,
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

/// Cut a tree for `id` on `branch`, fetched from the origin ref `fetch`.
///
/// Used for pull requests, whose head may not exist locally. The caller picks
/// `branch` because the head's own name can be taken. The refspec has no `+`,
/// so an existing local branch only fast-forwards; one with commits the
/// request lacks is refused. The base is read back off the fetched branch.
pub fn create_on(repo: &Path, id: &str, branch: &str, fetch: &str) -> Result<Worktree> {
    ensure_excluded(repo)?;
    if let Err(e) = git(
        repo,
        &["fetch", "origin", &format!("{fetch}:refs/heads/{branch}")],
    ) {
        if format!("{e:#}").contains("non-fast-forward") {
            bail!("{branch} has commits that {fetch} does not; amx left the branch unchanged");
        }
        return Err(e);
    }

    let path = path_for(repo, id);
    git(repo, &["worktree", "add", &path.to_string_lossy(), branch])?;
    let base = commit_of(repo, branch)?;

    Ok(Worktree {
        path,
        branch: branch.to_string(),
        base,
    })
}

/// Cut a tree for `id` on an existing local `branch`.
///
/// No fetch, so local commits stay where they are. The base is the branch's
/// current commit, so `diff` shows only what the agent adds.
pub fn create_on_local(repo: &Path, id: &str, branch: &str) -> Result<Worktree> {
    ensure_excluded(repo)?;

    let path = path_for(repo, id);
    git(repo, &["worktree", "add", &path.to_string_lossy(), branch])?;
    let base = commit_of(repo, branch)?;

    Ok(Worktree {
        path,
        branch: branch.to_string(),
        base,
    })
}

/// Whether git records `branch`'s upstream as deleted.
///
/// The one signal that catches a squash merge, where [`is_merged`] says no.
/// `%(upstream:track)` reads the remote-tracking ref, which only changes on a
/// fetch (see [`prune_origin`]). No upstream, or no such branch, is `false`.
pub fn upstream_gone(repo: &Path, branch: &str) -> Result<bool> {
    let track = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(upstream:track)",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    Ok(track.trim() == "[gone]")
}

/// Fetch from the origin, pruning branches it no longer has.
///
/// A repository with no origin is left alone and is not an error.
pub fn prune_origin(repo: &Path) -> Result<()> {
    if git(repo, &["remote", "get-url", "origin"]).is_err() {
        return Ok(());
    }
    git(repo, &["fetch", "--prune", "--quiet", "origin"])?;
    Ok(())
}

/// The local branch names of the repository at `dir`.
///
/// Local refs only: listing the origin's would mean a network call.
pub fn local_branches(dir: &Path) -> Result<Vec<String>> {
    let listed = git(
        dir,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    Ok(listed.lines().map(str::to_string).collect())
}

/// Whether some tree of this repository has `branch` checked out.
///
/// git allows one tree per branch, so a taken name means cutting under
/// another.
pub fn checked_out(repo: &Path, branch: &str) -> Result<bool> {
    let listed = git(repo, &["worktree", "list", "--porcelain"])?;
    let named = format!("branch refs/heads/{branch}");
    Ok(listed.lines().any(|line| line.trim_end() == named))
}

/// Whether every commit on `branch` is in the repository's main line.
///
/// A branch git does not have is `false`.
pub fn is_merged(repo: &Path, branch: &str) -> Result<bool> {
    let main = main_branch(repo);
    Ok(!git(repo, &["branch", "--merged", &main, "--list", branch])?.is_empty())
}

/// The repository's main line: `origin/HEAD` when set, else `main` if it
/// exists, else `master`.
///
/// Public so a sweep can name the branch in its reason ("merged into main").
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

/// The branch `repo` has checked out, for a repository heading in the view.
///
/// `None` on a detached HEAD, mid-rebase, or outside a repository.
pub fn branch_at(repo: &Path) -> Option<String> {
    git(repo, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|branch| !branch.is_empty())
}

/// The commit any ref names: branch, tag, remote-tracking name or hash.
///
/// `^{commit}` peels a tag to its commit, and `--verify` makes an unknown name
/// an error. The error names what was typed; git's own message does not help.
fn commit_of(repo: &Path, named: &str) -> Result<String> {
    git(
        repo,
        &["rev-parse", "--verify", &format!("{named}^{{commit}}")],
    )
    .map_err(|_| anyhow!("{named} is not a commit, branch or tag"))
}

/// The tree a setup command runs in.
pub const WORKTREE_ENV: &str = "AMX_WORKTREE";

/// The repository that tree was cut from.
pub const REPO_ENV: &str = "AMX_REPO";

/// Furnish a freshly cut tree: copy the `copy` files from the repository,
/// symlink the `link` directories to the repository's own, then run each
/// `setup` command in the tree.
///
/// Returns notes for entries that were skipped: a path the repository does
/// not have, or a copy the tree already has a file for (kept, never
/// overwritten).
///
/// # Errors
///
/// An entry that is absolute or climbs out through `..` is refused before
/// anything is copied. A failing setup command is an error carrying its
/// stderr, and later commands do not run.
///
/// `env` supplies the agent's own variables; [`WORKTREE_ENV`] and
/// [`REPO_ENV`] are set from the arguments.
pub fn furnish(
    repo: &Path,
    tree: &Path,
    copy: &[String],
    link: &[String],
    setup: &[String],
    env: &[(String, String)],
) -> Result<Vec<String>> {
    let mut missing = Vec::new();

    // Validate every entry first, so a refusal leaves the tree untouched.
    for path in copy.iter().chain(link) {
        let inside = Path::new(path).components().all(|part| {
            matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        if !inside {
            bail!("`{path}` is outside the repository: copy and link take paths inside it");
        }
    }

    for path in copy {
        let from = repo.join(path);
        if !from.exists() {
            missing.push(format!("{path} is not in {}", repo.display()));
            continue;
        }
        // A tracked file, or a tracked symlink a copy would write through.
        let to = tree.join(path);
        if to.symlink_metadata().is_ok() {
            missing.push(format!("kept {path}: the worktree already has it"));
            continue;
        }
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

/// Create the parent directory of a copy or link target.
fn make_way_for(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))
}

/// Run one setup command in the tree through `sh -c`.
///
/// stdout is discarded (`new` prints only the id); stderr is kept for the
/// error message.
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
            "" => bail!("setup {command:?} failed with no error output"),
            said => bail!("setup {command:?}: {said}"),
        }
    }
    Ok(())
}

/// Whether `dir` has uncommitted work to move into a tree: staged, unstaged or
/// untracked, excluding what `.gitignore` names.
///
/// Outside a repository there is nothing to move.
pub fn has_changes_to_carry(dir: &Path) -> Result<bool> {
    if repo_root(dir)?.is_none() {
        return Ok(false);
    }
    Ok(!git(dir, &["status", "--porcelain"])?.is_empty())
}

/// Move uncommitted work from `from` into `tree`, returning the stash commit
/// that carried it, or `None` when there was nothing to move.
///
/// Untracked files move too; ignored files stay. The work is applied to the
/// tree before it is removed from `from`, so it is never in neither place. If
/// the apply fails, `from` keeps every file, unstaged. The returned commit is
/// what [`give_back`] uses to undo the move if the spawn fails later.
pub fn carry_changes(from: &Path, tree: &Path) -> Result<Option<String>> {
    // `stash create` only records the index and tracked files, so untracked
    // files are staged first. `add -A` also honours `.gitignore`.
    git(from, &["add", "-A"])?;

    // `stash create` writes a commit without touching the stash stack, and
    // prints nothing when there is nothing to stash. The tree reads the commit
    // from the object store it shares with `from`.
    let stashed = match git(from, &["stash", "create"]) {
        Ok(stashed) => stashed,
        Err(e) => {
            let _ = git(from, &["reset", "-q"]);
            return Err(e);
        }
    };
    if stashed.is_empty() {
        // Undo the staging above; `from` is the person's working directory.
        git(from, &["reset", "-q"])?;
        return Ok(None);
    }
    if let Err(e) = git(tree, &["stash", "apply", &stashed]) {
        git(from, &["reset", "-q"])?;
        return Err(e).with_context(|| {
            format!(
                "moving the uncommitted changes in {} into {}",
                from.display(),
                tree.display()
            )
        });
    }
    // The apply stages new files; leave the work unstaged, as it was.
    git(tree, &["reset", "-q"])?;
    // `--hard` because the new files are only in the index and a plain reset
    // would leave them on disk.
    git(from, &["reset", "--hard", "HEAD"])?;
    Ok(Some(stashed))
}

/// Put work moved by [`carry_changes`] back into `from`, unstaged.
pub fn give_back(from: &Path, stash: &str) -> Result<()> {
    git(from, &["stash", "apply", stash])
        .with_context(|| format!("restoring the uncommitted changes in {}", from.display()))?;
    git(from, &["reset", "-q"])?;
    Ok(())
}

/// Whether the tree has uncommitted changes, untracked files included.
///
/// Runs with [`nothing_to_run`], since `status` refreshes the index, which can
/// run a hook and clean filters.
pub fn is_dirty(worktree: &Path) -> Result<bool> {
    let safe = nothing_to_run(worktree);
    Ok(!git_with(worktree, &safe, &["status", "--porcelain"])?.is_empty())
}

/// Remove the tree, refusing while it holds uncommitted work.
pub fn remove(repo: &Path, worktree: &Path) -> Result<()> {
    if !worktree.exists() {
        // Already deleted: only git's record of it is left.
        git(repo, &["worktree", "prune"])?;
        return Ok(());
    }

    if is_dirty(worktree)? {
        bail!("{} has uncommitted changes", worktree.display());
    }
    git(repo, &["worktree", "remove", &worktree.to_string_lossy()])?;
    Ok(())
}

/// Force-remove a tree nobody has worked in, and its branch if one is named.
///
/// The undo for a spawn whose [`furnish`] failed. The caller passes a branch
/// only when [`named_by_amx`] allows it, and [`delete_branch`] still refuses
/// one with unshared commits.
pub fn discard(repo: &Path, worktree: &Path, branch: Option<&str>) -> Result<()> {
    git(
        repo,
        &["worktree", "remove", "--force", &worktree.to_string_lossy()],
    )?;
    match branch {
        Some(branch) => delete_branch(repo, branch, &[]),
        None => Ok(()),
    }
}

/// Recreate a removed tree on its existing branch, for a resumed agent.
///
/// The branch is never created: if it is gone, that is an error.
pub fn restore(repo: &Path, worktree: &Path, branch: &str) -> Result<()> {
    // git refuses to add a tree it still has a record of.
    git(repo, &["worktree", "prune"])?;
    git(
        repo,
        &["worktree", "add", &worktree.to_string_lossy(), branch],
    )?;
    Ok(())
}

/// Delete a branch with `-D`, refusing when that would lose commits.
///
/// See [`loses`] for what counts as lost.
pub fn delete_branch(repo: &Path, branch: &str, merged_heads: &[String]) -> Result<()> {
    match loses(repo, branch, merged_heads)? {
        0 => {}
        1 => bail!("1 commit is not on any other branch"),
        n => bail!("{n} commits are not on any other branch"),
    }
    git(repo, &["branch", "-D", branch])?;
    Ok(())
}

/// How many commits deleting `branch` would lose.
///
/// Zero when the tip is one of `merged_heads`, the heads the forge reports as
/// merged: a squash or rebase merge leaves the branch's commits on no other
/// branch without losing the work. Otherwise [`unshared_commits`].
pub fn loses(repo: &Path, branch: &str, merged_heads: &[String]) -> Result<usize> {
    let tip = git(repo, &["rev-parse", &format!("refs/heads/{branch}")])?;
    if merged_heads.iter().any(|head| head == tip.trim()) {
        return Ok(0);
    }
    unshared_commits(repo, branch)
}

/// How many commits on `branch` no other local or remote branch reaches.
///
/// A branch git does not have is an error.
pub fn unshared_commits(repo: &Path, branch: &str) -> Result<usize> {
    let exclude = format!("--exclude={branch}");
    let count = git(
        repo,
        &[
            "rev-list",
            "--count",
            &format!("refs/heads/{branch}"),
            "--not",
            &exclude,
            "--branches",
            "--remotes",
        ],
    )?;
    count
        .trim()
        .parse()
        .with_context(|| format!("counting the commits on {branch}"))
}

/// Write the agent's work since its base to `out`, or a `--stat` summary with
/// `stat`.
///
/// Measured from [`work_began_at`], so a rebased tree still shows only the
/// agent's work. `add -N` first records intent-to-add for untracked files so
/// the diff includes them; it stages nothing else. Both git calls run with
/// [`nothing_to_run`].
pub fn diff(worktree: &Path, base: &str, stat: bool, out: &mut impl Write) -> Result<()> {
    let safe = nothing_to_run(worktree);
    git_with(worktree, &safe, &["add", "-N", "."])?;
    let from = work_began_at(worktree, base, &safe);

    // The flag forms of the diff.external and textconv overrides.
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv"];
    if stat {
        args.push("--stat");
    }
    args.push(&from);

    // Streamed, since a patch can be large.
    let mut child = command(worktree, &safe, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `git diff`")?;
    let mut printed = child.stdout.take().expect("stdout was asked for");
    let copied = std::io::copy(&mut printed, out);
    // Wait even when the reader stopped early (a viewer quit): closing the
    // pipe ends git, and an unwaited child stays a zombie in the view.
    drop(printed);
    let finished = child.wait_with_output().context("waiting for `git diff`")?;
    copied.context("reading the diff")?;
    if !finished.status.success() {
        bail!(
            "git diff {from}: {}",
            String::from_utf8_lossy(&finished.stderr).trim()
        );
    }
    Ok(())
}

/// The merge base of `base` and the tree's HEAD, where the agent's work
/// starts.
///
/// Usually `base` itself. After a rebase onto another line, diffing against
/// the recorded base would show the base's own changes reversed. Computed on
/// each call because the tree's history moves. With no shared history, `base`
/// is used as recorded and git reports the error.
fn work_began_at(worktree: &Path, base: &str, safe: &[String]) -> String {
    match git_with(worktree, safe, &["merge-base", base, "HEAD"]) {
        Ok(shared) if !shared.is_empty() => shared,
        _ => base.to_string(),
    }
}

/// Add [`EXCLUDE_LINE`] to the repository's `info/exclude` once.
fn ensure_excluded(repo: &Path) -> Result<()> {
    // The common dir: in a linked tree `.git` is a file, and the exclude file
    // belongs to the shared repository.
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

/// `-c` overrides that stop git from running programs named by the tree's
/// config while amx reads it.
///
/// The agent can write the tree's config, so hooks, clean/smudge/process
/// filters and (via flags in [`diff`]) external diff and textconv are blanked.
/// `-c` beats every config file. Filter drivers have no wildcard and are
/// blanked one at a time, with `required=false` so a blanked required filter
/// is not a fatal error. The cost is that a filtered file the agent touched is
/// compared as stored rather than as filtered.
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

/// The filter drivers the repository's config declares.
///
/// `--get-regexp` exits non-zero when nothing matches, which reads as none.
fn filter_drivers(dir: &Path) -> Vec<String> {
    let listed = git(
        dir,
        &["config", "--name-only", "--get-regexp", r"^filter\."],
    )
    .unwrap_or_default();
    drivers_in(&listed)
}

/// The driver names in `filter.<driver>.<key>` lines, sorted and deduplicated.
///
/// A driver name may contain dots (`git-lfs.2`), so the key is split off the
/// right.
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

/// Run git in `dir` and return its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_with(dir, &[], args)
}

/// [`git`] with config overrides before the subcommand.
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

/// A git command in `dir`, hardened for every call.
///
/// `core.fsmonitor` is disabled everywhere: it names a program git starts
/// before reading any file, and amx never needs the cache.
fn command(dir: &Path, overrides: &[String], args: &[&str]) -> Command {
    let mut git = Command::new("git");
    git.current_dir(dir)
        .args(["-c", "core.fsmonitor=false"])
        .args(overrides)
        .args(args)
        .stdin(Stdio::null())
        // /etc/gitconfig can name programs under keys the overrides miss.
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Fail instead of prompting for credentials.
        .env("GIT_TERMINAL_PROMPT", "0");
    git
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    /// Run git for test setup, isolated from the developer's config and with
    /// a fixed identity.
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

    /// A bare repository added as `repo`'s `origin`, standing in for the forge.
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

    /// Commit a `.gitignore` that ignores `/build/`.
    fn an_ignore(repo: &Path) {
        std::fs::write(repo.join(".gitignore"), "/build/\n").unwrap();
        setup(repo, &["add", ".gitignore"]);
        setup(repo, &["commit", "-m", "ignore the build"]);
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
    fn worktree_names_the_commit_a_directory_is_standing_on() {
        let repo = a_repo();
        let head = setup(repo.path(), &["rev-parse", "HEAD"]);
        assert_eq!(head_commit(repo.path()).unwrap(), Some(head));

        // Outside a repository there is no commit.
        let plain = TempDir::new().unwrap();
        assert_eq!(head_commit(plain.path()).unwrap(), None);
    }

    #[test]
    fn worktree_finds_where_a_branch_left_the_main_line() {
        let repo = a_repo();
        let fork = setup(repo.path(), &["rev-parse", "HEAD"]);
        // On the main line the fork point is HEAD.
        assert_eq!(fork_point(repo.path()).unwrap(), Some(fork.clone()));

        // On a branch it is where the branch left main.
        setup(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("README.md"), "after\n").unwrap();
        setup(repo.path(), &["commit", "-am", "second"]);
        assert_eq!(fork_point(repo.path()).unwrap(), Some(fork));

        let plain = TempDir::new().unwrap();
        assert_eq!(fork_point(plain.path()).unwrap(), None);
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
        // A branch, a tag and an abbreviated hash naming one commit, with HEAD
        // moved past it.
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
        // A pull request head exists only as a ref in the origin, so the branch
        // is fetched before the tree is cut on it.
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
    fn worktree_is_cut_on_a_branch_this_checkout_already_has() {
        // No origin needed: the tree is cut on the existing branch instead of a
        // new `amx/<id>`.
        let repo = a_repo();
        setup(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("login.rs"), "fn login() {}\n").unwrap();
        setup(repo.path(), &["add", "login.rs"]);
        setup(repo.path(), &["commit", "-m", "the work already on it"]);
        let head = setup(repo.path(), &["rev-parse", "HEAD"]);
        setup(repo.path(), &["checkout", "-q", "main"]);

        let tree = create_on_local(repo.path(), "fix-login-a1b", "feature").unwrap();

        assert_eq!(tree.branch, "feature");
        assert_eq!(tree.base, head, "recorded at the commit the branch is at");
        assert_eq!(tree.path, repo.path().join(".amx/worktrees/fix-login-a1b"));
        assert_eq!(
            setup(&tree.path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature",
            "and the tree is on the branch rather than on a detached head"
        );
        assert_eq!(
            std::fs::read_to_string(tree.path.join("login.rs")).unwrap(),
            "fn login() {}\n",
            "so the agent opens the work that is already there"
        );
        assert_eq!(
            setup(repo.path(), &["status", "--porcelain"]),
            "",
            "and this tree is kept out of the status like any other"
        );
    }

    #[test]
    fn worktree_says_a_branchs_upstream_has_gone_once_a_fetch_has_pruned_it() {
        // After a squash merge the forge deletes the branch, and the checkout
        // only learns of it on a fetch.
        let repo = a_repo();
        let origin = an_origin(repo.path());
        setup(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("login.rs"), "fn login() {}\n").unwrap();
        setup(repo.path(), &["add", "login.rs"]);
        setup(repo.path(), &["commit", "-m", "the agent's own commit"]);

        assert!(
            !upstream_gone(repo.path(), "feature").unwrap(),
            "a branch that was never pushed has no upstream to lose"
        );
        setup(repo.path(), &["push", "-q", "-u", "origin", "feature"]);
        assert!(
            !upstream_gone(repo.path(), "feature").unwrap(),
            "and one the origin holds is there"
        );

        setup(origin.path(), &["branch", "-D", "feature"]);
        assert!(
            !upstream_gone(repo.path(), "feature").unwrap(),
            "the delete is on the forge and this checkout has not heard of it"
        );
        prune_origin(repo.path()).unwrap();
        assert!(
            upstream_gone(repo.path(), "feature").unwrap(),
            "the fetch is what makes it a fact git records"
        );
        assert!(
            !upstream_gone(repo.path(), "amx/never-cut-b2c").unwrap(),
            "and a branch git does not have has no upstream either"
        );
    }

    #[test]
    fn worktree_fetches_nothing_and_says_nothing_where_there_is_no_origin() {
        // With no remote there is nothing to fetch, and that is no error.
        let repo = a_repo();
        prune_origin(repo.path()).unwrap();
        assert!(!upstream_gone(repo.path(), "main").unwrap());
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
        // git allows one tree per branch.
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
        // origin/HEAD wins; otherwise a `trunk` default would make every branch
        // read as unmerged.
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

        // Without origin/HEAD, whichever of main and master exists.
        let old = a_repo();
        setup(old.path(), &["branch", "-m", "master"]);
        assert_eq!(main_branch(old.path()), "master");
    }

    #[test]
    fn worktree_names_the_branch_a_directory_has_checked_out() {
        let repo = a_repo();
        assert_eq!(branch_at(repo.path()), Some("main".to_string()));

        // A detached HEAD names no branch.
        setup(repo.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(branch_at(repo.path()), None, "a detached HEAD is on none");

        let outside = TempDir::new().unwrap();
        assert_eq!(
            branch_at(outside.path()),
            None,
            "and neither is a directory"
        );
    }

    #[test]
    fn worktree_refuses_a_ref_that_is_no_commit_before_anything_is_made() {
        let repo = a_repo();

        let refused = create(repo.path(), "fix-login-a1b", Some("release")).unwrap_err();
        let said = format!("{refused:#}");
        assert!(said.contains("release"), "the ref that was typed: {said}");
        assert!(said.contains("not a commit"), "{said}");
        assert!(
            !repo.path().join(".amx").exists(),
            "and no tree was cut for it"
        );
        assert_eq!(
            setup(repo.path(), &["branch", "--list", "amx/fix-login-a1b"]),
            "",
            "nor a branch"
        );

        // A ref resolving to a non-commit object is refused too (`^{commit}`).
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

        // Only paths in amx's own layout are read.
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

        // A second tree does not add the line again.
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
        // The tree is cut from `second`, then its commit is rebased onto
        // `release`, which lacks `second`. Diffing against `second` itself would
        // show `second`'s changes reversed as the agent's.
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
        // A base the tree shares no history with fails with git's error.
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let mut out = Vec::new();
        let refused = diff(&tree.path, "0f1e2d3", false, &mut out).unwrap_err();
        assert!(format!("{refused:#}").contains("0f1e2d3"), "{refused:#}");
    }

    /// The git processes this thread started and has not waited for.
    #[cfg(target_os = "linux")]
    fn unwaited_gits() -> std::collections::BTreeSet<String> {
        std::fs::read_to_string("/proc/thread-self/children")
            .unwrap_or_default()
            .split_whitespace()
            .filter(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .is_ok_and(|comm| comm.trim() == "git")
            })
            .map(str::to_string)
            .collect()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worktree_diff_waits_for_git_when_the_reader_stops_early() {
        struct Quit;
        impl Write for Quit {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        let long: String = (0..20_000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(tree.path.join("long.txt"), long).unwrap();

        let before = unwaited_gits();
        assert!(diff(&tree.path, &tree.base, false, &mut Quit).is_err());
        let left: Vec<_> = unwaited_gits().difference(&before).cloned().collect();
        assert!(left.is_empty(), "git diff was never waited for: {left:?}");
    }

    #[test]
    fn diff_stat_answers_with_the_shape_of_the_work() {
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
    fn every_git_is_run_with_the_system_config_shut_out() {
        // /etc/gitconfig can name programs under keys the overrides miss, so
        // every git runs with GIT_CONFIG_NOSYSTEM.
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
    fn a_diff_runs_nothing_the_tree_it_reads_names() {
        // Each key names a program and can be set from inside the tree. The
        // attributes go in `.git/info/attributes`, which config cannot redirect.
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
    fn a_filter_driver_is_whatever_lies_between_the_two_ends() {
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
        // Entries the repository lacks are reported and skipped.
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
    fn furnish_refuses_a_path_outside_the_repository_and_copies_nothing() {
        let repo = a_repo();
        std::fs::write(repo.path().join(".env"), "TOKEN=hunter2\n").unwrap();
        let outside = repo.path().parent().unwrap().join("secret");
        std::fs::write(&outside, "the key\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        for entry in [
            "../secret".to_string(),
            outside.to_string_lossy().into_owned(),
            "config/../../secret".to_string(),
        ] {
            for (copy, link) in [
                (vec![".env".to_string(), entry.clone()], vec![]),
                (vec![".env".to_string()], vec![entry.clone()]),
            ] {
                let refused = furnish(repo.path(), &tree.path, &copy, &link, &[], &[]).unwrap_err();
                assert!(format!("{refused:#}").contains(&entry), "{refused:#}");
                assert!(
                    !tree.path.join(".env").exists(),
                    "nothing is copied once one entry is refused"
                );
            }
        }
        assert!(!repo.path().join(".amx/secret").exists());
    }

    #[test]
    fn furnish_never_copies_over_a_file_the_tree_already_has() {
        // A copy onto a tracked symlink would write through to its target, and
        // one onto a tracked file would overwrite it.
        let repo = a_repo();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("settings.toml");
        std::fs::write(&target, "the real settings\n").unwrap();
        std::os::unix::fs::symlink(&target, repo.path().join("settings.toml")).unwrap();
        setup(repo.path(), &["add", "settings.toml"]);
        setup(repo.path(), &["commit", "-m", "a tracked link"]);
        std::fs::write(repo.path().join("README.md"), "changed here\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        let said = furnish(
            repo.path(),
            &tree.path,
            &["settings.toml".to_string(), "README.md".to_string()],
            &[],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "the real settings\n",
            "the file the link points at keeps its bytes"
        );
        assert_eq!(
            std::fs::read_to_string(tree.path.join("README.md")).unwrap(),
            "before\n",
            "the tree's own file is kept"
        );
        assert!(
            said.iter()
                .any(|s| s == "kept settings.toml: the worktree already has it"),
            "{said:?}"
        );
        assert!(
            said.iter()
                .any(|s| s == "kept README.md: the worktree already has it"),
            "{said:?}"
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
        discard(repo.path(), &tree.path, Some(&tree.branch)).unwrap();

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
    fn worktree_carries_the_work_into_the_tree_and_leaves_what_git_ignores() {
        let repo = a_repo();
        an_ignore(repo.path());
        std::fs::write(repo.path().join("login.rs"), "fn login() {}\n").unwrap();
        setup(repo.path(), &["add", "login.rs"]);
        std::fs::write(repo.path().join("README.md"), "after\n").unwrap();
        std::fs::write(repo.path().join("notes.txt"), "scratch\n").unwrap();
        std::fs::create_dir(repo.path().join("build")).unwrap();
        std::fs::write(repo.path().join("build/out"), "compiled\n").unwrap();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();

        assert!(has_changes_to_carry(repo.path()).unwrap());
        assert!(carry_changes(repo.path(), &tree.path).unwrap().is_some());

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
            std::fs::read_to_string(tree.path.join("notes.txt")).unwrap(),
            "scratch\n",
            "and the new file no commit has ever held"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
            "before\n",
            "the directory it was typed in is left as the last commit had it"
        );
        assert!(
            !repo.path().join("login.rs").exists() && !repo.path().join("notes.txt").exists(),
            "the work moved rather than being copied"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("build/out")).unwrap(),
            "compiled\n",
            "a file .gitignore names stays where it was made"
        );
        assert!(!tree.path.join("build").exists());
        assert!(
            !has_changes_to_carry(repo.path()).unwrap(),
            "and nothing more to move"
        );
    }

    #[test]
    fn worktree_carries_nothing_where_no_commit_is_missing_any_of_it() {
        let repo = a_repo();
        an_ignore(repo.path());
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        std::fs::create_dir(repo.path().join("build")).unwrap();
        std::fs::write(repo.path().join("build/out"), "compiled\n").unwrap();

        // Ignored files alone are nothing to move.
        assert!(!has_changes_to_carry(repo.path()).unwrap());
        assert!(carry_changes(repo.path(), &tree.path).unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("build/out")).unwrap(),
            "compiled\n",
            "and it was left alone"
        );
        assert_eq!(
            setup(repo.path(), &["status", "--porcelain"]),
            "",
            "with nothing staged behind it either"
        );

        let elsewhere = TempDir::new().unwrap();
        assert!(
            !has_changes_to_carry(elsewhere.path()).unwrap(),
            "somewhere that is not a repository has nothing to move either"
        );
    }

    #[test]
    fn worktree_carrying_work_that_will_not_apply_leaves_the_directory_as_it_was() {
        // The tree is cut from an older commit, so the stash does not apply and
        // the work stays in the original directory.
        let repo = a_repo();
        std::fs::write(repo.path().join("README.md"), "second\n").unwrap();
        setup(repo.path(), &["commit", "-am", "second"]);
        let tree = create(repo.path(), "fix-login-a1b", Some("HEAD~1")).unwrap();
        std::fs::write(repo.path().join("README.md"), "third\n").unwrap();

        let refused = carry_changes(repo.path(), &tree.path).unwrap_err();
        assert!(
            format!("{refused:#}").contains("moving the uncommitted changes"),
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

        // The branch outlives the tree.
        let branches = setup(repo.path(), &["branch", "--list", &tree.branch]);
        assert!(branches.contains(&tree.branch), "{branches}");

        // It cannot be deleted while its commit is on no other branch.
        let refused = delete_branch(repo.path(), &tree.branch, &[]).unwrap_err();
        assert_eq!(
            format!("{refused:#}"),
            "1 commit is not on any other branch"
        );
        assert!(setup(repo.path(), &["branch", "--list", &tree.branch]).contains(&tree.branch));

        // Once main has the commit, deleting the branch loses nothing.
        setup(repo.path(), &["merge", "--ff-only", &tree.branch]);
        delete_branch(repo.path(), &tree.branch, &[]).unwrap();
        assert_eq!(setup(repo.path(), &["branch", "--list", &tree.branch]), "");
    }

    #[test]
    fn unshared_commits_are_the_ones_no_other_branch_or_remote_has() {
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        assert_eq!(unshared_commits(repo.path(), &tree.branch).unwrap(), 0);

        for n in 1..=2 {
            std::fs::write(tree.path.join("login.rs"), format!("{n}\n")).unwrap();
            setup(&tree.path, &["add", "login.rs"]);
            setup(&tree.path, &["commit", "-m", "the agent's own"]);
        }
        assert_eq!(unshared_commits(repo.path(), &tree.branch).unwrap(), 2);

        // Another branch holding the commits is enough.
        setup(repo.path(), &["branch", "keep-it", &tree.branch]);
        assert_eq!(unshared_commits(repo.path(), &tree.branch).unwrap(), 0);
    }

    #[test]
    fn unshared_a_branch_at_the_head_a_forge_merged_loses_nothing() {
        // Squash merge: main holds the work under its own commit, so the
        // branch's commit is on no other branch.
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        std::fs::write(tree.path.join("login.rs"), "fixed\n").unwrap();
        setup(&tree.path, &["add", "login.rs"]);
        setup(&tree.path, &["commit", "-m", "the agent's own"]);
        let merged = setup(&tree.path, &["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(loses(repo.path(), &tree.branch, &[]).unwrap(), 1);
        assert_eq!(
            loses(repo.path(), &tree.branch, std::slice::from_ref(&merged)).unwrap(),
            0
        );

        // Commits after the merged head count as lost.
        for n in 1..=2 {
            std::fs::write(tree.path.join("login.rs"), format!("{n}\n")).unwrap();
            setup(&tree.path, &["add", "login.rs"]);
            setup(&tree.path, &["commit", "-m", "after the merge"]);
        }
        assert_eq!(loses(repo.path(), &tree.branch, &[merged]).unwrap(), 3);
    }

    #[test]
    fn unshared_a_fresh_tree_cut_from_unpushed_work_holds_nothing_of_its_own() {
        // Unpushed commits the tree was cut on belong to the other branch.
        let repo = a_repo();
        setup(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("wip.rs"), "wip\n").unwrap();
        setup(repo.path(), &["add", "wip.rs"]);
        setup(repo.path(), &["commit", "-m", "the person's unpushed work"]);
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        assert_eq!(unshared_commits(repo.path(), &tree.branch).unwrap(), 0);
    }

    #[test]
    fn unshared_only_amx_names_are_amx_s_to_delete() {
        assert!(named_by_amx("fix-login-a1b", "amx/fix-login-a1b"));
        assert!(named_by_amx("fix-login-a1b", "pr-12"));
        assert!(!named_by_amx("fix-login-a1b", "amx/another-b2c"));
        assert!(!named_by_amx("fix-login-a1b", "feature"));
        assert!(!named_by_amx("fix-login-a1b", "pr-"));
        assert!(!named_by_amx("fix-login-a1b", "pr-12-fix"));
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

    #[test]
    fn worktree_tells_a_linked_tree_from_the_checkout_it_belongs_to() {
        // A tree cut elsewhere, in another layout, is linked too.
        let repo = a_repo();
        let tree = create(repo.path(), "fix-login-a1b", None).unwrap();
        assert!(is_linked(&tree.path), "{}", tree.path.display());

        let elsewhere = TempDir::new().unwrap();
        let theirs = elsewhere.path().join("plan/t1");
        setup(
            repo.path(),
            &[
                "worktree",
                "add",
                "-q",
                &theirs.to_string_lossy(),
                "-b",
                "t1",
            ],
        );
        assert!(is_linked(&theirs), "somebody else's layout, still a tree");

        assert!(!is_linked(repo.path()), "the checkout the trees belong to");
        let plain = TempDir::new().unwrap();
        assert!(!is_linked(plain.path()), "a directory in no repository");
    }
}
