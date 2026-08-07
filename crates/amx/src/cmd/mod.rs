//! One module per verb, and the dispatch that picks between them.
//!
//! M2's four new verbs are planted here by V02 alongside their clap trees, for
//! the reason `cli.rs` explains: the routing arms of a file every task wants a
//! line in land once, in the contracts task, so no two wave tasks collide over
//! it (the U01 precedent). **V09** fills [`hook`], **V10** [`integration`],
//! **V11** [`events`], **V16** [`skill`].

pub mod attach;
pub mod call;
pub mod detach;
pub mod events;
pub mod hook;
pub mod integration;
pub mod server;
pub mod session;
pub mod skill;
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
        // The emitter never fails, by contract: an agent's turn must not be
        // broken or slowed by a hook, so this arm returns an exit code rather
        // than a `Result` and nothing above it can add an error message.
        Some(("_hook", sub)) => Ok(hook::run(env, sub).await),
        Some(("integration", sub)) => integration::run(env, matches, sub).await,
        Some(("skill", sub)) => skill::run(env, matches, sub).await,
        // `events` is both: `events subscribe` is a generated method row, and a
        // bare `amx events [--json]` is the streaming consumer of 04 §8.
        Some(("events", sub)) => match sub.subcommand() {
            Some(("subscribe", _)) => call::run(env, matches, "events", sub).await,
            _ => events::run(env, matches, sub).await,
        },
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
