//! The generated verbs: one control call over the session socket.
//!
//! Every subcommand the method table produces lands here. The path the user
//! typed (`pane split`) is looked up in [`amx_proto::control::SPECS`] to get the
//! wire name (`pane.split`), the `--params` JSON is sent as-is, and the reply is
//! printed as JSON. No arm of this function names a method, which is the point
//! of deriving the tree from the table (04 §4, fixes W6): a new row in the table
//! is a new working verb here with no edit.
//!
//! These calls do **not** start a server. A one-shot verb against a session
//! nobody is running is a mistake worth reporting, not a reason to daemonize:
//! `amx` and `amx attach` are the two commands that mean "make this session
//! exist".

use std::process::ExitCode;

use amx_client::net::{self, Session};
use amx_core::Env;
use amx_proto::control::SPECS;
use amx_server::session::probe::probe;
use anyhow::Context as _;
use clap::ArgMatches;
use serde_json::Value;

use crate::cli::PARAMS;
use crate::cmd::attach::client_info;
use crate::ctx_of;

/// Make the control call `name`/`matches` names.
pub async fn run(
    env: &Env,
    root: &ArgMatches,
    name: &str,
    matches: &ArgMatches,
) -> anyhow::Result<ExitCode> {
    let (path, leaf) = leaf_of(name, matches);
    let method = SPECS
        .iter()
        .find(|spec| spec.cli == path.as_slice())
        .with_context(|| format!("no such command: {}", path.join(" ")))?;

    let params: Value = match leaf.get_one::<String>(PARAMS) {
        Some(json) => serde_json::from_str(json).context("--params must be a JSON value")?,
        None => Value::Object(serde_json::Map::new()),
    };

    let reply = one_shot(env, root, method.wire, params).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&reply).context("format the reply")?
    );
    Ok(ExitCode::SUCCESS)
}

/// Make one control call against the session `root` selects, and hand back its
/// raw reply.
///
/// The connect-probe-negotiate preamble every one-shot verb shares, factored
/// out so a verb that formats its own reply (`amx session report`) reaches the
/// session exactly the way the generated ones do — including refusing to start
/// a server, which is the rule stated above.
pub async fn one_shot(
    env: &Env,
    root: &ArgMatches,
    wire: &str,
    params: Value,
) -> anyhow::Result<Value> {
    let ctx = ctx_of(env, root, None)?;
    anyhow::ensure!(
        probe(&ctx.socket)
            .context("probe the session socket")?
            .is_running(),
        "session {} is not running; start it with `amx`",
        ctx.session
    );

    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (mut session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .context("negotiate with the session")?;
    session
        .call(wire, params)
        .await
        .with_context(|| format!("call {wire}"))
}

/// The full CLI path the user typed, and the matches of its last segment.
fn leaf_of<'a>(name: &'a str, matches: &'a ArgMatches) -> (Vec<&'a str>, &'a ArgMatches) {
    let mut path = vec![name];
    let mut leaf = matches;
    while let Some((sub, next)) = leaf.subcommand() {
        path.push(sub);
        leaf = next;
    }
    (path, leaf)
}
