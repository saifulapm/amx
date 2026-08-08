//! `amx work <branch>` / `amx work done` — git worktrees (D-M3-10).
//!
//! Three existing capabilities composed, not a new server concept: `git worktree
//! add` as argv — never a shell string, [`crate::git`] is where that rule lives —
//! `workspace.create` with the tree as cwd, and `agent.start` when `--kind` is
//! given. The one thing the server remembers is the membership block on the
//! workspace, because `done` resolves a workspace by branch and restore has to
//! notice a tree that is no longer there.
//!
//! # Where the tree goes
//!
//! The `work.dir` template, default `{repo_parent}/{repo_name}--{branch}`: a
//! *sibling* of the repository rather than a directory inside it, so repo-
//! internal tooling — a test that walks the tree, a linter with a glob, a build
//! that hashes every file — never trips over a second checkout of the same
//! project. [`derive_path`] is the whole of the substitution.
//!
//! # The destructive path
//!
//! Removing a worktree is the most dangerous thing any M3 verb does, and it is
//! the one place amx runs a command that deletes a directory somebody may have
//! been typing in a minute ago. Four locks, in the order they are checked, all
//! *before* anything is killed or removed:
//!
//! 1. The path is never taken from the user. It comes out of the workspace's
//!    recorded membership, and it has to equal the path [`derive_path`] computes
//!    today from the same repository and branch. A template that has been edited
//!    since the tree was made, a membership from another machine, a snapshot
//!    somebody hand-edited: each of those disagrees, and each is refused with
//!    both paths named.
//! 2. The repository has to agree the path is one of its worktrees, which is
//!    `git worktree list` and not a guess about directory layout.
//! 3. The path may not be the repository's own main working tree — `git worktree
//!    remove` refuses that too, but a check whose failure message says "that is
//!    the repository itself" is worth having ahead of one that says "the main
//!    worktree cannot be removed".
//! 4. A tree with modified or untracked files is refused unless `--force`. Asked
//!    here rather than left to git so that the refusal lands before the workspace
//!    is killed: a `done` that took the panes and then declined to take the tree
//!    would have destroyed the half the user could not get back.
//!
//! Only then: kill the workspace through the existing verb, then remove the tree.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use amx_client::net::{self, Session};
use amx_core::agent::AgentKind;
use amx_core::{Config, Ctx, Env, WorkspaceId, Worktree};
use amx_proto::control::session::{StateParams, StateReply, WorkspaceState};
use amx_proto::control::{Method, agent, workspace};
use amx_server::session::probe::probe;
use anyhow::{Context as _, bail};
use clap::ArgMatches;
use serde::Serialize;
use serde_json::Value;

use crate::cmd::attach::client_info;
use crate::ctx_of;
use crate::git::{self, Repo};

/// Where a worktree goes when `work.dir` says nothing.
///
/// A sibling of the repository, named for the repository and the branch. The
/// double dash is a separator a branch name cannot contain a single of by
/// accident and git will not let one contain twice in a row (`a..b` is not a
/// branch name), so `repo--feature/x` reads back unambiguously.
pub const DEFAULT_DIR: &str = "{repo_parent}/{repo_name}--{branch}";

/// The clap id of the `branch` positional, on both `work` and `work done`.
const BRANCH: &str = "branch";

/// The clap id of `work --kind`.
const KIND: &str = "kind";

/// The clap id of `work done --force`.
const FORCE: &str = "force";

/// How long `amx work --kind` waits for the agent to report ready.
///
/// The server's own default is 30 s and applies when a caller names nothing;
/// this verb names it explicitly for one reason: `amx work` prints what it built
/// as it builds it, and a caller reading three lines about a worktree deserves
/// to know that the fourth is a bounded wait rather than a hang.
const READY_TIMEOUT_MS: u64 = 30_000;

/// Run `amx work`.
///
/// # Errors
///
/// If the branch name is not one git takes, if the caller is not inside a git
/// repository, if git refuses the add or the remove, if the session is not
/// running, or if the server refuses one of the calls.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    match sub.subcommand() {
        Some(("done", args)) => {
            done(
                env,
                root,
                args.get_one::<String>(BRANCH).map(String::as_str),
                args.get_flag(FORCE),
            )
            .await
        }
        Some((other, _)) => bail!("unknown work verb: {other}"),
        None => {
            let branch = sub
                .get_one::<String>(BRANCH)
                .context("amx work needs a branch, or the `done` verb")?;
            up(
                env,
                root,
                branch,
                sub.get_one::<String>(KIND).map(String::as_str),
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------- up

/// `amx work <branch>`: a worktree, a workspace on it, and optionally an agent.
///
/// The order is chosen so that each step's failure leaves the least behind. The
/// branch name and the repository are settled with no side effects at all; the
/// tree is added next, and is rolled back if the workspace cannot be created,
/// because a checkout nobody has a workspace on is litter the user did not ask
/// for and has nothing in it to lose. An agent that fails to start is *not* rolled
/// back — by then the user has the workspace they asked for, sitting on the tree
/// they asked for, and taking both away over a third thing would be amx deciding
/// their work was worthless.
async fn up(
    env: &Env,
    root: &ArgMatches,
    branch: &str,
    kind: Option<&str>,
) -> anyhow::Result<ExitCode> {
    git::check_branch_name(branch).await?;
    let kind = match kind {
        Some(kind) => Some(AgentKind::new(kind).context("invalid --kind")?),
        None => None,
    };

    let cwd = std::env::current_dir().context("read the current directory")?;
    let repo = Repo::discover(&cwd).await?;
    let ctx = ctx_of(env, root, None)?;
    let path = derive_path(&template(&ctx), repo.root(), branch)?;
    anyhow::ensure!(
        !path.exists(),
        "{} already exists; `amx work done {branch}` takes down the last one",
        path.display()
    );

    // Refused before the tree exists rather than after: a session that is not
    // running would otherwise cost a rollback for nothing.
    let mut session = connect(&ctx).await?;

    // git creates the leaf, never its ancestors: a template with a directory
    // component (`{repo_parent}/trees/{branch}`) needs the containing directory
    // to be there first.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let create = !repo.has_branch(branch).await?;
    repo.add_worktree(&path, branch, create).await?;
    println!(
        "worktree {} ({} branch {branch})",
        path.display(),
        if create { "new" } else { "existing" }
    );

    let worktree = Worktree {
        repo: repo.root().to_path_buf(),
        branch: branch.to_owned(),
        path: path.clone(),
    };
    let created: workspace::CreateReply = match call(
        &mut session,
        Method::WorkspaceCreate,
        &workspace::CreateParams {
            label: Some(branch.to_owned()),
            focus: true,
            worktree: Some(worktree),
        },
    )
    .await
    {
        Ok(reply) => reply,
        Err(err) => {
            // Nothing has been typed into this checkout — it is seconds old and
            // no workspace ever opened on it — so removing it is the honest
            // undo. `--force` because a fresh checkout of a branch with
            // `.gitignore`d build output in the index is "dirty" the instant it
            // exists, and refusing to clean up after ourselves over that would
            // strand the tree.
            match repo.remove_worktree(&path, true).await {
                Ok(()) => bail!("{err:#}; the worktree was removed again"),
                Err(undo) => bail!("{err:#}; the worktree at {} is still there ({undo:#})", path.display()),
            }
        }
    };
    println!("workspace {} ({branch})", created.short);

    if let Some(kind) = kind {
        let started: agent::StartReply = call(
            &mut session,
            Method::AgentStart,
            &agent::StartParams {
                name: branch.to_owned(),
                kind,
                args: Vec::new(),
                cwd: Some(path),
                workspace: Some(created.workspace),
                timeout_ms: Some(READY_TIMEOUT_MS),
            },
        )
        .await
        .context("start the agent in the new workspace")?;
        println!("agent {branch} in pane {} ({:?})", started.short, started.readiness);
        if started.readiness != agent::Readiness::Ready {
            // V13's rule, inherited: the pane is left running for inspection.
            eprintln!("amx: agent {branch} did not report ready; its pane is running");
        }
    }
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------------- done

/// `amx work done [branch]`: the workspace, its panes and the tree, together.
///
/// Every lock this module's docs list is checked here, in that order, and all of
/// them before the first destructive call.
async fn done(
    env: &Env,
    root: &ArgMatches,
    branch: Option<&str>,
    force: bool,
) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, None)?;
    let mut session = connect(&ctx).await?;
    let state: StateReply = call(&mut session, Method::SessionState, &StateParams {}).await?;
    let (workspace, worktree) = target(&state, branch)?;

    let repo = Repo::at(worktree.repo.clone());
    // Lock 1: the path is the one this template derives today, not the one a
    // reply happened to carry.
    let derived = derive_path(&template(&ctx), &worktree.repo, &worktree.branch)?;
    anyhow::ensure!(
        derived == worktree.path,
        "refusing to remove {}: the work.dir template derives {} for branch {} \
         in {}. Remove the tree with `git worktree remove` if that is what you \
         meant.",
        worktree.path.display(),
        derived.display(),
        worktree.branch,
        worktree.repo.display(),
    );

    // A tree whose directory is already gone: nothing to remove and nothing to
    // check, so the workspace is collapsed and git's administrative files are
    // pruned. This is the state restore reports as degraded, met from the other
    // side.
    let vanished = !worktree.path.exists();
    if !vanished {
        let registered = repo.worktrees().await?;
        // Lock 2: the repository's own list, not a guess about layout.
        anyhow::ensure!(
            registered.iter().any(|known| known == &worktree.path),
            "refusing to remove {}: {} does not list it as one of its worktrees",
            worktree.path.display(),
            worktree.repo.display(),
        );
        // Lock 3: never the repository's own working tree.
        anyhow::ensure!(
            registered.first() != Some(&worktree.path),
            "refusing to remove {}: that is the repository's main working tree",
            worktree.path.display(),
        );
        // Lock 4: dirty is a refusal, and it arrives before the panes die.
        if !force && repo.is_dirty(&worktree.path).await? {
            bail!(
                "{} has uncommitted changes; commit them, or repeat with --force",
                worktree.path.display()
            );
        }
    }

    let killed: workspace::KillReply = call(
        &mut session,
        Method::WorkspaceKill,
        &workspace::KillParams { workspace },
    )
    .await
    .context("kill the workspace")?;
    println!(
        "workspace {} gone with {} pane{}",
        worktree.branch,
        killed.panes.len(),
        if killed.panes.len() == 1 { "" } else { "s" }
    );

    if vanished {
        repo.prune_worktrees().await?;
        println!(
            "worktree {} was already gone; pruned its git metadata",
            worktree.path.display()
        );
    } else {
        repo.remove_worktree(&worktree.path, force).await?;
        println!("worktree {} removed", worktree.path.display());
    }
    println!("branch {} is untouched", worktree.branch);
    Ok(ExitCode::SUCCESS)
}

/// Which workspace `done` is about, and the membership it carries.
///
/// A branch names one workspace; no branch means the focused one, which is the
/// closest a one-shot verb can come to "the workspace this pane is in" — a
/// verb-only connection has no pane, and focus is workspace-scoped session state
/// rather than per-client (04 §2), so the two answers agree for the person who
/// typed it.
///
/// A workspace with no worktree is refused rather than killed: `amx work done`
/// on the workspace somebody happens to be looking at must not be a way to lose
/// it, and `amx workspace kill` is right there for a workspace that is only a
/// workspace.
fn target<'a>(
    state: &'a StateReply,
    branch: Option<&str>,
) -> anyhow::Result<(WorkspaceId, &'a Worktree)> {
    let with_tree = |ws: &'a WorkspaceState| ws.worktree.as_ref().map(|tree| (ws.workspace, tree));

    let Some(branch) = branch else {
        let focused = state
            .focused_workspace
            .context("this session has no focused workspace; name a branch")?;
        let ws = state
            .workspaces
            .iter()
            .find(|ws| ws.workspace == focused)
            .context("the session reports a focused workspace it does not list")?;
        return with_tree(ws).with_context(|| {
            format!(
                "the focused workspace ({}) was not made by `amx work`; name a branch, or use \
                 `amx workspace kill`",
                ws.label.as_deref().unwrap_or("unlabelled")
            )
        });
    };

    let mut matched = state
        .workspaces
        .iter()
        .filter(|ws| ws.worktree.as_ref().is_some_and(|tree| tree.branch == branch));
    let first = matched
        .next()
        .with_context(|| format!("no workspace in this session is on branch {branch}"))?;
    if let Some(second) = matched.next() {
        bail!(
            "two workspaces are on branch {branch} ({} and {}); this should not happen — \
             take them down with `amx workspace kill` and `git worktree remove`",
            first.workspace,
            second.workspace,
        );
    }
    with_tree(first).context("the matched workspace lost its worktree block between two reads")
}

// ------------------------------------------------------------------ inputs

/// The `work.dir` template: the config file's, then the built-in default.
///
/// Read from disk here rather than asked of the server, and leniently, through
/// the same [`amx_core::config::reload`] every server uses — so a half-edited
/// file costs the user the default rather than a failure. The same shape
/// `amx update`'s channel override has.
fn template(ctx: &Ctx) -> String {
    std::fs::read_to_string(&ctx.config_path)
        .ok()
        .and_then(|text| amx_core::config::reload(&Config::default(), &text).0.work.dir)
        .unwrap_or_else(|| DEFAULT_DIR.to_owned())
}

/// Substitute `{repo_parent}`, `{repo_name}` and `{branch}` into `template`.
///
/// A pure function of three strings, which is what makes it usable as the pin on
/// the destructive path: `done` recomputes it and compares, so the answer must
/// not depend on anything that has changed since the tree was made.
///
/// # Errors
///
/// If the repository path has no parent or no final component, if the result is
/// not absolute, if it still holds a `{...}` token — a placeholder amx does not
/// know is a typo in someone's config, and quietly making a directory named
/// after it would be worse than saying so — or if any component is `..`, which
/// no substitution of a git-legal branch name can produce and no template should
/// need.
pub fn derive_path(template: &str, repo: &Path, branch: &str) -> anyhow::Result<PathBuf> {
    let parent = repo
        .parent()
        .with_context(|| format!("{} has no parent directory", repo.display()))?;
    let name = repo
        .file_name()
        .with_context(|| format!("{} has no final path component", repo.display()))?;
    let rendered = template
        .replace("{repo_parent}", &parent.to_string_lossy())
        .replace("{repo_name}", &name.to_string_lossy())
        .replace("{branch}", branch);

    if let Some(start) = rendered.find('{') {
        let token: String = rendered[start..]
            .chars()
            .take_while(|c| *c != '}')
            .chain(std::iter::once('}'))
            .collect();
        bail!(
            "work.dir template {template:?} holds {token}, which amx does not substitute; \
             the ones it does are {{repo_parent}}, {{repo_name}} and {{branch}}"
        );
    }
    let path = PathBuf::from(rendered);
    anyhow::ensure!(
        path.is_absolute(),
        "work.dir template {template:?} derives {}, which is not an absolute path",
        path.display(),
    );
    anyhow::ensure!(
        !path
            .components()
            .any(|part| part == std::path::Component::ParentDir),
        "work.dir template {template:?} derives {}, which walks upwards",
        path.display(),
    );
    Ok(path)
}

// ------------------------------------------------------------------- wire

/// Connect to a running session, without starting one.
///
/// `amx work` is not one of the two commands that mean "make this session
/// exist", and `attach: false` because a one-shot verb must not seed a workspace
/// into an empty session by connecting — the seeded one would land beside the
/// workspace this verb is about to make.
async fn connect(ctx: &Ctx) -> anyhow::Result<Session> {
    anyhow::ensure!(
        probe(&ctx.socket)
            .context("probe the session socket")?
            .is_running(),
        "session {} is not running; start it with `amx`",
        ctx.session
    );
    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .context("negotiate with the session")?;
    Ok(session)
}

/// Make one control call with typed parameters and a typed reply.
async fn call<P, R>(session: &mut Session, method: Method, params: &P) -> anyhow::Result<R>
where
    P: Serialize,
    R: serde::de::DeserializeOwned,
{
    let params: Value = serde_json::to_value(params).context("encode the call")?;
    let reply = session.call(method.wire_name(), params).await?;
    serde_json::from_value(reply).context("read the reply")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{DEFAULT_DIR, derive_path};

    #[test]
    fn the_default_template_puts_the_tree_beside_the_repository() {
        assert_eq!(
            derive_path(DEFAULT_DIR, Path::new("/src/amx"), "feat").unwrap(),
            PathBuf::from("/src/amx--feat"),
        );
        // A branch with a slash nests, which is a directory the template's own
        // author asked for by writing `{branch}` — still under the sibling
        // prefix, still nowhere near the repository's inside.
        assert_eq!(
            derive_path(DEFAULT_DIR, Path::new("/src/amx"), "feature/x").unwrap(),
            PathBuf::from("/src/amx--feature/x"),
        );
    }

    #[test]
    fn a_template_amx_cannot_satisfy_is_refused_by_name() {
        let err = derive_path("{repo_parent}/{repo}--{branch}", Path::new("/src/amx"), "feat")
            .unwrap_err()
            .to_string();
        assert!(err.contains("{repo}"), "{err}");

        for bad in ["trees/{branch}", "{repo_parent}/../{branch}"] {
            assert!(
                derive_path(bad, Path::new("/src/amx"), "feat").is_err(),
                "{bad:?} must not derive a path",
            );
        }
    }
}
