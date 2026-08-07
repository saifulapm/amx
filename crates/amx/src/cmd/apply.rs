//! `amx apply <file>` — replay a layout through the public verbs (D-M3-11).
//!
//! **W13** fills this, beside [`super::layout`]. Parse the whole file before
//! the first call — a malformed layout names its line and applies nothing,
//! because a half-applied layout is worse than a refused one — then replay
//! `workspace.create`, `pane.split` and `agent.start` in an order that
//! reconstructs the BSP exactly.
//!
//! Planted by W03 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run `amx apply`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **W13** owes it.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx apply` is not wired yet; W13 owes it")
}
