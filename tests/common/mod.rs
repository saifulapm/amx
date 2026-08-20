//! The end-to-end harness: a throwaway tmux server, a state directory of its
//! own, and a stand-in for the vendor.
//!
//! Every test that drives amx end to end runs against real tmux and real
//! panes. Nothing here fakes the multiplexer: a fake would prove that amx
//! agrees with a fake.
//!
//! Two things are pinned for every process the harness starts: `AMX_STATE_DIR`
//! and `HOME`. Without both, a test reads the records and the configuration of
//! whoever is running it.

// Each test binary uses the part of the harness it needs.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The amx this suite drives, built by cargo alongside it.
pub const AMX: &str = env!("CARGO_BIN_EXE_amx");

/// How long a poll waits before it gives up and says what it wanted.
const PATIENCE: Duration = Duration::from_secs(20);

pub struct Harness {
    state: TempDir,
    home: TempDir,
    /// The tmux socket name this harness owns.
    socket: String,
}

impl Harness {
    pub fn new() -> Harness {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Harness {
            state: TempDir::new().expect("a state directory"),
            home: TempDir::new().expect("a home directory"),
            socket: format!(
                "amx-e2e-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }

    pub fn state_root(&self) -> PathBuf {
        self.state.path().join("agents")
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    pub fn agent_dir(&self, id: &str) -> PathBuf {
        self.state_root().join(id)
    }

    // ── amx ──────────────────────────────────────────────────────────────────

    /// Run amx, with the machine it reads pinned to this harness.
    pub fn amx(&self, args: &[&str]) -> Output {
        self.amx_command(args).output().expect("running amx")
    }

    pub fn amx_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(AMX);
        command
            .args(args)
            .env("AMX_STATE_DIR", self.state.path())
            .env("HOME", self.home.path());
        command
    }

    // ── tmux ─────────────────────────────────────────────────────────────────

    /// Run one tmux command against this harness's own server.
    pub fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.socket, "-f", "/dev/null"])
            .args(args)
            .env("AMX_STATE_DIR", self.state.path())
            .env("HOME", self.home.path())
            .output()
            .expect("running tmux");
        assert!(
            out.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// What is on a pane's screen now.
    pub fn capture(&self, pane: &str) -> String {
        self.tmux(&["capture-pane", "-p", "-J", "-t", pane])
    }

    pub fn pane_alive(&self, pane: &str) -> bool {
        Command::new("tmux")
            .args(["-L", &self.socket, "list-panes", "-a", "-F", "#{pane_id}"])
            .output()
            .is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .any(|line| line == pane)
            })
    }

    // ── agents ───────────────────────────────────────────────────────────────

    /// Start an agent playing `scenario`, and answer with its pane.
    ///
    /// The pane waits for the record to exist before it starts the vendor, so
    /// the first hook has somewhere to go. amx's own `new` writes the record
    /// first for the same reason.
    pub fn play(&self, id: &str, scenario: &str) -> String {
        let pane = self.tmux(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "sh",
            "-c",
            &self.pane_script(id, scenario),
        ]);
        self.record(id, &pane);
        pane
    }

    /// The record amx's own `new` would have written.
    pub fn record(&self, id: &str, pane: &str) {
        let dir = self.agent_dir(id);
        std::fs::create_dir_all(&dir).expect("the agent's directory");
        write(
            &dir.join("meta.json"),
            &json!({
                "id": id,
                "task": "fix the login bug",
                "dir": self.home.path(),
                "socket": { "name": self.socket },
                "pane": pane,
                "created": 1,
            }),
        );
        write(&dir.join("state.json"), &json!({ "state": "starting" }));
    }

    /// What the record says now.
    pub fn state(&self, id: &str) -> Value {
        read(&self.agent_dir(id).join("state.json")).unwrap_or_else(|| json!({}))
    }

    pub fn meta(&self, id: &str) -> Value {
        read(&self.agent_dir(id).join("meta.json")).unwrap_or_else(|| json!({}))
    }

    /// Everything that has happened to the agent, oldest first.
    pub fn events(&self, id: &str) -> Vec<Value> {
        let path = self.agent_dir(id).join("events.jsonl");
        std::fs::read_to_string(path)
            .map(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The names of the events recorded, in order.
    pub fn event_kinds(&self, id: &str) -> Vec<String> {
        self.events(id)
            .iter()
            .filter_map(|event| event["kind"].as_str().map(str::to_string))
            .collect()
    }

    /// Wait until the record says this, or say what it said instead.
    pub fn until_state(&self, id: &str, want: &str) -> Value {
        self.until(&format!("{id} to be {want}"), || {
            let state = self.state(id);
            (state["state"] == want).then_some(state)
        })
    }

    /// Poll until `f` has an answer. Polling rather than sleeping: a fixed
    /// wait is either slower than the machine or shorter than a bad day.
    pub fn until<T>(&self, what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(answer) = f() {
                return answer;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // ── the fixture ──────────────────────────────────────────────────────────

    pub fn scenario(&self, name: &str) -> PathBuf {
        fixtures()
            .join("scenarios")
            .join(format!("{name}.scenario"))
    }

    pub fn transcript(&self, id: &str) -> PathBuf {
        self.home.path().join(format!("transcript-{id}.jsonl"))
    }

    /// The command a harness pane runs: the vendor's stand-in, and amx
    /// recording how it ended.
    fn pane_script(&self, id: &str, scenario: &str) -> String {
        let state = self.state.path().display();
        let home = self.home.path().display();
        let mock = fixtures().join("mock-claude");
        let scenario = self.scenario(scenario);
        let transcript = self.transcript(id);

        format!(
            "export AMX_ID={id} AMX_STATE_DIR='{state}' HOME='{home}' AMX_BIN='{AMX}' \
             MOCK_CLAUDE_SCENARIO='{scenario}' MOCK_CLAUDE_TRANSCRIPT='{transcript}'; \
             while [ ! -f \"$AMX_STATE_DIR/agents/$AMX_ID/meta.json\" ]; do sleep 0.01; done; \
             '{mock}'; '{AMX}' _exit {id} $?",
            scenario = scenario.display(),
            transcript = transcript.display(),
            mock = mock.display(),
        )
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // The server, and every pane on it, go with the test that made them.
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
    }
}

impl Default for Harness {
    fn default() -> Self {
        Harness::new()
    }
}

/// Where the vendor's stand-in and its scenarios live.
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_claude")
}

fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).expect("json"))
        .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}
