//! The `amx` binary, as a library so its parts can be tested.
//!
//! 04 §1: "One binary, two roles" — `amx` probes the session socket,
//! daemonizes `amx server` if nothing answers, and attaches; `amx <verb>` is a
//! one-shot call over the same socket. Everything below is the wiring of that
//! sentence and nothing else: the session lifetime lives in
//! [`amx_server::session`], the client lives in [`amx_client`], and this crate
//! only decides which of them to call.
//!
//! The binary target is a three-line `main.rs` over [`run`]. That split is not
//! ceremony: `crates/amx/tests/` can only run a binary as a process, and the
//! pure parts of a CLI — the parse of `--session`, the detach chord, the
//! chrome-free viewport paint — deserve tests that assert on values rather
//! than on scraped output.

use std::ffi::OsString;
use std::process::ExitCode;

use amx_core::{Ctx, Env, SessionName};
use anyhow::Context as _;

pub mod cli;
pub mod cmd;
pub mod integration;
pub mod layout;
pub mod update;

/// Parse `argv` and run the command it names.
///
/// The environment is read exactly once, here, and passed down as a value: no
/// function below this one calls `std::env::var` (04 §2, fixes W9).
pub fn run<I, T>(argv: I) -> anyhow::Result<ExitCode>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = cli::cli().try_get_matches_from(argv)?;
    let env = Env::from_process();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the tokio runtime")?;
    let outcome = runtime.block_on(cmd::dispatch(&env, &matches));
    // Not `drop`: dropping a runtime waits for every blocking task, and the
    // attach loop leaves one behind by construction — `tokio::io::stdin` reads
    // on a blocking thread, and the read that is pending when the user detaches
    // only returns when they press another key. Waiting for that would hang
    // every detach until the terminal was typed into again.
    runtime.shutdown_background();
    outcome
}

/// Which session a set of matches selects.
///
/// `--session` wins over `$AMX_SESSION`, which wins over `default`; the flag is
/// global, so it may appear before or after the subcommand. `explicit` is the
/// name a `session <verb> <name>` positional supplied, which outranks both.
pub fn session_of(
    env: &Env,
    matches: &clap::ArgMatches,
    explicit: Option<&str>,
) -> anyhow::Result<SessionName> {
    if let Some(name) = explicit {
        return SessionName::new(name).context("invalid session name");
    }
    if let Some(name) = matches.get_one::<String>(cli::SESSION) {
        return SessionName::new(name.as_str()).context("invalid --session name");
    }
    Ok(env.session.clone().unwrap_or_default())
}

/// The context for the session `matches` selects.
pub fn ctx_of(
    env: &Env,
    matches: &clap::ArgMatches,
    explicit: Option<&str>,
) -> anyhow::Result<Ctx> {
    let session = session_of(env, matches, explicit)?;
    Ctx::for_session(session, env).context("derive the session's paths")
}
