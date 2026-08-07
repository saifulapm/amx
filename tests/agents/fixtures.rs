//! One session, five stand-in agents, and the questions asked of them.
//!
//! Everything here talks to a **real `amx server` process over its real
//! socket**. That is the whole point of the suite: `crates/amx-server/tests/`
//! assembles a session in-process, which is the right place to test an actor
//! and the wrong place to notice that the actor was never handed to a
//! connection. V17 found exactly that — the hub's mailbox and its `StatusView`
//! stopped at the gateway — and a rig one process away is what would have
//! caught it a wave earlier.
//!
//! The fake agents are `rig::agent`'s; its module documentation is the honest
//! account of which parts of them stand in.

#![allow(dead_code, reason = "each module of the suite drives a subset")]

use std::time::{Duration, Instant};

use amx_core::{PaneId, WorkspaceId};
use rig::agent::{self, FakeAgents};
use rig::{Env, ServerChild, Wire, result_of};
use serde_json::{Value, json};

/// The rows the exit suite's client attaches with.
///
/// Deliberately larger than the persistence suite's 24×80: five agents mean
/// five panes, the shipped Claude rules read `bottom_non_empty_lines(5)` for
/// the prompt box, and one of the anchors that box is matched by is a rule
/// twenty columns wide. A pane too narrow to hold one would fail this suite for
/// a reason that has nothing to do with amx.
pub const ROWS: u16 = 40;
/// See [`ROWS`].
pub const COLS: u16 = 120;

/// The five agents the M2 exit criterion names.
pub const NAMES: [&str; 5] = ["a1", "a2", "a3", "a4", "a5"];

/// A running session with the scripted agents installed.
pub struct Rig {
    /// The isolated machine. Public so a test can restart the server on it.
    pub env: Env,
    /// The scripted agents' scripts and recordings.
    pub agents: FakeAgents,
    /// The control connection every call in a test goes through.
    pub wire: Wire,
    server: Option<ServerChild>,
}

impl Rig {
    /// Install the agents, start the server, and connect.
    pub async fn start(tag: &str) -> Self {
        let mut env = Env::new(tag);
        let agents = FakeAgents::install(&mut env);
        let server = env.server();
        let wire = connected(&env).await;
        Self {
            env,
            agents,
            wire,
            server: Some(server),
        }
    }

    /// Make one call, expecting it to succeed.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let reply = self.wire.request(method, params).await;
        match &reply.outcome {
            amx_proto::RpcOutcome::Result(value) => value.clone(),
            amx_proto::RpcOutcome::Error(err) => {
                panic!("{method} failed: {} ({})", err.message, err.code)
            }
        }
    }

    /// Make one call, expecting it to fail, and answer with the error message.
    pub async fn call_err(&mut self, method: &str, params: Value) -> String {
        let reply = self.wire.request(method, params).await;
        match &reply.outcome {
            amx_proto::RpcOutcome::Error(err) => err.message.clone(),
            amx_proto::RpcOutcome::Result(value) => {
                panic!("{method} unexpectedly succeeded: {value}")
            }
        }
    }

    /// Start one scripted agent under `name`, in its own workspace.
    ///
    /// Its own workspace, and not five panes of one, because the exit criterion
    /// is about five agents a user *switches between* — `agent.next` answering
    /// with a workspace is only exercised when the head is somewhere else — and
    /// because a fifth of a terminal is not a width the shipped rules were
    /// written for.
    pub async fn start_agent(&mut self, name: &str) -> Agent {
        let created = self
            .call("workspace.create", json!({ "focus": true }))
            .await;
        let workspace: WorkspaceId =
            serde_json::from_value(created["workspace"].clone()).expect("a workspace id");
        let reply = self
            .call(
                "agent.start",
                json!({ "name": name, "kind": agent::KIND, "workspace": workspace }),
            )
            .await;
        assert_eq!(
            reply["readiness"], "ready",
            "the scripted agent did not come up ready: {reply}"
        );
        let pane: PaneId = serde_json::from_value(reply["pane"].clone()).expect("a pane id");
        // `agent.start` can answer `ready` off the tracker's *opening* reading
        // — an identified pane whose activity is silence is assumed idle, and a
        // stanza with no startup grace has no window in which that assumption
        // is corrected before the reply is written. That is a real looseness in
        // the handshake (it is recorded in `docs/08-m2-plan.md` §6's wave
        // outcomes), and it is not this suite's subject: every test below wants
        // an agent that has actually *painted*, so waiting for the paint is
        // the precondition rather than something to assert around.
        self.wait_screen(pane, agent::IDLE_TEXT).await;
        Agent {
            name: name.to_owned(),
            pane,
            workspace,
        }
    }

    /// Wait until a pane's visible grid shows `needle`.
    pub async fn wait_screen(&mut self, pane: PaneId, needle: &str) {
        let deadline = Instant::now() + rig::PATIENCE;
        loop {
            let screen = self.screen(pane).await;
            if screen.iter().any(|row| row.contains(needle)) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "pane {pane} never painted {needle:?}; its screen is:\n{}",
                screen.join("\n")
            );
            rig::env::tick().await;
        }
    }

    /// The durable snapshot, once one has been written.
    pub fn snapshot(&self) -> Option<Value> {
        let bytes = std::fs::read(self.env.state_dir().join("session.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Type one command word at a scripted agent, through `pane.run`.
    ///
    /// The same verb `agent.prompt` submits with, and the same one a user's
    /// script would: a `pane.run` is bracketed-paste-aware text plus the
    /// submit byte, and the agent's `read` is on the far side of a real pty's
    /// line discipline.
    pub async fn drive(&mut self, target: &str, command: &str) {
        self.call("pane.run", json!({ "target": target, "text": command }))
            .await;
    }

    /// The session's state tree.
    pub async fn state(&mut self) -> Value {
        self.call("session.state", json!({})).await
    }

    /// One pane's entry in the state tree.
    pub async fn pane_state(&mut self, pane: PaneId) -> Value {
        let state = self.state().await;
        pane_entry(&state, pane)
    }

    /// One pane's fused agent status, if it has one.
    pub async fn status(&mut self, pane: PaneId) -> Option<String> {
        let entry = self.pane_state(pane).await;
        entry["agent"]["state"].as_str().map(str::to_owned)
    }

    /// The bus sequence of a pane's last status transition.
    pub async fn transition_seq(&mut self, pane: PaneId) -> u64 {
        let entry = self.pane_state(pane).await;
        entry["agent"]["transition_seq"]
            .as_u64()
            .unwrap_or_else(|| panic!("pane {pane} has no transition seq: {entry}"))
    }

    /// The attention queue, in queue order, as `session.state` carries it.
    ///
    /// Absent rather than empty on a session with nothing waiting — the field
    /// is additive and skipped when empty, so a pre-M2 peer never sees it — and
    /// an empty vector is the right reading of that.
    pub async fn attention(&mut self) -> Vec<PaneId> {
        let state = self.state().await;
        state["attention"]
            .as_array()
            .map(|queue| {
                queue
                    .iter()
                    .map(|value| {
                        serde_json::from_value(value.clone())
                            .expect("an attention entry is a pane id")
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Ask the agent for permission and wait until the dialog is really up and
    /// the pane is really in the queue.
    ///
    /// Three halves, because they are three different facts arriving in a fixed
    /// order: `PermissionRequest` enters `Blocked` 8–14 ms *before* the dialog
    /// paints (V01 §3 M3), so a test that only waited for the status would be
    /// reading a screen from before the block; and the hub's *enqueue* is a
    /// third arrival again, so a caller that blocked two agents back to back on
    /// the status alone was choosing the pane order and letting the runner
    /// choose the queue order. On a loaded macOS runner it chose the other one.
    ///
    /// Waiting on the queue is what makes "block order" a fact this rig
    /// established rather than one it hoped for — and it is the same queue the
    /// assertions read, so nothing here waits on a proxy.
    pub async fn block(&mut self, target: &Agent) {
        self.drive(&target.name, agent::ASK).await;
        self.wait_status(target.pane, "blocked").await;
        self.wait_screen(target.pane, agent::BLOCKED_TEXT).await;
        self.wait_queued(target.pane).await;
    }

    /// Wait until `pane` has reached the attention queue.
    pub async fn wait_queued(&mut self, pane: PaneId) {
        let deadline = Instant::now() + rig::PATIENCE;
        loop {
            let queue = self.attention().await;
            if queue.contains(&pane) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "pane {pane} blocked but never reached the attention queue; it holds {queue:?}"
            );
            rig::env::tick().await;
        }
    }

    /// Wait until `pane`'s status reads `want`, failing with what it read.
    pub async fn wait_status(&mut self, pane: PaneId, want: &str) {
        let deadline = Instant::now() + rig::PATIENCE;
        loop {
            let seen = self.status(pane).await;
            if seen.as_deref() == Some(want) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "pane {pane} never reached {want}; it reads {seen:?} and its screen is:\n{}",
                self.screen(pane).await.join("\n")
            );
            rig::env::tick().await;
        }
    }

    /// Wait until a predicate over the session state holds.
    pub async fn wait_state(&mut self, what: &str, mut done: impl FnMut(&Value) -> bool) -> Value {
        let deadline = Instant::now() + rig::PATIENCE;
        loop {
            let state = self.state().await;
            if done(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; the session reads {state}"
            );
            rig::env::tick().await;
        }
    }

    /// A pane's visible grid, as `pane.read` serves it.
    pub async fn screen(&mut self, pane: PaneId) -> Vec<String> {
        let reply = self
            .call("pane.read", json!({ "target": pane.to_string() }))
            .await;
        reply["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row.as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// Wait until the durable snapshot names every one of `refs`.
    ///
    /// The capture is debounced, and a restart before it lands would test the
    /// debounce rather than the resume. Reading the file the server writes —
    /// from outside the server — is the only honest way to know it has.
    pub async fn wait_snapshot_holds(&mut self, refs: &[String]) {
        let deadline = Instant::now() + rig::PATIENCE;
        loop {
            let bytes =
                std::fs::read(self.env.state_dir().join("session.json")).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if refs.iter().all(|value| text.contains(value.as_str())) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the snapshot never recorded every conversation; it holds:\n{text}"
            );
            rig::env::tick().await;
        }
    }

    /// Wait until `want` restored panes have recorded what they were resumed
    /// with, and answer with the refs.
    pub async fn wait_resumed(&self, want: usize) -> Vec<String> {
        let deadline = Instant::now() + rig::PATIENCE;
        loop {
            let seen = self.agents.resumed();
            if seen.len() >= want {
                return seen;
            }
            assert!(
                Instant::now() < deadline,
                "{} of {want} conversations were typed back: {seen:?}",
                seen.len()
            );
            rig::env::tick().await;
        }
    }

    /// Stop the server the way `amx session stop` does, and assert it went
    /// down cleanly inside `budget`.
    ///
    /// Bounded rather than patient (R-M1-2): a wedged drain must cost one
    /// diagnostic, not a minute of silence, and the message carries what every
    /// thread was doing when the budget expired.
    pub fn shutdown_within(&mut self, budget: Duration) {
        let server = self.server.take().expect("the server is still running");
        if let Err(stuck) = server.shutdown_within(budget) {
            panic!("{stuck}");
        }
    }

    /// Start a fresh server on the same state, and reconnect.
    ///
    /// The restart the exit criterion is about: same runtime and state roots,
    /// same registry override, a new process.
    pub async fn restart(&mut self) {
        self.server = Some(self.env.server());
        self.wire = connected(&self.env).await;
    }
}

/// One scripted agent in the session.
#[derive(Clone, Debug)]
pub struct Agent {
    /// Its name, which is its pane's label (D-M2-9).
    pub name: String,
    /// The pane it runs in.
    pub pane: PaneId,
    /// The workspace that pane was created in.
    pub workspace: WorkspaceId,
}

impl Agent {
    /// The conversation its hooks report.
    pub fn conversation(&self) -> String {
        FakeAgents::conversation(&self.pane.to_string())
    }
}

/// A control connection that has said hello.
pub async fn connected(env: &Env) -> Wire {
    let mut wire = Wire::connect(&env.socket()).await;
    wire.hello((amx_proto::PROTO_MIN, amx_proto::PROTO_MAX))
        .await;
    wire
}

/// One pane's entry in a state tree already read.
pub fn pane_entry(state: &Value, pane: PaneId) -> Value {
    state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .find(|entry| entry["pane"] == pane.to_string())
        .cloned()
        .unwrap_or_else(|| panic!("no pane {pane} in the state tree: {state}"))
}

/// Read a pane id out of a reply field.
pub fn pane_of(reply: &Value, field: &str) -> PaneId {
    serde_json::from_value(reply[field].clone())
        .unwrap_or_else(|_| panic!("{field} is a pane id: {reply}"))
}

/// The result of a raw wire response, for the calls a test makes off `Rig`.
pub fn ok(response: &amx_proto::Response) -> &Value {
    result_of(response)
}
