//! A session with a real `Core`, a real `AgentHub`, and a router wired to both.
//!
//! The suite drives [`amx_server::dispatch::handle`] rather than a socket, and
//! that is a deliberate choice with a reason worth writing down. `agent.start`
//! and `agent.prompt` need two things from their connection: the hub's mailbox
//! (for the registry) and the [`StatusView`] the hub writes (for the readiness
//! and prompt waits). When this file was written neither reached a connection —
//! `Gateway::bind` minted a `StatusView` of its own that no hub ever wrote, and
//! `Router::attach_agent` had no call site outside this file — so a
//! socket-level rig here would have been testing the wiring gap rather than the
//! verbs. The gap was nobody's file by construction, which made it V17's ("the
//! seams by exception", `docs/08-m2-plan.md` §6); **V17 closed it**, in
//! `session/serve.rs` and `actor/gateway.rs`, and `tests/agents.rs` is the
//! suite that drives these verbs over a real socket now. This harness stays
//! because it is the cheap place to test the verbs' own logic, and because it
//! wires exactly what the serve path wires.
//!
//! Everything below the router is the shipped thing: a real `Core` spawning
//! real children on real ptys, a real hub parsing a real registry override off
//! disk, real manifests compiled from a real file, and the fusion machine in
//! between. The only fakes are the agents themselves, and they are `/bin/sh`
//! scripts in the `MarkerShell` tradition `tests/drive.rs` established.
//!
//! The shell is pinned to `/bin/sh` for the reason that suite records: a
//! session on the defaults spawns the developer's own login shell behind every
//! seeded pane, which is slow, rude, and (for a zsh interrupted during startup)
//! leaves a stale lock on their real history file.

#![allow(dead_code, reason = "each test uses a subset of the harness")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amx_core::{Config, Ctx, PaneId, Scheduled};
use amx_proto::rpc::RpcError;
use amx_server::actor::agent_hub::AgentHub;
use amx_server::actor::core::Core;
use amx_server::actor::{AGENT_MAILBOX, AgentHandle, CoreHandle, StatusView};
use amx_server::conn::events::ConnEvents;
use amx_server::conn::resume::ConnResume;
use amx_server::conn::writer::{self, WriterQueue};
use amx_server::dispatch::Router;
use amx_server::runtime::Runtime;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::support::{PATIENCE, TICK, TempDir};

/// The fake agent every test starts: identified by its own basename.
pub const FAKE: &str = "fake";

/// A fake agent nothing can ever identify.
///
/// Its `executables` list names a program its `start` argv does not run, so the
/// argv guess misses, no hook ever reports, and the readiness handshake has
/// nothing to confirm — which is the only honest way to reach the timeout
/// branch without waiting on a real agent failing to boot.
pub const SHY: &str = "shy";

/// A stanza whose `start` argv runs a *different* agent's program.
///
/// The pane comes up, is identified, and reads idle — as something else. A
/// readiness that only asked "is this pane idle?" would call that ready; the
/// handshake asks whether the agent that came up is the one that was ordered.
pub const IMPOSTOR: &str = "impostor";

/// A fake agent whose stanza states a startup grace worth measuring.
pub const SLOW: &str = "slow";

/// That grace, in milliseconds.
///
/// Short enough not to slow the suite, long enough that a handshake answering
/// inside it is unambiguous rather than a scheduling artefact.
pub const SLOW_GRACE_MS: u64 = 400;

/// The marker the fake agents paint for each state.
pub const IDLE_MARK: &str = "IDLE>";
/// See [`IDLE_MARK`].
pub const WORKING_MARK: &str = "WORKING>";
/// See [`IDLE_MARK`].
pub const BLOCKED_MARK: &str = "BLOCKED>";

/// A session, its hub, and a router that can reach both.
pub struct Rig {
    /// The session context — its bus is what waits subscribe to.
    pub ctx: Ctx,
    /// The router every call in this suite goes through.
    pub router: Router,
    /// The fast read model, exactly as a wait predicate holds it.
    pub view: StatusView,
    dir: TempDir,
    runtime: Runtime,
    /// Kept alive so the `Core`'s receiver keeps reading the pinned shell.
    _config: watch::Sender<Config>,
    /// Kept alive so the connection's outbound queue has a far end.
    _queue: WriterQueue,
}

impl Rig {
    /// Start a session whose registry and manifests are this rig's own.
    ///
    /// The override files are written *before* the hub is constructed, because
    /// the hub parses both at assembly — which is the production path
    /// (D-M2-2's "the override merge is also the test seam") rather than the
    /// `with_registry` back door.
    pub async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let ctx = ctx_under(dir.path());
        plant_registry(&ctx.config_path, dir.path());
        plant_manifest(&ctx.config_path);

        let mut runtime = Runtime::new(ctx.clone());
        let mut pinned = Config::default();
        pinned.terminal.shell = Some("/bin/sh".to_owned());
        let (config_tx, config_rx) = watch::channel(pinned);

        let (core_tx, core_rx) = mpsc::channel(64);
        let (agent_tx, agent_rx) = mpsc::channel(AGENT_MAILBOX);
        let agent_events = ctx.bus.subscribe();
        let view = StatusView::new();

        let mut core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
        core.set_config(config_rx);
        core.set_agent(AgentHandle::new(agent_tx.clone()));

        let hub = AgentHub::new(ctx.clone(), CoreHandle::new(core_tx.clone()), view.clone());
        runtime.spawn("agent-hub", async move {
            let _agents = hub.run(agent_rx, agent_events).await;
        });
        runtime.spawn("core", async move {
            let _core = core.run(core_rx, |_: &Scheduled| {}).await;
        });

        let (outbound, queue) = writer::channel();
        let mut router = Router::new(CoreHandle::new(core_tx));
        router.attach_events(ConnEvents::new(
            Arc::clone(&ctx.bus),
            view.clone(),
            outbound,
            ctx.cancel.child_token(),
            // No hello reached this rig, so there is nothing to resume from —
            // the same state a first attach's connection carries.
            ConnResume::none(),
        ));
        router.attach_agent(AgentHandle::new(agent_tx));

        let mut rig = Self {
            ctx,
            router,
            view,
            dir,
            runtime,
            _config: config_tx,
            _queue: queue,
        };
        // The session needs one workspace with one live pane before an agent
        // can be split off anything. `workspace.create` is the wire verb that
        // makes one, shell and all, through the same spawn path.
        rig.call("workspace.create", json!({ "focus": true })).await;
        rig
    }

    /// Make one call through the real decode path, expecting it to succeed.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        match amx_server::dispatch::handle(&mut self.router, method, Some(params)).await {
            Ok(value) => value,
            Err(err) => panic!("{method} failed: {} ({})", err.message, err.code),
        }
    }

    /// Make one call, expecting it to fail, and answer with the error.
    pub async fn call_err(&mut self, method: &str, params: Value) -> RpcError {
        match amx_server::dispatch::handle(&mut self.router, method, Some(params)).await {
            Err(err) => err,
            Ok(value) => panic!("{method} unexpectedly succeeded: {value}"),
        }
    }

    /// The session's state tree.
    pub async fn state(&mut self) -> Value {
        self.call("session.state", json!({})).await
    }

    /// One pane's entry in the state tree.
    pub async fn pane_state(&mut self, pane: PaneId) -> Value {
        let state = self.state().await;
        state["panes"]
            .as_array()
            .expect("panes")
            .iter()
            .find(|entry| entry["pane"] == pane.to_string())
            .cloned()
            .unwrap_or_else(|| panic!("no pane {pane} in the state tree"))
    }

    /// The pane's visible grid, as `pane.read` serves it.
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

    /// Wait until the pane's visible grid shows `needle`, and answer with it.
    ///
    /// A spawned child paints on its own schedule, so "the pane is running its
    /// program" is a fact that arrives rather than one that is true when a verb
    /// returns — and a verb that answered `timed_out` on its own deadline has
    /// said nothing at all about the pty.
    pub async fn wait_screen(&mut self, pane: PaneId, needle: &str) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let screen = self.screen(pane).await;
            if screen.iter().any(|row| row.contains(needle)) {
                return screen;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "pane {pane} never painted {needle:?}; its screen is {screen:?}"
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Wait until `pane`'s entry in `session.state` carries an agent, and
    /// answer with the entry.
    ///
    /// The barrier `agent.start` does not give a caller. Readiness is decided
    /// against the hub's [`StatusView`] — the fast read model, which a wait
    /// predicate holds directly — while `session.state`'s `agent` block is
    /// `Core`'s **mirror** of that view, posted with an un-awaited `try_send`
    /// (`docs/08-m2-plan.md` §3). So a `ready` reply says the agent is up; it
    /// does not say the state tree knows yet, and the state tree is what both
    /// `session.state` readers and `Scope::Agent` addressing consult. A read
    /// taken straight after the reply comes back carrying the pane's label and
    /// no `agent` at all.
    ///
    /// The entry, not a unit: a caller that waited and then read again would
    /// have two reads to reason about instead of one.
    pub async fn wait_pane_agent(&mut self, pane: PaneId) -> Value {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let entry = self.pane_state(pane).await;
            if entry["agent"]["kind"].is_string() {
                return entry;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "pane {pane} never reached the state tree as an agent; it reads {entry}"
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Start an agent by name and wait until that name can address it.
    ///
    /// The two halves a test means by "there is an agent called `name`": the
    /// verb's own reply, and the state tree naming it — see
    /// [`Rig::wait_pane_agent`] for why the second does not follow from the
    /// first. Addressing by label resolves in `Scope::Agent`, which admits only
    /// panes whose *mirrored* status carries a kind, so an `agent.prompt` sent
    /// on the strength of the start reply alone is refused with "no agent is
    /// running in it" — about the pane that start just brought up.
    pub async fn start_agent(&mut self, name: &str, kind: &str) -> (Value, PaneId) {
        let reply = self
            .call("agent.start", json!({ "name": name, "kind": kind }))
            .await;
        let pane: PaneId = reply["pane"]
            .as_str()
            .unwrap_or_else(|| panic!("pane is a pane id: {reply}"))
            .parse()
            .expect("a pane id");
        self.wait_pane_agent(pane).await;
        (reply, pane)
    }

    /// Wait until the pane's mirrored agent status reads `state`.
    ///
    /// A condition plus a deadline, never a nap: the deadline's expiry is the
    /// failure path, which is the shape `tests/hygiene.rs` enforces.
    pub async fn wait_status(&mut self, pane: PaneId, want: &str) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let seen = self.view.get(pane).map(|status| status.state.to_string());
            if seen.as_deref() == Some(want) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "pane {pane} never reached {want}; it is {seen:?} and its screen is {:?}",
                self.screen(pane).await
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Where a fake agent records what was typed at it.
    pub fn recording(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Where this rig's registry stanzas expect their programs to be.
    ///
    /// A test plants its scripts here *after* the rig starts, which is fine and
    /// deliberate: the stanza only carries the path, and nothing runs it until
    /// an `agent.start` asks for it.
    pub fn scripts(&self) -> &Path {
        self.dir.path()
    }

    /// Wait until `path` holds at least `want` bytes, and answer with them.
    pub async fn recorded(&self, path: &Path, want: usize) -> Vec<u8> {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let bytes = std::fs::read(path).unwrap_or_default();
            if bytes.len() >= want {
                return bytes;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the agent recorded {} of {want} bytes: {:?}",
                bytes.len(),
                String::from_utf8_lossy(&bytes)
            );
            tokio::time::sleep(TICK).await;
        }
    }

    /// Cancel the session and join every task.
    ///
    /// The router goes first, and it has to: a cancelled hub drains its mailbox
    /// *to closure* before it returns (`docs/08-m2-plan.md` §3), and this
    /// router holds an `AgentHandle` — a live sender — so a shutdown that kept
    /// it would wait for a mailbox that can never close. In a real session the
    /// clones belong to `Core` and to connection tasks, all of which end on the
    /// same cancellation; here the test is the connection.
    pub async fn stop(self) {
        let Self {
            ctx,
            router,
            runtime,
            ..
        } = self;
        ctx.cancel.cancel();
        drop(router);
        runtime.shutdown().await;
    }
}

// ----------------------------------------------------------- the fake agents

/// A fake agent that paints an idle prompt, then blocks after one prompt.
///
/// `dd bs=1 count=N` and not `read`, for the reason `tests/drive.rs` records:
/// `pane.run` submits with `\r`, and a terminal in raw mode does not translate
/// it, so a shell `read` would wait for a newline that never comes. Counting
/// bytes is what a fake agent has instead of a line discipline.
pub fn blocks_after(prompt: usize, record: &Path) -> String {
    format!(
        "stty raw -echo; printf '{IDLE_MARK}\\r\\n'; \
         dd bs=1 count={prompt} of={} 2>/dev/null; \
         printf '{WORKING_MARK}\\r\\n'; printf '{BLOCKED_MARK}\\r\\n'; sleep 300",
        record.display()
    )
}

/// A fake agent that is blocked from the moment it starts and stays that way.
///
/// It reads nothing and paints nothing further, so a `--wait blocked` sent to
/// it can only be satisfied by the block it was *already* in — which is the
/// thing the floor exists to refuse.
pub fn blocked_from_the_start() -> String {
    format!("stty raw -echo; printf '{BLOCKED_MARK}\\r\\n'; sleep 300")
}

/// A fake agent that sits at its prompt and does nothing else.
pub fn idles() -> String {
    format!("stty raw -echo; printf '{IDLE_MARK}\\r\\n'; sleep 300")
}

/// Write the scripts this rig's registry stanzas point at.
///
/// The stanza's `start` argv must be the *program*, since tier 3 identifies an
/// agent by the basename of what it spawned — so each script is its own file
/// named after the agent rather than an argument to `sh -c`.
pub fn plant_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the fake agent");
    make_executable(&path);
    path
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)
        .expect("stat the fake agent")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod the fake agent");
}

// -------------------------------------------------------------- the overrides

/// A `Ctx` whose paths all live under `root`.
fn ctx_under(root: &Path) -> Ctx {
    crate::support::ctx_under(root)
}

/// Plant the registry override the suite's agents live in.
///
/// Two stanzas and one grace of zero: the grace is the window in which the
/// fusion machine refuses to read the screen, and a fake agent that paints a
/// settled screen on its first line has nothing to wait out. The stanzas that
/// need a grace ask for one by name (`slow`), because that is the field V01 §6
/// made per-agent.
fn plant_registry(config_path: &Path, scripts: &Path) {
    let dir = config_path.parent().expect("a config directory");
    std::fs::create_dir_all(dir).expect("create the config directory");
    // Each stanza's executable is its own id, and each script is named after
    // it: tier 3 identifies a pane by the *basename* of what `Core` spawned, so
    // two stanzas sharing one program would be two stanzas amx could never tell
    // apart — the first one in file order would answer for both.
    let text = format!(
        r#"
[[agent]]
id          = "{FAKE}"
aliases     = ["faux"]
label       = "Fake Agent"
executables = ["{FAKE}"]
coverage    = "edges"
start       = ["{fake}"]
resume      = ["{fake}", "--resume", "{{ref}}"]
ref_kind    = "id"
manifest    = "fake.toml"
startup_grace_ms = 0
hook_events = ["SessionStart", "Stop"]

[[agent]]
id          = "{SLOW}"
label       = "Slow Fake"
executables = ["{SLOW}"]
coverage    = "edges"
start       = ["{slow}"]
manifest    = "fake.toml"
startup_grace_ms = {SLOW_GRACE_MS}
hook_events = ["SessionStart", "Stop"]

[[agent]]
id          = "{SHY}"
label       = "Unidentifiable Fake"
executables = ["a-program-this-stanza-never-starts"]
coverage    = "edges"
start       = ["{shy}"]
manifest    = "fake.toml"
startup_grace_ms = 0
hook_events = ["SessionStart", "Stop"]

[[agent]]
id          = "{IMPOSTOR}"
label       = "Impostor"
executables = ["{IMPOSTOR}"]
coverage    = "edges"
start       = ["{fake}"]
manifest    = "fake.toml"
startup_grace_ms = 0
hook_events = ["SessionStart", "Stop"]
"#,
        fake = scripts.join(FAKE).display(),
        slow = scripts.join(SLOW).display(),
        shy = scripts.join(SHY).display(),
    );
    std::fs::write(dir.join("agents.toml"), text).expect("write the registry override");
}

/// Plant the manifest the fake agents' markers are read by.
///
/// `bottom_non_empty_lines(1)` throughout: the fakes paint one marker per
/// transition and leave the earlier ones on the screen above, so a wider region
/// would match a state the agent has already left — which is the bug herdr's
/// changelog records against `whole_recent` and stale "esc to interrupt" text.
fn plant_manifest(config_path: &Path) {
    let dir = config_path.with_file_name("manifests");
    std::fs::create_dir_all(&dir).expect("create the manifest directory");
    let text = format!(
        r#"
id      = "fake"
version = "test"

[[rule]]
name     = "blocked_marker"
state    = "blocked"
priority = 900
region   = "bottom_non_empty_lines(1)"
contains = ["{BLOCKED_MARK}"]

[[rule]]
name     = "working_marker"
state    = "working"
priority = 800
region   = "bottom_non_empty_lines(1)"
contains = ["{WORKING_MARK}"]

[[rule]]
name         = "idle_marker"
state        = "idle"
priority     = 700
region       = "bottom_non_empty_lines(1)"
visible_idle = true
contains     = ["{IDLE_MARK}"]
"#
    );
    std::fs::write(dir.join("fake.toml"), text).expect("write the manifest override");
}
