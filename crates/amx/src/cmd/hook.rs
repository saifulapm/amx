//! `amx _hook <agent>` — the agent hook emitter.
//!
//! D-M2-4: "read the agent's hook payload from stdin, map it to a single
//! `agent.report` control call over the session socket, exit 0 no matter what."
//!
//! The whole design is a reaction to one herdr failure: its hooks are sh plus a
//! python heredoc, they silently no-op when `python3` is missing, and
//! `integration status` still reports `current` because it only greps a version
//! comment. amx's emitter *is* the binary that already speaks the protocol, so
//! the failure mode is deleted rather than detected. (herdr's own Windows asset
//! already shells to the herdr CLI, which is the strongest evidence the POSIX
//! heredoc was never necessary.)
//!
//! The call sequence, and its budget: connect → Hello/Welcome → one request →
//! read reply → exit, under a total of about 500 ms. **Every** failure — no
//! socket, refused, timed out, an error reply, a server mid-shutdown — is a
//! silent success from the agent's point of view, because a hook must never
//! break or slow a turn, and because agents surface hook noise to the user.
//!
//! Identity comes from the environment V07 injects into every pane child:
//! `AMX_SOCKET`, `AMX_PANE_ID`, `AMX_HOOK_TOKEN`. V01 §3 M6 measured that chain
//! reaching hook processes on both agents — all 185 invocations in the run
//! carried all six variables verbatim, through an interactive shell — so the
//! scheme stands as designed and the degraded `session_id`-plus-process-tree
//! branch is not needed.
//!
//! Forward, do not filter. The emitter tags subagent scope from `agent_id` and
//! sends everything else through; the fusion machine owns the policy. herdr
//! baked its filtering into the installed scripts and paid for it by needing a
//! reinstall on every machine to change a rule.
//!
//! # The shape of the code
//!
//! Every step below answers with an `Option` and the whole sequence is one
//! `?`-chain under one timeout, because the contract has exactly one failure
//! behavior and no diagnostics: there is nothing for a typed error to carry
//! that anybody would ever read. [`run`] returns [`ExitCode::SUCCESS`] on both
//! branches of that chain, and it is the only place that decides so.
//!
//! # Task ownership
//!
//! **V09** filled [`run`], together with `dispatch/agent.rs`'s report arm and
//! `crates/amx/tests/hook.rs`.
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use amx_client::net::{self, Session};
use amx_core::agent::{AgentKind, HookToken, RefSource};
use amx_core::platform::pane_env;
use amx_core::{Env, PaneId};
use amx_proto::control::Method;
use amx_proto::control::agent::{HookEvent, ReportParams, ReportScope};
use clap::ArgMatches;
use serde_json::Value;
use tokio::io::AsyncReadExt as _;

use crate::cmd::attach::client_info;

/// Everything the emitter is allowed to spend, start to finish.
///
/// D-M2-4's "~500 ms" as a number. It covers the payload read as well as the
/// wire, because a hook that sits waiting on a stdin nobody is going to close
/// costs the agent's turn exactly as much as one waiting on a socket nobody is
/// going to answer. V01 §3 M9 measured the room this leaves: dispatch to hook
/// process start is 26 ms at the median, and everything after it here is a
/// connect and one round trip on a local socket.
const BUDGET: Duration = Duration::from_millis(500);

/// The most payload the emitter will read from stdin.
///
/// A `PermissionRequest` carries the whole `tool_input` — for `Bash`, an
/// arbitrarily long command — so the cap is generous. It exists so a hook wired
/// to something that streams cannot make the emitter grow without bound inside
/// its own budget.
const MAX_PAYLOAD: u64 = 1 << 20;

/// The clap id of the positional naming which agent this hook was installed
/// for.
const AGENT: &str = "agent";

/// Forward one hook payload, and exit 0 whatever happens.
///
/// The signature promises the contract: there is no `Result`, because there is
/// no failure this command reports.
///
/// `env` is unused on purpose. It carries the session-selection variables
/// [`Env::from_process`] reads once at startup (04 §2's W9 rule), and a hook
/// process needs a different set entirely: the pane identity V07 injects, read
/// by [`PaneIdentity::from_process`] and passed down as a value from there. A
/// hook must reach the server *its pane* belongs to, which `AMX_SOCKET` names
/// outright — re-deriving it from `$XDG_RUNTIME_DIR` would aim a hook at the
/// terminal's runtime directory, which need not be the server's.
pub async fn run(env: &Env, matches: &ArgMatches) -> ExitCode {
    let _ = env;
    // The timeout's own result is discarded with everything else: expired and
    // failed are the same outcome, and that outcome is silence.
    let _ = tokio::time::timeout(BUDGET, forward(matches)).await;
    ExitCode::SUCCESS
}

/// One payload, one report, one call.
///
/// `None` at any step is the end of it — a missing variable, a payload that is
/// not JSON, a socket nobody answers. The emitter filters nothing *but*
/// malformed input (D-M2-4): every judgement about what an event means belongs
/// to the fusion machine, so that changing one ships a binary instead of
/// reinstalling hooks on every machine.
async fn forward(matches: &ArgMatches) -> Option<()> {
    let kind = AgentKind::new(matches.get_one::<String>(AGENT)?.as_str()).ok()?;
    let identity = PaneIdentity::from_process()?;
    let payload: Value = serde_json::from_slice(&read_payload().await?).ok()?;
    let params = report(kind, &identity, &payload)?;
    send(&identity.socket, params).await
}

/// Where a hook process is, according to the environment it inherited.
///
/// The three variables of D-M2-4's identity scheme that the emitter cannot work
/// without. The other three [`pane_env`] names — the marker, the session name,
/// the workspace — are for programs *inside* a pane to read; a report addresses
/// a pane, on a socket, with a token, and nothing else.
#[derive(Debug)]
struct PaneIdentity {
    /// `AMX_SOCKET`: the session socket to report on.
    socket: PathBuf,
    /// `AMX_PANE_ID`: the pane the hook believes it is running in.
    pane: PaneId,
    /// `AMX_HOOK_TOKEN`: that pane's spawn token.
    token: HookToken,
}

impl PaneIdentity {
    /// Read it out of the environment.
    ///
    /// `None` means this hook is not running under amx — a config left behind
    /// in a project the user opened outside a pane, most often — and the right
    /// response to that is to do nothing at all, quickly.
    fn from_process() -> Option<Self> {
        Some(Self {
            socket: PathBuf::from(var(pane_env::SOCKET)?),
            pane: var(pane_env::PANE)?.parse().ok()?,
            token: HookToken::new(var(pane_env::TOKEN)?),
        })
    }
}

/// One environment variable, treating unset and empty alike.
///
/// The same rule `Env::from_process` applies, for the same reason: an
/// exported-but-empty variable is a shell artifact, not a value.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Everything the emitter can read from stdin, capped.
async fn read_payload() -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    tokio::io::stdin()
        .take(MAX_PAYLOAD)
        .read_to_end(&mut payload)
        .await
        .ok()?;
    Some(payload)
}

/// The report `payload` describes, tagged with what the emitter knows.
///
/// The one field that is a judgement rather than a copy is
/// [`ReportScope`](amx_proto::control::agent::ReportScope), and it is a
/// *tag*, not a filter: V01 §3 M4 measured `agent_id` present on every
/// subagent-scoped event and absent from every parent one, so the emitter says
/// which it saw and the fusion machine decides what that means. An emitter that
/// dropped `SubagentStop` here would be herdr's installed-policy mistake in a
/// new language.
///
/// `None` only when the payload does not name its own event, which is the
/// definition of malformed for this surface.
fn report(kind: AgentKind, identity: &PaneIdentity, payload: &Value) -> Option<ReportParams> {
    let source = RefSource::for_agent(&kind);
    Some(ReportParams {
        pane: identity.pane,
        token: identity.token.clone(),
        agent: kind,
        source,
        event: HookEvent::from(text(payload, "hook_event_name")?.to_owned()),
        seq: now_nanos(),
        scope: scope_of(payload),
        session_id: text(payload, "session_id").map(str::to_owned),
        transcript_path: text(payload, "transcript_path").map(PathBuf::from),
        start_source: text(payload, "source").map(str::to_owned),
        tool: text(payload, "tool_name").map(str::to_owned),
        notification: text(payload, "notification_type").map(str::to_owned),
    })
}

/// Whose turn `payload` is about.
///
/// The anonymous `SubagentStop` V01 §3 M4 recorded arriving two seconds after
/// the parent's `Stop` on essentially every tool-using turn carries an
/// `agent_id` and an *empty* `agent_type`, which is why the type is present and
/// the type name is optional.
fn scope_of(payload: &Value) -> ReportScope {
    match text(payload, "agent_id") {
        None => ReportScope::Parent,
        Some(agent_id) => ReportScope::Subagent {
            agent_id: agent_id.to_owned(),
            agent_type: text(payload, "agent_type").map(str::to_owned),
        },
    }
}

/// One string field of a hook payload, present and non-empty.
fn text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key)?.as_str().filter(|value| !value.is_empty())
}

/// Nanoseconds since the epoch: the emitter's own ordering stamp.
///
/// Nanoseconds and not milliseconds because hooks subscribed to one event run
/// in parallel and their processes start a median of 1.0 ms apart, with a
/// measured minimum of 0.1 ms (V01 §3 M9). A millisecond counter would tie, and
/// a tie is an ordering the server would have to guess at.
///
/// A clock before the epoch, or one far enough past it to overflow, yields a
/// stamp rather than an error: the field orders reports that arrive together,
/// and a machine whose clock is nonsense has already lost that ordering.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// Connect, negotiate, call, and go.
///
/// `attach: false` — a hook is a one-shot verb, and a verb connection must
/// never mutate a session by connecting (04 §1). The method name comes off the
/// shared table rather than being spelled here, so a renamed row is a compile
/// error instead of a hook that reports into nothing.
async fn send(socket: &Path, params: ReportParams) -> Option<()> {
    let params = serde_json::to_value(params).ok()?;
    let stream = net::connect(socket).await.ok()?;
    let (mut session, _welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .ok()?;
    // The reply is read — the budget covers a round trip, and a call whose
    // answer nobody waits for is a call that can be lost in a socket buffer —
    // and then discarded, because there is nothing the emitter would do
    // differently for any value it could carry.
    session
        .call(Method::AgentReport.wire_name(), params)
        .await
        .ok()?;
    Some(())
}
