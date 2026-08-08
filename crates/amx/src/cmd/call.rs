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
//!
//! # Riding a swap (D-M3-7)
//!
//! A handoff retires the session socket and the successor binds it back a
//! moment later (`docs/09-m3-plan.md` §3 step 11). Every one-shot verb that
//! spans that window — and `amx wait` spans it by design, since a standing wait
//! is the whole point of the verb — sees its connection end. Before M3 that was
//! the command's answer: a broken pipe, and a wait that had been waiting for
//! ten minutes lost. Now the failure is redialled and the call re-issued.
//!
//! **A swap ends a standing wait in two different ways, and both are the same
//! event.** The socket can simply die under it; or the server can cut the wait
//! short first and *say so* —
//! [`RpcError::WAIT_ABANDONED`](amx_proto::RpcError::WAIT_ABANDONED), sent by
//! the exporter as it quiesces, on a connection that is about to close but has
//! not yet. Which of the two the client sees is a race between the reply and
//! the socket, decided by how loaded the machine is; treating only the first as
//! a reconnect made the milestone's own sentence — "waits keep waiting" — true
//! on a fast machine and false on a slow one. So the abandoned-wait code is
//! read as exactly what a dropped connection is: the session went away
//! mid-call. It is a code and never a message, because a string is not a
//! contract.
//!
//! Two rules keep the re-issue honest, and both are refusals:
//!
//! - **A call that was written is only re-issued when re-issuing it is asking
//!   the same question.** `wait`, `pane.wait_output` and the pure reads are
//!   *state* predicates (04 §2), so a transition that fired while nothing was
//!   connected is simply true when the call is made again — which is what makes
//!   the retry exact rather than lucky. `agent.prompt` is not: its first act is
//!   to type into a pane, and a connection that died after the request was
//!   written cannot say whether it typed. Re-issuing that would be a second
//!   prompt, so it is refused with the transport error instead
//!   ([`reissuable`]).
//! - **A failure before the request was written is always retried**, whatever
//!   the method: connect and `Hello` are the only two things that happened, and
//!   neither touched the session. That is the ordinary shape of a verb typed
//!   while a swap is in flight.
//!
//! The redial window is the caller's own patience, not a constant invented
//! here: a call carrying `timeout_ms` gets exactly that long *in total*, and the
//! re-issued call's timeout is reduced by what has already been spent, so
//! `--timeout 2s` is two seconds whether it took one connection or four.
//! [`REDIAL_WINDOW`] is what a call with no timeout of its own gets.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use amx_client::net::{self, NetError, Session};
use amx_core::{Ctx, Env};
use amx_proto::control::SPECS;
use amx_proto::control::{Method, cli};
use amx_server::session::probe::probe;
use anyhow::Context as _;
use clap::ArgMatches;
use serde_json::Value;

use crate::cli::PARAMS;
use crate::cmd::attach::client_info;
use crate::ctx_of;

/// How long a verb with no deadline of its own keeps redialling.
///
/// Bounded by what a swap can cost rather than by what a user will tolerate:
/// §3's socket-free probe loop is capped at five seconds, and a verb that gave
/// up at four would turn a slow-but-successful upgrade into an error message.
/// Everything past that is a server that is not coming back, which is worth
/// saying rather than waiting out.
const REDIAL_WINDOW: Duration = Duration::from_secs(10);

/// How long the first redial waits, and how long the longest does.
const FIRST_BACKOFF: Duration = Duration::from_millis(20);
const LONGEST_BACKOFF: Duration = Duration::from_millis(250);

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

    // One or the other, never both: clap refuses the combination, so reaching
    // the flag layer means `--params` was absent. A row with no flags and no
    // `--params` sends the empty object, as it always has.
    let params: Value = match leaf.get_one::<String>(PARAMS) {
        Some(json) => serde_json::from_str(json).context("--params must be a JSON value")?,
        None => cli::params_from_flags(method.method, leaf)?,
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
/// a server, which is the rule stated above, and including the redial the
/// module header describes.
pub async fn one_shot(
    env: &Env,
    root: &ArgMatches,
    wire: &str,
    params: Value,
) -> anyhow::Result<Value> {
    let ctx = ctx_of(env, root, None)?;
    // Probed once, before anything is attempted: "there is no server" and "the
    // server went away mid-call" are different sentences, and only the second
    // one is worth redialling.
    anyhow::ensure!(
        probe(&ctx.socket)
            .context("probe the session socket")?
            .is_running(),
        "session {} is not running; start it with `amx`",
        ctx.session
    );

    let started = Instant::now();
    let window = redial_window(&params);
    let reissuable = Method::from_wire_name(wire).is_some_and(reissuable);
    let mut attempts = 0_u32;
    let mut last: Option<NetError> = None;

    loop {
        if attempts > 0 {
            let delay = backoff(attempts - 1);
            if delay >= window.saturating_sub(started.elapsed()) {
                return Err(gave_up(&ctx, started.elapsed(), last));
            }
            tokio::time::sleep(delay).await;
        }
        attempts += 1;

        // The timeout the *re-issued* call carries is what is left of the one
        // the caller asked for. Without this a verb that lost three
        // connections would wait three times as long as it was told to.
        // Measured after the backoff and not before it: time spent waiting to
        // redial is the caller's time too.
        let left = window.saturating_sub(started.elapsed());
        let attempt = attempt(&ctx, wire, &spend(&params, left)).await;
        match attempt {
            Ok(value) => return Ok(value),
            Err(Attempt::Refused(err)) => {
                return Err(anyhow::Error::new(err).context(format!("call {wire}")));
            }
            Err(Attempt::Fatal(err)) => return Err(err),
            Err(Attempt::Lost { sent, err }) => {
                if sent && !reissuable {
                    return Err(anyhow::Error::new(err).context(format!(
                        "the session went away while {wire} was in flight, and re-issuing it \
                         could repeat what it already did"
                    )));
                }
                if started.elapsed() >= window {
                    return Err(gave_up(&ctx, started.elapsed(), Some(err)));
                }
                last = Some(err);
            }
        }
    }
}

/// How one attempt ended.
enum Attempt {
    /// The session went away under the call — the connection failed, or it
    /// answered that it was abandoning the call unanswered. `sent` says whether
    /// the request had been written when it did, which is the whole of what
    /// makes re-issuing safe or not.
    Lost { sent: bool, err: NetError },
    /// The server answered, and its answer was a refusal.
    Refused(NetError),
    /// Something no redial would change.
    Fatal(anyhow::Error),
}

/// Connect, negotiate and make one call.
async fn attempt(ctx: &Ctx, wire: &str, params: &Value) -> Result<Value, Attempt> {
    let stream = net::connect(&ctx.socket)
        .await
        .map_err(|err| Attempt::Lost { sent: false, err })?;
    // Nothing this connection did before the request went out can have touched
    // the session: the handshake is a `Hello` and a `Welcome`, and a verb
    // connection is defined as one that mutates nothing by connecting
    // (`amx_proto::Hello::attach`).
    let (mut session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .map_err(|err| {
            if err.is_transport() {
                Attempt::Lost { sent: false, err }
            } else {
                Attempt::Fatal(anyhow::Error::new(err).context("negotiate with the session"))
            }
        })?;
    session.call(wire, params.clone()).await.map_err(|err| {
        // A wait the session abandoned is the session going away, not an
        // answer: it says the question was dropped unanswered, which is the
        // one refusal a redial can legitimately ask again.
        if err.is_transport() || err.is_abandoned_wait() {
            Attempt::Lost { sent: true, err }
        } else {
            Attempt::Refused(err)
        }
    })
}

/// The error a verb that ran out of patience reports.
fn gave_up(ctx: &Ctx, after: Duration, last: Option<NetError>) -> anyhow::Error {
    let why = last.map_or_else(
        || "the session socket never answered".to_owned(),
        |err| err.to_string(),
    );
    anyhow::anyhow!(
        "session {} stopped answering and did not come back within {after:.1?}: {why}",
        ctx.session
    )
}

/// The delay after `attempts` failed redials: doubling, capped.
fn backoff(attempts: u32) -> Duration {
    let grown = FIRST_BACKOFF
        .as_millis()
        .saturating_mul(1_u128 << attempts.min(16));
    Duration::from_millis(u64::try_from(grown).unwrap_or(u64::MAX)).min(LONGEST_BACKOFF)
}

/// How long this call may spend in total, connections included.
///
/// A `timeout_ms` in the params is the caller stating their patience, and it is
/// taken literally: nothing about losing a server entitles a verb to outstay
/// what it was told. Everything else gets [`REDIAL_WINDOW`].
fn redial_window(params: &Value) -> Duration {
    params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .map_or(REDIAL_WINDOW, Duration::from_millis)
}

/// The same params with `timeout_ms` reduced to `left`, when it has one.
fn spend(params: &Value, left: Duration) -> Value {
    let Some(object) = params.as_object() else {
        return params.clone();
    };
    if !object.contains_key("timeout_ms") {
        return params.clone();
    }
    let mut spent = params.clone();
    if let Some(slot) = spent.get_mut("timeout_ms") {
        *slot = Value::from(u64::try_from(left.as_millis()).unwrap_or(u64::MAX));
    }
    spent
}

/// Whether re-issuing `method` on a fresh connection asks the same question.
///
/// True only for the reads and the state predicates. The list is exhaustive so
/// a new row in the method table cannot inherit an answer nobody chose — the
/// same discipline `mutates_layout` keeps in the client's wired loop.
const fn reissuable(method: Method) -> bool {
    match method {
        // Pure reads: the reply describes the session, and asking twice
        // describes it twice.
        Method::Ping
        | Method::SessionState
        | Method::SessionReport
        | Method::AgentReport
        | Method::AgentExplain
        | Method::PaneRead
        // State predicates (04 §2). The reason the CLI needs no server-side
        // wait persistence at all: a transition that fired while nothing was
        // connected is true at re-subscribe time, so a wait cannot miss it.
        | Method::Wait
        | Method::PaneWaitOutput => true,
        // Everything that types into a pane, spawns a process, moves the
        // layout or hands the session over. A connection that died after the
        // request was written cannot say whether it landed, and repeating any
        // of these would be doing it twice.
        Method::PaneSplit
        | Method::PaneClose
        | Method::PaneZoom
        | Method::PaneSwap
        | Method::PaneMove
        | Method::PaneResize
        | Method::PaneFocus
        | Method::PaneRename
        | Method::PaneRun
        | Method::PaneSendText
        | Method::PaneSendKeys
        | Method::WorkspaceCreate
        | Method::WorkspaceRename
        | Method::WorkspaceKill
        | Method::WorkspaceSwitch
        | Method::AgentStart
        | Method::AgentPrompt
        | Method::AgentNext
        | Method::SessionHandoff => false,
        // Connection-scoped, and a redial is a different connection: a binding
        // nothing reads and a subscription nothing consumes are not the call
        // the caller made. `amx events` redials with its own cursor instead
        // (`super::events`), and a bound stream belongs to the client that
        // bound it.
        Method::StreamBind | Method::PaneHistory | Method::ClientViewport
        | Method::EventsSubscribe => false,
    }
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
