//! `amx agents` — the read-only mission-control screen (D15 surface 3).
//!
//! **X16** fills this. It is the human rendering of `agent.list`, and the two
//! spellings are deliberate rather than an accident (`docs/11-m4-plan.md`
//! D-M4-11): `amx agent list` is the generated machine surface the method table
//! gives for free, and this is the table a person reads — workspace, name,
//! status, reason, age, last line — with `--json` printing the reply verbatim
//! so nobody has to know there are two.
//!
//! `--watch` is `amx events --json`'s loop packaged, not a second contract:
//! subscribe, redial on EOF, re-query on `gap`, which is what
//! [`super::events`] already documents and implements. R-M4-7 is the
//! constraint it inherits — one `agent.list` per refresh window for every row,
//! never one per pane.
//!
//! Planted by X02 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/11-m4-plan.md` §5).

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Run `amx agents`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **X16** owes it, and the row it renders is
/// **X10**'s.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx agents` is not wired yet; X16 owes it")
}
