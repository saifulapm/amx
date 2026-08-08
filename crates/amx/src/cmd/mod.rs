//! One module per verb, and the dispatch that picks between them.
//!
//! M2's four new verbs are planted here by V02 alongside their clap trees, for
//! the reason `cli.rs` explains: the routing arms of a file every task wants a
//! line in land once, in the contracts task, so no two wave tasks collide over
//! it (the U01 precedent). **V09** fills [`hook`], **V10** [`integration`],
//! **V11** [`events`], **V16** [`skill`].
//!
//! M3's six are planted the same way by W03: **W10** fills [`update`], **W11**
//! [`bridge`], **W12** [`work`], **W13** [`layout`] and [`apply`], **W06**
//! [`handoff_caps`]. Each of those modules is a stub that names its owner and
//! refuses; the arms below are what make the refusal reachable, so the task
//! that fills one writes a body and touches nothing else.
//!
//! M4's two are planted by X02: **X16** fills [`agents`], **X07** [`keys`].
//! Neither is a method-table row — `agents` renders one for a person and
//! `keys` reads configuration and talks to no server at all.

pub mod agents;
pub mod apply;
pub mod attach;
pub mod bridge;
pub mod call;
pub mod detach;
pub mod events;
pub mod handoff_caps;
pub mod hook;
pub mod integration;
pub mod keys;
pub mod layout;
pub mod server;
pub mod session;
pub mod skill;
pub mod update;
pub mod viewport;
pub mod work;

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
        // The sub-matches are read, unlike every other lifecycle arm: `amx
        // server` carries `--handoff-import`, the one flag that changes which
        // assembly the process runs (`docs/09-m3-plan.md` §3).
        Some(("server", sub)) => server::run(ctx_of(env, matches, None)?, sub).await,
        // The emitter never fails, by contract: an agent's turn must not be
        // broken or slowed by a hook, so this arm returns an exit code rather
        // than a `Result` and nothing above it can add an error message.
        Some(("_hook", sub)) => Ok(hook::run(env, sub).await),
        Some(("integration", sub)) => integration::run(env, matches, sub).await,
        Some(("skill", sub)) => skill::run(env, matches, sub).await,
        // M3's six. Four compose existing capabilities client-side and two are
        // hidden plumbing; none of them is a control call, which is why each
        // gets an arm here rather than a method-table row.
        Some(("update", sub)) => update::run(env, matches, sub).await,
        Some(("work", sub)) => work::run(env, matches, sub).await,
        Some(("layout", sub)) => layout::run(env, matches, sub).await,
        Some(("apply", sub)) => apply::run(env, matches, sub).await,
        Some(("_bridge", sub)) => bridge::run(env, matches, sub).await,
        // M4's two. `agents` renders `agent.list` for a person; `keys` prints
        // the resolved keybinding table and reaches no server at all.
        Some(("agents", sub)) => agents::run(env, matches, sub).await,
        Some(("keys", sub)) => keys::run(env, matches, sub).await,
        // No session and no environment: it answers about the binary it is,
        // and an exporter runs it before deciding whether to touch anything.
        Some(("_handoff-caps", _)) => handoff_caps::run().await,
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
