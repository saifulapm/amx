//! `amx keys` — print the resolved keybinding table.
//!
//! **X07** fills this. 04 §7 promises the bindings are introspectable and for
//! three milestones there was nothing to introspect: the prefix was a `const`
//! and the table was a `match` on byte literals (`docs/11-m4-plan.md` D-M4-8).
//! X07 turns both into data read from `[keys]`, and this is what makes the
//! result visible — including *which* bindings came from the file, because a
//! table that cannot show you what your own config did is a table you debug by
//! guessing.
//!
//! It reads configuration and talks to no server: the bindings live entirely
//! client-side, so this answers from `Ctx::config_path` alone and works with no
//! session running.
//!
//! Planted by X02 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/11-m4-plan.md` §5).

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run `amx keys`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **X07** owes it.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx keys` is not wired yet; X07 owes it")
}
