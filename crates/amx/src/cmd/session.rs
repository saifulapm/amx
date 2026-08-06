//! `amx session list|attach|stop|delete` (04 §1).
//!
//! "A daemon that outlives terminals must be discoverable and stoppable from
//! the CLI." Each verb is a thin front for one function in
//! [`amx_server::session::registry`]; what lives here is the output format and
//! the exit codes, which are the parts a script depends on.

use std::process::ExitCode;
use std::time::Duration;

use amx_core::Env;
use amx_server::session::registry::{self, StopOutcome};
use anyhow::Context as _;
use clap::ArgMatches;

use crate::cmd::attach;
use crate::ctx_of;

/// How long `stop` waits for the server to go away after signalling it.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Run one `amx session …` verb.
pub async fn run(env: &Env, root: &ArgMatches, matches: &ArgMatches) -> anyhow::Result<ExitCode> {
    let (verb, sub) = matches.subcommand().context("amx session needs a verb")?;
    // `list` is the one verb with no name argument, so the lookup is per-arm:
    // asking clap for an argument a subcommand never declared is a panic, not
    // a `None`.
    let named = |sub: &ArgMatches| sub.get_one::<String>("name").cloned();

    match verb {
        "list" => list(env),
        "attach" => {
            let ctx = ctx_of(env, root, named(sub).as_deref())?;
            attach::run(&ctx, attach::Options::default()).await
        }
        "stop" => stop(env, root, named(sub).as_deref()).await,
        "delete" => delete(env, root, named(sub).as_deref()),
        other => anyhow::bail!("unknown session verb: {other}"),
    }
}

/// Print the sessions with a server answering, one per line.
///
/// Name and socket, tab separated and nothing else: a list a script can cut is
/// worth more than a table a human can admire, and the human can read this too.
/// Sessions whose server is gone are not listed at all — a runtime directory is
/// evidence that a session *ran*, and `amx session delete` is what removes one.
fn list(env: &Env) -> anyhow::Result<ExitCode> {
    let running = registry::list(env).context("enumerate sessions")?;
    for entry in &running {
        println!("{}\t{}", entry.session, entry.socket.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// Stop a session's server and wait for its socket to go quiet.
async fn stop(env: &Env, root: &ArgMatches, named: Option<&str>) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, named)?;
    match registry::stop(&ctx, STOP_TIMEOUT)
        .await
        .with_context(|| format!("stop session {}", ctx.session))?
    {
        StopOutcome::Stopped { pid } => {
            println!("stopped {} (pid {pid})", ctx.session);
            Ok(ExitCode::SUCCESS)
        }
        StopOutcome::NotRunning => {
            eprintln!("amx: session {} is not running", ctx.session);
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Remove a stopped session's directories.
fn delete(env: &Env, root: &ArgMatches, named: Option<&str>) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, named)?;
    registry::delete(&ctx).with_context(|| format!("delete session {}", ctx.session))?;
    println!("deleted {}", ctx.session);
    Ok(ExitCode::SUCCESS)
}
