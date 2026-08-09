//! `amx agents` — the read-only mission-control screen (D15 surface 3).
//!
//! The human rendering of `agent.list`, and the two spellings are deliberate
//! rather than an accident (`docs/11-m4-plan.md` D-M4-11): `amx agent list` is
//! the generated machine surface the method table gives for free, and this is
//! the table a person reads — workspace, name, status, reason, age, last line —
//! with `--json` printing the reply verbatim so nobody has to know there are
//! two.
//!
//! The rendering lives in [`crate::agents`], because both forms use it; what is
//! here is the three decisions the command itself makes.
//!
//! **It never starts a server.** The same rule every one-shot verb follows
//! (`super::call`): `amx` and `amx attach` are the two commands that mean "make
//! this session exist", and a monitor is not one of them.
//!
//! **It needs no client attached.** One control connection, no bind, no
//! viewport, nothing declared — so a spare pane, a plain SSH window and a phone
//! all work, which is the workflow D14 exists for.
//!
//! **`--watch --json` is refused, and the refusal is the documentation.** The
//! two flags mean opposite things: `--json` is the machine surface and
//! `--watch` is the human packaging of a contract machines already have. D15
//! says so — "live consumers that want a stream use `amx events --json` plus
//! re-query on `gap`, the standard contract; `--watch` is that loop, packaged"
//! — so the refusal points at the spelling that works instead of inventing a
//! third one.

use std::process::ExitCode;

use amx_client::net::{self, Session};
use amx_client::term::window_size;
use amx_core::Env;
use amx_proto::control::agent::ListReply;
use amx_server::session::probe::probe;
use anyhow::Context as _;
use clap::ArgMatches;

use crate::agents::{scope::Scope, table, watch};
use crate::cli::JSON;
use crate::cmd::attach::client_info;
use crate::ctx_of;

/// Run `amx agents`.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let json = sub.get_flag(JSON);
    let watching = sub.get_flag("watch");
    anyhow::ensure!(
        !(json && watching),
        "--json prints one reply; --watch is the live screen. For a live \
         machine-readable stream use `amx events --json` and re-query on a \
         `gap`, which is the contract --watch packages for a person."
    );

    let scope = Scope::new(sub.get_one::<String>("workspace").cloned());
    let ctx = ctx_of(env, root, None)?;
    // Probed before anything else, and before `--watch` touches the terminal:
    // "there is no server" is a sentence worth reading on the screen the user
    // is already looking at, not on an alternate one that is about to go away.
    anyhow::ensure!(
        probe(&ctx.socket)
            .context("probe the session socket")?
            .is_running(),
        "session {} is not running; start it with `amx`",
        ctx.session
    );

    if watching {
        return watch::run(&ctx, &scope).await;
    }
    one_shot(&ctx, &scope, json).await
}

/// The one-shot form: one connection, one reply, one table.
async fn one_shot(ctx: &amx_core::Ctx, scope: &Scope, json: bool) -> anyhow::Result<ExitCode> {
    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (mut session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .context("negotiate with the session")?;

    let params = scope.params(&mut session).await?;
    let reply = session
        .call("agent.list", params)
        .await
        .context("call agent.list")?;

    if json {
        // Verbatim, off the wire, and not round-tripped through this build's
        // own types: a field a newer server carries and this binary has never
        // heard of reaches the consumer, which is the whole promise of "the
        // reply, verbatim".
        println!(
            "{}",
            serde_json::to_string_pretty(&reply).context("format the reply")?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let reply: ListReply = serde_json::from_value(reply).context("decode the agent.list reply")?;
    for line in table::render(&reply, reply.now, terminal_width()) {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

/// How wide the output may be, when it is going to a terminal at all.
///
/// `None` for a pipe, a file or a capture, and then nothing is truncated: a
/// consumer redirecting this asked for the last line, not for the part of it
/// that would have fitted a window that is not there.
fn terminal_width() -> Option<usize> {
    window_size(std::io::stdout())
        .ok()
        // A zero-column terminal is what a size nobody filled in looks like.
        .filter(|size| size.cols > 0)
        .map(|size| usize::from(size.cols))
}
