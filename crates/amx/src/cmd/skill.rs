//! `amx skill install` — the in-binary agent skill.
//!
//! 04 §8, herdr's K10 kept: "The **agent skill** ships in-binary: `amx skill
//! install` teaches agents to drive amx — spawn panes, send input, prompt
//! siblings, `wait --until blocked`, read outputs — gated on `AMX_ENV=1` with
//! pane/workspace identity env vars."
//!
//! In-binary because a skill fetched from anywhere else is a skill that can
//! disagree with the binary it is describing. The test that keeps it honest
//! walks every verb the asset names against `SPECS`, so a renamed method breaks
//! the build here rather than in an agent's hands.
//!
//! # Task ownership
//!
//! **V16** fills this, with the asset under `crates/amx/assets/skill/` and
//! `examples/notify.sh` beside it.
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// Write the agent skill into a project.
///
/// **V16 fills this.**
pub async fn run(env: &Env, matches: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let _ = (env, matches, sub);
    anyhow::bail!("amx skill is not implemented yet; V16 fills it")
}
