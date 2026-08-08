//! V09 acceptance: `amx _hook` and the `agent.report` path into the hub.
//!
//! The emitter's contract is *silence*, which makes it the easiest thing in the
//! tree to fake passing: a command that does nothing at all satisfies "exit 0
//! and print nothing" perfectly. So every test here runs the real binary
//! against a real Unix socket and a real `agent.report` dispatch, and asserts
//! on what arrived in the hub's mailbox — the only evidence that tells
//! "silently succeeded" from "silently did nothing".
//!
//! The hub itself is V08's, a wave-mate. What stands in for it here is its
//! mailbox and an ack: [`AgentCommand::HookReport`] is a frozen V02 contract,
//! so a recorder reading that channel observes exactly what the real hub will.
//!
//! Payload shapes are copied from V01's recordings
//! (`docs/notes/hook-coverage.md` §2), not invented — R-M2-14's rule that
//! fixtures anchor to the measurement holds for unit fixtures as much as for
//! V17's fake agents.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use amx_core::agent::HookToken;
use amx_core::{PaneId, SessionId};
use amx_proto::control::agent::{HookEvent, ReportParams, ReportReply, ReportScope};
use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, FrameHeader};
use amx_proto::hello::{Hello, ServerInfo};
use amx_proto::rpc::{Request, Response, RpcError};
use amx_server::actor::{AgentCommand, AgentHandle, CoreHandle};
use amx_server::dispatch::Router;
use amx_server::session::probe::probe;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

mod support;

use support::{Env, Output, TempDir, wait_until};

/// The most an invocation of the emitter may take, whatever goes wrong.
///
/// Generously above the emitter's own 500 ms budget, because the number under
/// test is "does not hang", not "is fast in nanoseconds" — the budget itself is
/// asserted by the case that talks to a listener which never answers.
const PATIENCE: Duration = Duration::from_secs(5);

/// A `PermissionRequest` as V01 recorded it (§2, "Payload shapes, as
/// recorded"), which is the payload that matters most: it is the one edge the
/// fusion machine may trust outright.
fn permission_request() -> Value {
    json!({
        "session_id": "c9a3c73b-b184-4871-8e98-79b46b87b635",
        "transcript_path": "/home/s/.claude/projects/amx/c9a3c73b.jsonl",
        "cwd": "/home/s/src/amx",
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "tool_input": {"command": "echo spike-permission-probe"},
        "permission_mode": "default",
    })
}

// ------------------------------------------------------------ the happy path

#[tokio::test(flavor = "multi_thread")]
async fn hook_reads_claude_payload_and_sends_one_agent_report() {
    let hub = Hub::listening("payload", Answer::Dispatch).await;
    let env = Env::new("hook-payload");
    let pane = PaneId::new_v4();

    let out = hub
        .emit(&env, pane, "b7f0c1a4d9e25638", &permission_request())
        .await;
    assert_silent(&out);

    let reports = hub.reports();
    assert_eq!(reports.len(), 1, "one payload, one report: {reports:?}");
    let report = &reports[0];
    assert_eq!(report.agent.as_str(), "claude");
    assert_eq!(report.source.as_str(), "amx:claude");
    assert_eq!(report.event, HookEvent::PermissionRequest);
    assert_eq!(report.tool.as_deref(), Some("Bash"));
    assert_eq!(
        report.session_id.as_deref(),
        Some("c9a3c73b-b184-4871-8e98-79b46b87b635"),
    );
    assert_eq!(
        report.transcript_path.as_deref(),
        Some(Path::new("/home/s/.claude/projects/amx/c9a3c73b.jsonl")),
    );
    assert_eq!(report.scope, ReportScope::Parent);

    // Nanoseconds since the epoch, not milliseconds: parallel hooks for one
    // event start 1.0 ms apart at the median and 0.1 ms apart at the minimum
    // (V01 §3 M9), so a coarser stamp would tie.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_nanos();
    let skew = now.saturating_sub(u128::from(report.seq));
    assert!(
        report.seq > 0 && skew < 60_000_000_000,
        "the emitter stamps nanoseconds now, got {} (skew {skew} ns)",
        report.seq,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn report_reaches_the_hub_with_pane_and_token_intact() {
    let hub = Hub::listening("identity", Answer::Dispatch).await;
    let env = Env::new("hook-identity");
    let pane = PaneId::new_v4();
    let token = "5b6c0e1f-a-token-nobody-guesses";

    let out = hub.emit(&env, pane, token, &permission_request()).await;
    assert_silent(&out);

    // The report the *mailbox* carried, addressed and tokened: `AgentHub` reads
    // `HookReport { pane, token, .. }` to decide whether this pane is tracked
    // and whether the token matches the one it minted at spawn (D-M2-4's
    // misattribution guard). Both are asserted against the mailbox fields as
    // well as the payload's, because the hub reads the former.
    let addressed = hub.addressed();
    assert_eq!(addressed.len(), 1, "one report reached the mailbox");
    assert_eq!(addressed[0].0, pane, "the pane the environment named");
    assert_eq!(addressed[0].1, HookToken::new(token), "the spawn token");

    let report = hub.reports().remove(0);
    assert_eq!(report.pane, pane);
    assert_eq!(report.token, HookToken::new(token));
}

#[tokio::test(flavor = "multi_thread")]
async fn subagent_scope_is_tagged_from_agent_id_not_filtered() {
    let hub = Hub::listening("subagent", Answer::Dispatch).await;
    let env = Env::new("hook-subagent");
    let pane = PaneId::new_v4();

    // The hazard V01 §3 M4 measured: an anonymous `SubagentStop`, with an
    // `agent_id` no `SubagentStart` announced and an empty `agent_type`,
    // arriving ~2 s after the parent's `Stop` on essentially every tool-using
    // turn. herdr's scripts dropped it at the source and needed a reinstall to
    // change that; amx forwards it tagged and lets the fusion machine rule.
    let anonymous = json!({
        "session_id": "c9a3c73b-b184-4871-8e98-79b46b87b635",
        "hook_event_name": "SubagentStop",
        "agent_id": "a7d7604b5dfd35b1e",
        "agent_type": "",
        "stop_hook_active": false,
    });
    assert_silent(&hub.emit(&env, pane, "token", &anonymous).await);

    // …and the parent's own `Stop`, which carries no `agent_id` at all. Across
    // V01's whole run every `Stop` lacked one and every `SubagentStop` had one,
    // which is the entire discriminator.
    let parent = json!({
        "session_id": "c9a3c73b-b184-4871-8e98-79b46b87b635",
        "hook_event_name": "Stop",
        "stop_hook_active": false,
    });
    assert_silent(&hub.emit(&env, pane, "token", &parent).await);

    let reports = hub.reports();
    assert_eq!(
        reports.len(),
        2,
        "both forwarded, neither filtered: {reports:?}"
    );
    assert_eq!(reports[0].event, HookEvent::SubagentStop);
    assert_eq!(
        reports[0].scope,
        ReportScope::Subagent {
            agent_id: "a7d7604b5dfd35b1e".to_owned(),
            // Empty in the payload, absent on the wire: an anonymous stop names
            // no type, and "" is not a type.
            agent_type: None,
        },
    );
    assert_eq!(reports[1].event, HookEvent::Stop);
    assert_eq!(reports[1].scope, ReportScope::Parent);
}

// ------------------------------------------------------- the miserable paths

#[tokio::test(flavor = "multi_thread")]
async fn hook_exits_zero_and_fast_with_no_socket_no_env_or_dead_server() {
    let env = Env::new("hook-misery");
    let dir = TempDir::new("hook-misery-sockets");
    let pane = PaneId::new_v4();
    let payload = permission_request();

    // No `AMX_*` at all: a hook config left behind in a project the user opened
    // outside a pane. It must cost nothing and say nothing.
    let (out, took) = run_hook(&env, &[], &payload).await;
    assert_silent(&out);
    assert!(took < PATIENCE, "a hook with no environment took {took:?}");

    // A socket path that does not exist: the session was deleted under the
    // agent, or the variable is stale.
    let absent = dir.path().join("no-such-socket");
    let (out, took) = run_hook(&env, &planted(&absent, pane, "token"), &payload).await;
    assert_silent(&out);
    assert!(took < PATIENCE, "an absent socket took {took:?}");

    // A socket file with nobody behind it: the stale-socket case `amx`'s own
    // probe knows, reached here without a probe — a hook has no budget for one.
    let dead = dir.path().join("dead-socket");
    std::fs::write(&dead, b"not a socket").expect("plant a dead socket");
    let (out, took) = run_hook(&env, &planted(&dead, pane, "token"), &payload).await;
    assert_silent(&out);
    assert!(took < PATIENCE, "a dead socket took {took:?}");

    // A server that accepts and then says nothing — a session mid-shutdown,
    // which is the case a budget rather than a probe is what saves you from.
    let mute = Hub::listening("mute", Answer::Silence).await;
    let (out, took) = run_hook(&env, &planted(&mute.socket, pane, "token"), &payload).await;
    assert_silent(&out);
    assert!(
        took < PATIENCE,
        "a server that never answers held the hook for {took:?}; the emitter's \
         budget is what bounds an agent's turn",
    );
}

/// An emitter that is gone before the payload is written is still a pass.
///
/// DR-19's `_hook` self-race, made deterministic: with no environment the
/// emitter answers `None` before it reads stdin, so the caller's write races the
/// child's exit and loses whenever the child is quick. Here it always loses —
/// the payload is offered *after* the process has been reaped — which is the
/// same `BrokenPipe` the flake produced, arriving every time instead of once in
/// 115 runs. The contract under test is the emitter's exit and its silence;
/// whether anybody was left to read the payload is not part of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_that_exits_before_its_payload_is_written_is_still_silent() {
    let env = Env::new("hook-epipe");
    let text = permission_request().to_string();

    let out = tokio::task::spawn_blocking(move || {
        let mut child = env
            .command()
            .args(["_hook", "claude", "--marker", "1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn amx _hook");
        let stdin = child.stdin.take().expect("a piped stdin");
        // Waited for, not slept on: the write below has to happen after the
        // read end is really closed, and only the exit says that it is.
        let status = child.wait().expect("wait for amx _hook");
        offer_payload(stdin, text.as_bytes());
        let out = child.wait_with_output().expect("collect the output");
        Output {
            code: status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    })
    .await
    .expect("run the emitter");

    assert_silent(&out);
}

#[tokio::test(flavor = "multi_thread")]
async fn hook_never_writes_to_stdout_or_stderr_on_failure() {
    let env = Env::new("hook-quiet");
    let dir = TempDir::new("hook-quiet-sockets");
    let pane = PaneId::new_v4();

    // Malformed input — the one thing the emitter does filter.
    let absent = dir.path().join("no-such-socket");
    let vars = planted(&absent, pane, "token");
    assert_silent(&run_hook(&env, &vars, &json!("not an object")).await.0);
    assert_silent(&run_hook(&env, &vars, &json!({"cwd": "/tmp"})).await.0);

    // A server that answers every call with an error. An agent surfaces hook
    // output to the user, so a failed report has to be as quiet as a refused
    // connection.
    let refusing = Hub::listening("refusing", Answer::Refuse).await;
    let vars = planted(&refusing.socket, pane, "token");
    assert_silent(&run_hook(&env, &vars, &permission_request()).await.0);
}

// ------------------------------------------------- the arm, over a real server

#[tokio::test(flavor = "multi_thread")]
async fn agent_report_acks_over_a_live_server_with_no_tracked_pane() {
    let env = Env::new("hook-live");
    let mut server = env.spawn(&["server"]);
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });

    let params = json!({
        "pane": PaneId::new_v4(),
        "token": "a-token-for-a-pane-nothing-tracks",
        "agent": "claude",
        "source": "amx:claude",
        "event": "Stop",
        "seq": 1_754_524_800_123_456_789_u64,
    });
    let out = env.run(&["agent", "report", "--params", &params.to_string()]);
    let reply: Value = serde_json::from_str(out.ok()).expect("the reply is JSON");

    // Not an error, and not `NOT_IMPLEMENTED`: the arm is wired, and this
    // build's session has no `AgentHub` to track a pane with (V08's, a
    // wave-mate), which is precisely what `accepted: false` says.
    assert_eq!(reply["accepted"], json!(false), "{reply}");
    assert!(
        reply["seq"].is_u64(),
        "the ack carries the bus head: {reply}"
    );

    env.run(&["session", "stop"]).ok();
    let _ = server.wait();
}

// ------------------------------------------------------------------ the rig

/// What a recording server does with a control call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    /// Route it through the real dispatch table, with the recorder's mailbox
    /// attached as the `AgentHub`.
    Dispatch,
    /// Answer every call with an error.
    Refuse,
    /// Accept the connection and say nothing at all, ever.
    Silence,
}

/// A session socket with a stand-in `AgentHub` behind it.
struct Hub {
    /// Where it listens.
    socket: PathBuf,
    /// What reached the mailbox, in order.
    seen: Arc<Mutex<Vec<(PaneId, HookToken, ReportParams)>>>,
    /// Kept so the socket outlives the test.
    _dir: TempDir,
}

impl Hub {
    /// Bind a socket and start answering on it.
    async fn listening(tag: &str, answer: Answer) -> Self {
        let dir = TempDir::new(tag);
        let socket = dir.path().join("sock");
        let listener = UnixListener::bind(&socket).expect("bind the session socket");
        let seen = Arc::new(Mutex::new(Vec::new()));

        let (tx, mut rx) = mpsc::channel(16);
        let recorder = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Some(AgentCommand::HookReport {
                pane,
                token,
                report,
                reply,
            }) = rx.recv().await
            {
                recorder.lock().unwrap().push((pane, token, *report));
                let _ = reply.send(Ok(ReportReply {
                    accepted: true,
                    seq: 7,
                }));
            }
        });

        let hub = AgentHandle::new(tx);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, hub.clone(), answer));
            }
        });

        Self {
            socket,
            seen,
            _dir: dir,
        }
    }

    /// Run the emitter against this socket, as a pane child would.
    async fn emit(&self, env: &Env, pane: PaneId, token: &str, payload: &Value) -> Output {
        run_hook(env, &planted(&self.socket, pane, token), payload)
            .await
            .0
    }

    /// The reports the mailbox received.
    fn reports(&self) -> Vec<ReportParams> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, report)| report.clone())
            .collect()
    }

    /// The pane and token each report was *addressed* with, which is what the
    /// hub reads before it looks at the report at all.
    fn addressed(&self) -> Vec<(PaneId, HookToken)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(pane, token, _)| (*pane, token.clone()))
            .collect()
    }
}

/// One connection: `Hello`/`Welcome`, then calls until the peer goes away.
async fn serve(mut stream: UnixStream, hub: AgentHandle, answer: Answer) {
    if answer == Answer::Silence {
        // Held open, unanswered, until the emitter gives up on its own budget.
        return std::future::pending().await;
    }

    let Some(frame) = read_frame(&mut stream).await else {
        return;
    };
    let hello: Hello = serde_json::from_slice(&frame).expect("a hello");
    let welcome = hello
        .accept(
            ServerInfo {
                name: "amx-hook-test".to_owned(),
                version: "0".to_owned(),
            },
            &BTreeSet::new(),
            0,
            SessionId::new_v4(),
        )
        .expect("negotiate");
    write_frame(&mut stream, &serde_json::to_vec(&welcome).unwrap()).await;

    // The real table, so a report that decodes here decodes in a real session.
    // The `Core` mailbox is never read: with a hub attached, `agent.report`
    // never asks `Core` anything.
    let (core_tx, _core_rx) = mpsc::channel(1);
    let mut router = Router::new(CoreHandle::new(core_tx));
    router.attach_agent(hub);

    while let Some(frame) = read_frame(&mut stream).await {
        let request: Request = serde_json::from_slice(&frame).expect("a request");
        let response = match answer {
            Answer::Refuse => Response::err(request.id, RpcError::new(-1, "no")),
            _ => match amx_server::dispatch::handle(&mut router, &request.method, request.params)
                .await
            {
                Ok(value) => Response::ok(request.id, value),
                Err(error) => Response::err(request.id, error),
            },
        };
        write_frame(&mut stream, &serde_json::to_vec(&response).unwrap()).await;
    }
}

/// One control frame's payload, or `None` when the peer closed.
async fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    stream.read_exact(&mut header).await.ok()?;
    let header = FrameHeader::decode(header).ok()?;
    let mut payload = vec![0_u8; header.payload_len()];
    stream.read_exact(&mut payload).await.ok()?;
    Some(payload)
}

/// Write one control frame.
async fn write_frame(stream: &mut UnixStream, payload: &[u8]) {
    let len = u32::try_from(payload.len()).expect("a small frame");
    let header = FrameHeader::new(len, CONTROL_CHANNEL);
    let _ = stream.write_all(&header.encode()).await;
    let _ = stream.write_all(payload).await;
}

/// The environment V07 injects into a pane child, as a hook process sees it.
fn planted(socket: &Path, pane: PaneId, token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("AMX_ENV", "1".to_owned()),
        ("AMX_SOCKET", socket.to_string_lossy().into_owned()),
        ("AMX_PANE_ID", pane.to_string()),
        ("AMX_HOOK_TOKEN", token.to_owned()),
    ]
}

/// Run `amx _hook claude` with `env_vars` planted and `payload` on stdin, and
/// say how long it took.
///
/// The child runs on a blocking thread so the recording server — a task on this
/// same runtime — keeps answering while the emitter talks to it.
async fn run_hook(env: &Env, env_vars: &[(&str, String)], payload: &Value) -> (Output, Duration) {
    let mut command = env.command();
    command
        .args(["_hook", "claude", "--marker", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env_vars {
        command.env(name, value);
    }
    let text = payload.to_string();

    tokio::task::spawn_blocking(move || {
        let start = Instant::now();
        let mut child = command.spawn().expect("spawn amx _hook");
        offer_payload(child.stdin.take().expect("a piped stdin"), text.as_bytes());
        let out = child.wait_with_output().expect("wait for amx _hook");
        let elapsed = start.elapsed();
        (
            Output {
                code: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            elapsed,
        )
    })
    .await
    .expect("run the emitter")
}

/// Write `payload` to a hook's stdin, tolerating a hook that has already gone.
///
/// The emitter's fastest contract is to exit without reading stdin at all: with
/// no `AMX_SOCKET` in the environment, `PaneIdentity::from_process` answers
/// `None` before `read_payload` is ever called (`crates/amx/src/cmd/hook.rs`),
/// so a child that wins the race closes the read end and this write returns
/// `BrokenPipe`. Panicking on that fails the test for the emitter doing exactly
/// what the test is asserting it does, and gets more likely the faster the
/// emitter gets — DR-19's `_hook` self-race, seen once in 115 runs under 16–20
/// hogs. A refused payload is an outcome the caller is entitled to see, so it is
/// dropped and the child's own exit and silence stay the assertion. Every other
/// error is still a real failure of the harness.
fn offer_payload(mut stdin: std::process::ChildStdin, payload: &[u8]) {
    match stdin.write_all(payload) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(err) => panic!("write the payload: {err}"),
    }
}

/// The whole visible contract: exit 0, and not a byte anywhere.
fn assert_silent(out: &Output) {
    assert_eq!(out.code, Some(0), "a hook must never fail a turn: {out:?}");
    assert!(out.stdout.is_empty(), "wrote to stdout: {out:?}");
    assert!(out.stderr.is_empty(), "wrote to stderr: {out:?}");
}
