//! `amx work <branch>` / `amx work done` — git worktrees (D-M3-10).
//!
//! **W12** fills this. Three existing capabilities composed, not a new server
//! concept: `git worktree add` as argv — never a shell string —
//! `workspace.create` with the tree as cwd, and `agent.start` when `--kind` is
//! given. The one thing the server remembers is the membership block W03 added
//! to workspace state and to the snapshot, because `done` resolves a workspace
//! by branch and restore has to notice a tree that is no longer there.
//!
//! Planted by W03 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run `amx work`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **W12** owes it.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx work` is not wired yet; W12 owes it")
}
