//! `amx _bridge` — the byte splice behind an SSH remote (D-M3-9).
//!
//! **W11** fills this. A splice and nothing more: resolve the session, connect
//! to its socket, copy bytes both ways, and exit with the connect error before
//! the first protocol byte if there is one. The local side hands one end of a
//! socketpair to the ssh child as stdin+stdout and gives the other to
//! `Session::attach`, which takes a `UnixStream` today and never learns its
//! peer is a subprocess — which is why the whole remote story costs the client
//! no changes at all.
//!
//! Planted by W03 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).
//! `tests/skew.rs` already carries the row that runs the conformance table over
//! this transport, and it fails the moment this stub goes away without the row
//! being finished.

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

/// What an unwired `_bridge` prints, and what `tests/skew.rs` recognises it by.
///
/// Named rather than spelled twice: the skew suite's bridge row asserts this
/// exact refusal, so the day W11 writes a real splice the row stops matching
/// and has to be finished rather than quietly passing over a stub.
pub const NOT_WIRED: &str = "`amx _bridge` is not wired yet; W11 owes it";

/// Run `amx _bridge`.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than a silent success. **W11** owes it.
pub async fn run(_env: &Env, _root: &ArgMatches, _sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    anyhow::bail!(NOT_WIRED)
}
