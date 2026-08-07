//! `amx layout export` — a session's shape as a file (D-M3-11).
//!
//! **W13** fills this. Entirely client-side: `session.state` already carries
//! the workspace list, the BSP trees, the cwds, the labels and the agent kinds,
//! so export is a rendering of a reply and needs no new wire type at all. That
//! is also what lets a layout be exported from a remote session over the bridge
//! unchanged.
//!
//! Planted by W03 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run `amx layout`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **W13** owes it.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx layout` is not wired yet; W13 owes it")
}
