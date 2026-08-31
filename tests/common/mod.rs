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

    /// Run amx with something typed at it.
    pub fn amx_with_input(&self, args: &[&str], typed: &str) -> Output {
        use std::io::Write;
        use std::process::Stdio;

        let mut child = self
            .amx_command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("running amx");
        child
            .stdin
            .take()
            .expect("stdin was asked for")
            .write_all(typed.as_bytes())
            .expect("typing at amx");
        child.wait_with_output().expect("waiting for amx")
    }

    pub fn amx_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(AMX);
        command
            .args(args)
            .env("AMX_STATE_DIR", self.state.path())
            .env("HOME", self.home.path())
            // A machine with XDG_CONFIG_HOME set would otherwise hand the test
            // the developer's own config, however carefully HOME was pinned.
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            // amx's own tmux server, pinned to this harness: a test must not
            // reach the developer's agents, and must not be reached by them.
            .env("AMX_TMUX_SOCKET", &self.socket)
            // Whether the suite itself is being run from inside tmux is not a
            // test's business; the tests that care say so themselves.
            .env_remove("TMUX")
            .env_remove("TMUX_PANE");
        command
    }

    /// Run amx in a pane of this harness's server, and answer with the pane.
    ///
    /// Some of what amx answers to is not on its command line at all: whether
    /// anybody is looking at a terminal, and whether that terminal is already
    /// inside tmux. A pane is a real terminal, so this is the only way to ask
    /// those questions honestly.
    ///
    /// A variable given an empty value is unset rather than set — the pane is
    /// inside tmux by birth, and a test that wants a terminal outside one says
    /// so by clearing tmux's own two.
    ///
    /// The terminal opens in this harness's own home. Where a terminal is
    /// matters — anything it starts starts there — and inheriting the
    /// directory the suite was run from would put a test's agents in the
    /// developer's own repository.
    pub fn in_a_terminal(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let config = self.home.path().join(".config");
        let mut pairs = vec![
            ("AMX_STATE_DIR", self.state.path().to_string_lossy()),
            ("HOME", self.home.path().to_string_lossy()),
            ("XDG_CONFIG_HOME", config.to_string_lossy()),
            ("AMX_TMUX_SOCKET", self.socket.as_str().into()),
        ];
        pairs.extend(env.iter().map(|(name, value)| (*name, (*value).into())));

        // Every `-u` first: `env` reads its own flags only until the first
        // assignment.
        let mut line = String::from("exec env");
        for (name, _) in pairs.iter().filter(|(_, value)| value.is_empty()) {
            line.push_str(&format!(" -u {name}"));
        }
        for (name, value) in pairs.iter().filter(|(_, value)| !value.is_empty()) {
            line.push_str(&format!(" {name}={}", quoted(value)));
        }
        line.push_str(&format!(" {}", quoted(AMX)));
        for arg in args {
            line.push_str(&format!(" {}", quoted(arg)));
        }

        self.tmux(&[
            "new-session",
            "-d",
            "-c",
            &self.home.path().to_string_lossy(),
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "sh",
            "-c",
            &line,
        ])
    }

    /// The environment a person who is already inside tmux would have.
    pub fn inside_tmux(&self) -> Vec<(String, String)> {
        let pane = self.tmux(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "sh",
            "-c",
            "while :; do sleep 0.05; done",
        ]);
        let socket = self.tmux(&["display-message", "-p", "-t", &pane, "#{socket_path}"]);
        let pid = self.tmux(&["display-message", "-p", "-t", &pane, "#{pid}"]);
        vec![
            ("TMUX".to_string(), format!("{socket},{pid},0")),
            ("TMUX_PANE".to_string(), pane),
        ]
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

    /// Put the record where the test needs it — an agent that has not been
    /// heard from for an hour, without an hour of waiting.
    pub fn set_state(&self, id: &str, state: Value) {
        write(&self.agent_dir(id).join("state.json"), &state);
    }

    /// The pane the record names.
    pub fn pane_of(&self, id: &str) -> String {
        self.meta(id)["pane"]
            .as_str()
            .unwrap_or_else(|| panic!("no pane recorded for {id}"))
            .to_string()
    }

    pub fn meta(&self, id: &str) -> Value {
        read(&self.agent_dir(id).join("meta.json")).unwrap_or_else(|| json!({}))
    }

    /// What the pane was handed at birth: its environment, its command, and
    /// the task.
    pub fn handoff(&self, id: &str) -> Value {
        read(&self.agent_dir(id).join("handoff.json"))
            .unwrap_or_else(|| panic!("no handoff for {id}"))
    }

    /// Write this harness's config file.
    pub fn config(&self, text: &str) {
        let path = self.home.path().join(".config/amx/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).expect("the config directory");
        std::fs::write(&path, text).expect("writing the config");
    }

    /// A git repository with one commit in it, for the agents that want a
    /// worktree.
    pub fn a_repo(&self) -> PathBuf {
        let repo = self.home.path().join("repo");
        std::fs::create_dir_all(&repo).expect("the repository");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "amx tests")
                .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
                .env("GIT_COMMITTER_NAME", "amx tests")
                .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
                .output()
                .expect("running git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "amx tests"]);
        git(&["config", "user.email", "tests@example.invalid"]);
        std::fs::write(repo.join("README.md"), "before\n").expect("a file to commit");
        git(&["add", "README.md"]);
        git(&["commit", "-m", "first"]);
        repo
    }

    /// The vendor's stand-in, as a command line.
    pub fn mock(&self) -> String {
        fixtures()
            .join("mock-claude")
            .to_string_lossy()
            .into_owned()
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
        // And so does the socket. tmux does not always unlink it -- a server
        // that was already gone leaves it behind -- and repeated suite runs
        // piled up thousands of dead sockets until /tmp/tmux-1000 itself made
        // new servers time out (friction #G40BJA0X).
        let _ = std::fs::remove_file(socket_dir().join(&self.socket));
    }
}

/// Where `tmux -L <name>` keeps its sockets: `$TMUX_TMPDIR`, else
/// `/tmp/tmux-<uid>`, the same rule tmux applies.
fn socket_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TMUX_TMPDIR") {
        return PathBuf::from(dir);
    }
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
    PathBuf::from(format!("/tmp/tmux-{uid}"))
}

impl Default for Harness {
    fn default() -> Self {
        Harness::new()
    }
}

/// The card, opened on the agent the view is holding the cursor over.
///
/// Waited for by the mark that closes it, which is the one mark the card draws
/// whatever shape it is drawn in: a box has it in the corner and a spine has it
/// at the foot of the rule. What a test that opens a card is about is what is
/// on the card, and waiting on the frame around it would pin every one of them
/// to a drawing that is not theirs.
pub fn card_on(amx: &Harness, view: &str, id: &str) -> String {
    amx.until("the row", || amx.capture(view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", view, "Space"]);
    amx.until("the card", || {
        let drawn = amx.capture(view);
        drawn
            .lines()
            .any(|line| line.trim_start().starts_with('╰'))
            .then_some(drawn)
    })
}

/// Where the vendor's stand-in and its scenarios live.
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_claude")
}

/// A word a shell reads as one word, whatever is in it.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).expect("json"))
        .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}
