//! `amx update check|apply` — self-update (D-M3-8).
//!
//! **W10** fills this. The mechanisms are herdr's, mined and not copied:
//! channel manifests as JSON with a per-platform asset URL and sha256, fetched
//! with a curl subprocess so no HTTP dependency enters the tree, verified
//! before install, and package-manager installs *redirected* — print brew's or
//! mise's or nix's own upgrade command, never write into their tree.
//!
//! The honesty this verb owes (R-M3-4): there is no release pipeline and no
//! published manifest yet, so `check` against the default channel reports that
//! plainly rather than failing or pretending otherwise.
//!
//! Planted by W03 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run `amx update`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **W10** owes it.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx update` is not wired yet; W10 owes it")
}
