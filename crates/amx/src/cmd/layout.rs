//! `amx layout export` — a session's shape as a file (D-M3-11).
//!
//! Entirely client-side: `session.state` already carries the workspace list,
//! the BSP trees, the labels and the agent kinds, so export is a rendering of a
//! reply and needs no new wire type at all. That is also what lets a layout be
//! exported from a remote session over the bridge unchanged.
//!
//! What it never writes is a session ref. A layout is a shape, not a
//! conversation: a ref is machine-specific and secrets-adjacent, and the point
//! of a layout file is that it can be committed, mailed and replayed elsewhere.
//! [`crate::layout::schema`] has no key for one, so the omission is structural
//! rather than a line of care in this file.

use std::path::PathBuf;
use std::process::ExitCode;

use amx_core::Env;
use amx_proto::control::Method;
use amx_proto::control::session::StateReply;
use anyhow::Context as _;
use clap::ArgMatches;

use crate::cmd::call;
use crate::layout::build;

/// Run `amx layout`.
///
/// # Errors
///
/// If the session is not running, if it refuses `session.state`, or if the
/// file cannot be written.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let (verb, leaf) = sub.subcommand().context("amx layout needs a verb")?;
    match verb {
        "export" => export(env, root, leaf).await,
        other => anyhow::bail!("unknown layout verb: {other}"),
    }
}

/// Write the session's shape, to a file or to stdout.
async fn export(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let reply = call::one_shot(
        env,
        root,
        Method::SessionState.wire_name(),
        serde_json::json!({}),
    )
    .await?;
    let state: StateReply =
        serde_json::from_value(reply).context("read the session's state reply")?;
    let file = build::project(&state).context("read the session's layout trees")?;
    let text = file.to_string();

    match sub.get_one::<String>("out").map(PathBuf::from) {
        Some(path) => {
            std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
        }
        None => print!("{text}"),
    }
    Ok(ExitCode::SUCCESS)
}
