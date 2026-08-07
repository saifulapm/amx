//! One module per verb, and the dispatch that picks between them.

pub mod attach;
pub mod call;
pub mod detach;
pub mod server;
pub mod session;
pub mod viewport;

use std::process::ExitCode;

use amx_core::Env;
use clap::ArgMatches;

use crate::ctx_of;

/// Run the command `matches` names.
///
/// `amx` with no subcommand is `amx attach` with no arguments — 04 §1's first
/// line ("probe socket; daemonize `amx server` if absent; attach") is the
/// default behavior, not a verb you have to know.
pub async fn dispatch(env: &Env, matches: &ArgMatches) -> anyhow::Result<ExitCode> {
    match matches.subcommand() {
        None => attach::run(&ctx_of(env, matches, None)?, attach::Options::default()).await,
        Some(("attach", sub)) => {
            attach::run(&ctx_of(env, matches, None)?, attach::Options::parse(sub)?).await
        }
        Some(("server", _)) => server::run(ctx_of(env, matches, None)?).await,
        // The `session` group holds both the lifecycle verbs and the generated
        // `session.state`/`session.report` calls; the verb list decides which
        // module answers. `report` is a control call like `state`, but its
        // output is written for a human, so `session` formats it.
        Some(("session", sub)) => match sub.subcommand() {
            Some(("list" | "attach" | "stop" | "delete" | "report", _)) => {
                session::run(env, matches, sub).await
            }
            _ => call::run(env, matches, "session", sub).await,
        },
        Some((name, sub)) => call::run(env, matches, name, sub).await,
    }
}
