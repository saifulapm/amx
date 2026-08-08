//! The rig M3's exit criterion runs on: a live session, an attached client, and
//! the two long-lived consumers that must not notice a swap.
//!
//! Split from the suite for the module budget, and along the seam that keeps
//! both readable: everything here is *scaffolding around a real session* — a
//! child process holding a standing call, a relay reading NDJSON, and the two
//! or three questions a test asks of the process table. The assertions live
//! next door.
//!
//! Nothing here models anything. The session is a real `amx server`, the
//! client is the real `amx attach` on a real pseudoterminal, the consumers are
//! the real `amx wait` and `amx events --json`, and the swap is the real
//! `amx session handoff` against the real binary.

#![allow(dead_code, reason = "the suite uses a subset of the rig")]

use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::process::{Child, Stdio};

use amx_core::PaneId;
use amx_core::WorkspaceId;
use rig::agent;
use rig::{Env, PATIENCE, Terminal, Wire, rasterize_styled, styled_run};
use serde_json::{Value, json};

use crate::fixtures::{Agent, Rig};

/// Run one exit test at a time.
///
/// Each of these tests is a whole session: a server, five agent panes on five
/// pseudoterminals, a real client at 200×50, two long-lived CLI children and a
/// process swap. `cargo test` runs a binary's tests on threads of one process,
/// and three of these at once measures the machine rather than amx — observed
/// as a sixty-second client-repaint timeout when the suite ran beside a
/// compile. They are serialized here rather than made more patient, because a
/// deadline that has to absorb two other copies of the same test is a deadline
/// that no longer means anything.
///
/// A tokio mutex rather than a std one: the guard is held across every await
/// in the test body, which is exactly what a std guard may not be, and a
/// panicking test leaves this one unpoisoned so the rest of the suite still
/// runs and still says what it found.
pub async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    ONE_AT_A_TIME.lock().await
}

/// A child `amx` whose output is captured to files under the environment root.
///
/// Files rather than pipes, for the reason the M3 client-reconnect suite gives:
/// a test that has to read a long-running child's output *while it runs* cannot
/// read a pipe without blocking on it, and polling a file is the same
/// observation without the deadlock.
#[derive(Debug)]
pub struct Standing {
    child: Child,
    out: PathBuf,
    err: PathBuf,
    what: String,
}

impl Standing {
    /// Start `args` in the background, capturing both streams.
    pub fn start(env: &Env, tag: &str, args: &[&str]) -> Self {
        let out = env.home().join(format!("{tag}.out"));
        let err = env.home().join(format!("{tag}.err"));
        let child = env
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).expect("create stdout")))
            .stderr(Stdio::from(File::create(&err).expect("create stderr")))
            .spawn()
            .expect("spawn amx");
        Self {
            child,
            out,
            err,
            what: format!("amx {}", args.join(" ")),
        }
    }

    /// Whether it is still running.
    pub fn running(&mut self) -> bool {
        self.child.try_wait().expect("try_wait").is_none()
    }

    /// What it has written to stdout so far.
    #[must_use]
    pub fn stdout(&self) -> String {
        std::fs::read_to_string(&self.out).unwrap_or_default()
    }

    /// What it has written to stderr so far.
    #[must_use]
    pub fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }

    /// Every complete line of stdout so far, as JSON.
    #[must_use]
    pub fn deliveries(&self) -> Vec<serde_json::Value> {
        parse(&self.stdout())
    }

    /// Wait until it exits, and answer with its status and both streams.
    pub fn finish(mut self) -> Finished {
        let mut done = None;
        {
            let (child, what, out, err) = (&mut self.child, &self.what, &self.out, &self.err);
            rig::wait_until_or(
                what,
                || {
                    done = child.try_wait().expect("try_wait");
                    done.is_some()
                },
                || {
                    format!(
                        "it has written {:?} and {:?}",
                        std::fs::read_to_string(out).unwrap_or_default(),
                        std::fs::read_to_string(err).unwrap_or_default(),
                    )
                },
            );
        }
        Finished {
            // Present: `wait_until_or` panics rather than return without it.
            code: done.and_then(|status| status.code()),
            stdout: self.stdout(),
            stderr: self.stderr(),
        }
    }

    /// Stop it and read what it wrote.
    pub fn stop(mut self) -> Finished {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Finished {
            code: None,
            stdout: self.stdout(),
            stderr: self.stderr(),
        }
    }
}

/// What a [`Standing`] child did.
#[derive(Debug)]
pub struct Finished {
    /// Its exit code, or `None` if it was killed.
    pub code: Option<i32>,
    /// Everything it wrote to stdout.
    pub stdout: String,
    /// Everything it wrote to stderr.
    pub stderr: String,
}

/// Parse NDJSON, dropping a partial last line.
///
/// The file is being appended to while it is read, so a half-written delivery
/// is a fact about the reader's timing rather than about the stream.
#[must_use]
pub fn parse(text: &str) -> Vec<serde_json::Value> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if !text.ends_with('\n') {
        lines.pop();
    }
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| panic!("not NDJSON: {line:?} ({err})"))
        })
        .collect()
}

/// A protocol connection to `env`'s session, reached through `amx _bridge`.
///
/// Tier 1 of D-M3-9, exactly as W11 built it: one end of a socketpair is the
/// bridge child's stdin and stdout, the other is an ordinary stream, and
/// nothing above it knows the peer is a subprocess. The child is leaked into
/// the environment's lifetime deliberately — it dies when its socket does, and
/// the test's business is the protocol rather than the process.
pub async fn bridged(env: &Env) -> Wire {
    let (mine, theirs) = StdUnixStream::pair().expect("socketpair");
    let stdin = OwnedFd::from(theirs.try_clone().expect("dup"));
    let stdout = OwnedFd::from(theirs);
    let child = env
        .command()
        .arg("_bridge")
        .arg("--session")
        .arg(&env.session)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn amx _bridge");
    std::mem::forget(child);
    mine.set_nonblocking(true).expect("non-blocking");
    Wire::over(tokio::net::UnixStream::from_std(mine).expect("adopt the socketpair"))
}

/// The sequence numbers an `amx events --json` relay has printed, in order.
///
/// A `gap` contributes its `to`, because that is what the consumer's cursor
/// becomes: the contract is gapless *or gap-marked*, and a gap is a delivery
/// like any other rather than a hole in the record.
#[must_use]
pub fn seqs(deliveries: &[serde_json::Value]) -> Vec<u64> {
    deliveries
        .iter()
        .filter_map(|delivery| match delivery["delivery"].as_str() {
            Some("event") => delivery["seq"].as_u64(),
            Some("gap") => delivery["to"].as_u64(),
            other => panic!("unknown delivery kind {other:?} in {delivery}"),
        })
        .collect()
}

/// Whether any delivery in the relay's record is a gap.
#[must_use]
pub fn gapped(deliveries: &[serde_json::Value]) -> bool {
    deliveries
        .iter()
        .any(|delivery| delivery["delivery"] == "gap")
}

// -------------------------------------------------- the session and its shape

/// Poll `session.report` until it names an attempt that reached `stage`.
pub async fn wait_report(rig: &mut Rig, stage: &str) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let report = rig.call("session.report", json!({})).await;
        if report["handoff"]["stage"] == stage {
            return report;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no handoff attempt reached {stage}; the report reads {report}",
        );
        rig::env::tick().await;
    }
}

/// A staged binary that is not an amx at all: refused at the pre-flight.
pub fn not_an_amx(rig: &Rig) -> String {
    let path = rig.env.home().join("not-amx");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write the fixture");
    make_executable(&path);
    path.to_string_lossy().into_owned()
}

/// A staged binary that passes the pre-flight and then never authenticates.
///
/// It answers `_handoff-caps` with this build's own window, so the pre-flight
/// accepts it and the session is frozen and captured; then it does nothing at
/// all, and the exporter's token stage times out into §3's abort.
pub fn a_silent_successor(rig: &Rig) -> String {
    let path = rig.env.home().join("silent-amx");
    let caps = serde_json::to_string(&json!({
        "version": "0.1.0",
        "handoff": [1, 1],
        "proto": [amx_proto::PROTO_MIN, amx_proto::PROTO_MAX],
    }))
    .expect("encode the capabilities");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = _handoff-caps ]; then printf '%s' '{caps}'; exit 0; fi\n\
             while : ; do read -r _ || exit 0; done\n"
        ),
    )
    .expect("write the fixture");
    make_executable(&path);
    path.to_string_lossy().into_owned()
}

pub fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fixture executable");
}

/// Five agents across three workspaces (§7 step 1).
pub async fn seed_three_workspaces(rig: &mut Rig) -> Vec<Agent> {
    let mut agents = Vec::new();
    let mut homes: Vec<WorkspaceId> = Vec::new();
    for (n, name) in crate::fixtures::NAMES.iter().enumerate() {
        // Three workspaces, five agents: the fourth and fifth join the first
        // and second, which is what makes "across three workspaces" a shape
        // rather than a count.
        let home = homes.get(n).copied().or_else(|| homes.get(n % 3).copied());
        let agent = rig.start_agent_in(name, home).await;
        if homes.len() < 3 {
            homes.push(agent.workspace);
        }
        agents.push(agent);
    }
    agents
}

/// Paint a distinctively styled sentinel in every pane (§7 step 1).
pub async fn paint_sentinels(rig: &mut Rig, agents: &[Agent]) {
    for a in agents {
        rig.drive(&a.name, &format!("{} {}", agent::SENTINEL, a.name))
            .await;
    }
    for a in agents {
        rig.wait_screen(a.pane, &sentinel_of(a)).await;
    }
}

/// The sentinel text one agent paints.
pub fn sentinel_of(a: &Agent) -> String {
    format!("{} {}", agent::SENTINEL_TEXT, a.name)
}

/// How many non-blank characters a terminal's output rasterizes to.
pub fn non_blank(bytes: &[u8]) -> usize {
    rasterize_styled(bytes)
        .values()
        .filter(|(c, _)| !c.is_whitespace())
        .count()
}

/// Wait until the client has repainted a sentinel, and answer with its cells.
pub fn wait_for_repaint(client: &mut Terminal, width: usize) -> Vec<(char, String)> {
    let mut last = Vec::new();
    client.wait_output("the client to repaint its sentinel", |bytes| {
        let painted = rasterize_styled(bytes);
        match styled_run(&painted, agent::SENTINEL_TEXT, width) {
            Some(run) => {
                last = run;
                true
            }
            None => false,
        }
    });
    last
}

/// A pane's history window — `(head, floor)` — as `session.state` reports it.
pub async fn history_window(rig: &mut Rig, pane: PaneId) -> (u64, u64) {
    let entry = rig.pane_state(pane).await;
    let field = |name: &str| {
        entry[name]
            .as_u64()
            .unwrap_or_else(|| panic!("pane {pane} has no {name}: {entry}"))
    };
    (field("history_head"), field("history_floor"))
}

/// Wait until a process that is not `previous` is serving `socket`.
pub async fn wait_for_successor(socket: &std::path::Path, previous: u32) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Ok(Some(pid)) = amx_server::session::probe::server_pid(socket).await
            && pid != previous
            && amx_server::session::probe::probe(socket).is_ok_and(|state| state.is_running())
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no successor took {} from pid {previous}",
            socket.display(),
        );
        rig::env::tick().await;
    }
}

/// Whether a pid is still a live process.
pub fn alive(pid: u32) -> bool {
    rustix::process::Pid::from_raw(pid.cast_signed())
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

pub fn sorted(pids: &[u32]) -> Vec<u32> {
    let mut copy = pids.to_vec();
    copy.sort_unstable();
    copy
}
