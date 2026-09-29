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
            // test's business; the tests that care say so themselves. Nor is
            // whether it is being run from inside an agent's pane: parentage
            // is read off `AMX_ID`, and a suite run from an amx pane must not
            // turn every spawn in it into a child.
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("AMX_ID")
            .env_remove("AMX_PARENT")
            .env_remove("AMX_PARENT_DIR")
            .env_remove("AMX_DEPTH");
        leaks_nothing(&mut command);
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

    /// Run one tmux command against this harness's own server, asking again
    /// if the server went while it was being asked.
    ///
    /// A test that waits for its agents to finish empties this server — the
    /// panes exit, the sessions go with them — and a server on its way out
    /// still holds its socket for a moment. A command arriving inside that
    /// moment is told `server exited unexpectedly`, which under parallel
    /// suites failed the wall's ctrl+x test about six runs in twenty-four.
    /// Nothing was half-done, and the next client starts a fresh server, so
    /// the question is worth asking again. Any other failure is still this
    /// harness's to shout about.
    pub fn tmux(&self, args: &[&str]) -> String {
        let mut out = self.tmux_once(args);
        if !out.status.success() && the_server_went(&out.stderr) {
            out = self.tmux_once(args);
        }
        assert!(
            out.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn tmux_once(&self, args: &[&str]) -> std::process::Output {
        let mut command = Command::new("tmux");
        command
            .args(["-L", &self.socket, "-f", "/dev/null"])
            .args(args)
            .env("AMX_STATE_DIR", self.state.path())
            .env("HOME", self.home.path());
        // The server this starts is where every pane of the test gets its
        // environment from.
        leaks_nothing(&mut command);
        command.output().expect("running tmux")
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
    ///
    /// The pane is stamped with the id first, the way `amx adopt` stamps a
    /// pane it took over. amx puts the panes it opens in a session called
    /// `amx-<id>` and reads that name to tell whose pane a pane is; these
    /// panes are made by hand, in sessions tmux named after itself, so the
    /// stamp is what makes this one answer for this agent. Without it the
    /// reader calls the agent gone, which is true of a pane that answers for
    /// nobody and is not what these tests are about.
    ///
    /// A record naming a pane that is not there — `%404`, `%99` — is left
    /// naming one, because a record of a gone pane is what those tests came
    /// for.
    pub fn record(&self, id: &str, pane: &str) {
        if self.pane_alive(pane) {
            // amx's own pane option, spelled here because a test binary has no
            // way to read a constant out of the binary it drives.
            self.tmux(&["set-option", "-p", "-t", pane, "@amx-id", id]);
        }
        let dir = self.agent_dir(id);
        std::fs::create_dir_all(&dir).expect("the agent's directory");
        write(
            &dir.join("meta.json"),
            &json!({
                "id": id,
                "task": "fix the login bug",
                "agent": "claude",
                "dir": self.home.path(),
                "socket": { "name": self.socket },
                "pane": pane,
                "created": 1,
            }),
        );
        write(&dir.join("state.json"), &json!({ "state": "starting" }));
    }

    /// Rewrite part of how the agent was started: whatever `patch` names, over
    /// what the record holds.
    pub fn set_meta(&self, id: &str, patch: Value) {
        let path = self.agent_dir(id).join("meta.json");
        let mut meta = read(&path).unwrap_or_else(|| json!({}));
        if let (Some(into), Some(from)) = (meta.as_object_mut(), patch.as_object()) {
            for (key, value) in from {
                into.insert(key.clone(), value.clone());
            }
        }
        write(&path, &meta);
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
        a_repo_at(&repo);
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

    /// Wait until the pane itself shows `needle`.
    ///
    /// A scenario delivers its hooks before it draws the screen under them, so
    /// a record that has reached a phase is not yet a pane that looks like it.
    /// A test that reads the screen rather than the record waits here first.
    pub fn until_shown(&self, id: &str, needle: &str) {
        let pane = self.pane_of(id);
        self.until(&format!("{id} to show {needle:?}"), || {
            self.capture(&pane).contains(needle).then_some(())
        });
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

/// Take out what the suite's own environment carries when it is run from
/// inside an agent's pane, or beside somebody's own codex.
///
/// `AMX_NESTED` drops every hook a stand-in delivers as nested, and the rest
/// name that outer agent's record, its scratch directory and the amx it runs
/// (#KR7ZYJ5Z, #C2G0FZ5E). `CODEX_HOME` names the person's own codex, and the
/// three `OPENCODE_CONFIG` variables their own opencode's config, which no
/// test may read or write: a test that wants one sets it itself.
fn leaks_nothing(command: &mut Command) {
    for name in [
        "AMX_NESTED",
        "AMX_DIR",
        "AMX_AGENT_DIR",
        "AMX_WORKTREE",
        "AMX_BIN",
        "CODEX_HOME",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_CONTENT",
    ] {
        command.env_remove(name);
    }
}

/// Whether tmux is saying nothing was listening, in the three shapes amx's
/// own `is_no_server` (src/tmux.rs) reads: a socket that was never there, one
/// with no server behind it any more, and a server that went while it was
/// being asked.
fn the_server_went(stderr: &[u8]) -> bool {
    let said = String::from_utf8_lossy(stderr);
    said.contains("error connecting to")
        || said.contains("no server running")
        || said.contains("server exited")
}

/// Where `tmux -L <name>` keeps its sockets: `tmux-<uid>` under
/// `$TMUX_TMPDIR`, else under `/tmp`, the same rule tmux applies and the same
/// one `tmux::socket_dir` reads. The suites set `TMUX_TMPDIR` themselves
/// (tests/e2e_wall.rs), so reading it as the socket directory itself, rather
/// than as what that directory sits under, is a cleanup that misses.
pub fn socket_dir() -> PathBuf {
    sockets_under(std::env::var_os("TMUX_TMPDIR"))
}

/// The same rule, given the variable, so it can be checked without setting one
/// on a process that has tests running beside it.
fn sockets_under(tmpdir: Option<std::ffi::OsString>) -> PathBuf {
    tmpdir
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
        .join(format!("tmux-{}", uid()))
}

/// Whose sockets these are, as tmux names the directory.
fn uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(0)
}

impl Default for Harness {
    fn default() -> Self {
        Harness::new()
    }
}

/// The card, opened on the agent the view is holding the cursor over.
///
/// Waited for by its rule, which is the one row the card draws whatever it is
/// a look at: the agent's own name on it, past the mark its row wears, and the
/// card's own dashes, which no row of the list carries. What a test that opens
/// a card is about is what is on the card, and waiting on anything inside it
/// would pin every one of them to a drawing that is not theirs.
pub fn card_on(amx: &Harness, view: &str, id: &str) -> String {
    amx.until("the row", || amx.capture(view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", view, "Space"]);
    amx.until("the card", || {
        let drawn = amx.capture(view);
        drawn
            .lines()
            .any(|line| line.contains(id) && line.contains('┈'))
            .then_some(drawn)
    })
}

/// The exit code of a finished process.
pub fn code(out: &Output) -> i32 {
    out.status.code().expect("amx exited with a code")
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `amx status <id> --json`, parsed.
pub fn status(amx: &Harness, id: &str) -> Value {
    let out = amx.amx(&["status", id, "--json"]);
    assert!(
        out.status.success(),
        "amx status: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the status is json")
}

/// `amx ls --json`, parsed.
pub fn ls(amx: &Harness) -> Vec<Value> {
    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the listing is json")
}

/// Wait until `amx status` reads the agent as `want`.
pub fn until_read(amx: &Harness, id: &str, want: &str) -> Value {
    amx.until(&format!("{id} to read {want}"), || {
        let agent = status(amx, id);
        (agent["state"] == want).then_some(agent)
    })
}

/// A pane's whole text, scrollback included.
pub fn said_in(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-J", "-S", "-", "-t", pane])
}

/// The `argv:` line the stand-in prints on its screen, once it has.
pub fn argv_of(amx: &Harness, id: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until("the vendor to say how it was called", || {
        amx.capture(&pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    })
}

/// The vendor argv amx wrote into the handoff.
pub fn command_of(amx: &Harness, id: &str) -> Vec<String> {
    amx.handoff(id)["command"]
        .as_array()
        .expect("the handoff names a command")
        .iter()
        .map(|arg| arg.as_str().expect("an argument").to_string())
        .collect()
}

/// A directory under home with its own allowed `.amx/config.toml`.
pub fn a_project(amx: &Harness, name: &str, config: &str) -> PathBuf {
    let dir = amx.home().join(name);
    std::fs::create_dir_all(dir.join(".amx")).expect("the project's own directory");
    std::fs::write(dir.join(".amx/config.toml"), config).expect("the project's config");
    let allowed = amx.amx(&["allow", "--dir", &dir.to_string_lossy()]);
    assert!(allowed.status.success(), "amx allow: {:?}", allowed);
    dir
}

/// `amx doctor`'s line for the check called `name`: whether it passed, and the
/// line itself.
pub fn check_line(printed: &str, name: &str) -> (bool, String) {
    printed
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let verdict = fields.next()?;
            (fields.next()? == name).then(|| (verdict == "ok", line.to_string()))
        })
        .unwrap_or_else(|| panic!("doctor said nothing about the {name}:\n{printed}"))
}

/// Every path under `dir` with each file's bytes, sorted.
pub fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let (mut found, mut left) = (Vec::new(), vec![dir.to_path_buf()]);
    while let Some(here) = left.pop() {
        for entry in std::fs::read_dir(&here).into_iter().flatten().flatten() {
            let path = entry.path();
            let bytes = match path.is_dir() && !path.is_symlink() {
                true => {
                    left.push(path.clone());
                    None
                }
                false => std::fs::read(&path).ok(),
            };
            found.push((path, bytes));
        }
    }
    found.sort();
    found
}

/// Epoch seconds, for records a test writes as though they had just happened.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

/// An agent whose command ended `ago` seconds ago: no pane, only the record.
pub fn finished(amx: &Harness, id: &str, state: &str, ago: u64) {
    let at = now() - ago;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": state,
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "did what it was asked",
        }),
    );
}

/// Every agent amx holds a record for, sorted.
pub fn agents(amx: &Harness) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(amx.state_root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

/// Wait for the empty view's one line.
pub fn until_empty(amx: &Harness, view: &str) {
    amx.until("the empty view", || {
        amx.capture(view).contains("nobody asking").then_some(())
    });
}

/// Send one key, by tmux's name for it.
pub fn press(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key]);
}

/// Type text literally, as a person types it.
pub fn types(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["send-keys", "-t", view, "-l", text]);
}

/// A mouse event as the raw SGR bytes a terminal sends: button 0 is the left
/// button, 64 and 65 the wheel, 35 motion. Column and row count from one.
pub fn mouse(amx: &Harness, view: &str, code: u16, column: u16, row: u16, press: bool) {
    let end = if press { 'M' } else { 'm' };
    types(amx, view, &format!("\u{1b}[<{code};{column};{row}{end}"));
}

/// A left click: press and release on one cell.
pub fn click(amx: &Harness, view: &str, column: u16, row: u16) {
    mouse(amx, view, 0, column, row, true);
    mouse(amx, view, 0, column, row, false);
}

/// The screen with the escapes tmux wrote for its colours.
pub fn coloured(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-e", "-J", "-t", pane])
}

/// The last line holding `text`, escapes and all.
///
/// The last, because the header repeats the group names and the list is below
/// it.
pub fn coloured_line(amx: &Harness, view: &str, text: &str) -> String {
    let drawn = coloured(amx, view);
    drawn
        .lines()
        .rfind(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
        .to_string()
}

/// The SGR attributes in force where `word` starts in a coloured capture.
///
/// Pass the whole screen when an attribute may have been set on a line above:
/// tmux writes an attribute where it changes and leaves it in force.
pub fn sgr_at(drawn: &str, word: &str) -> Vec<u16> {
    in_force(&drawn[..starts_at(drawn, word)])
}

/// The byte offset of `word` in a coloured capture.
pub fn starts_at(line: &str, word: &str) -> usize {
    line.find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"))
}

/// The SGR attributes in force at the end of `walked`.
///
/// Resets are honoured and the arguments of 38 and 48 are consumed, so the 2
/// of `38;2;r;g;b` is never read as dim.
pub fn in_force(walked: &str) -> Vec<u16> {
    let mut on: Vec<u16> = Vec::new();
    let mut rest = walked;
    while let Some(start) = rest.find("\u{1b}[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('m') else { break };
        let params: Vec<u16> = after[..end]
            .split(';')
            .map(|param| param.parse().unwrap_or(0))
            .collect();
        let mut n = 0;
        while n < params.len() {
            match params[n] {
                0 => on.clear(),
                22 => on.retain(|param| *param != 1 && *param != 2),
                38 | 48 => {
                    n += match params.get(n + 1) {
                        Some(2) => 4,
                        Some(5) => 2,
                        _ => 0,
                    };
                }
                param => on.push(param),
            }
            n += 1;
        }
        rest = &after[end + 1..];
    }
    on
}

/// A role's colour as `assets/themes/default.toml` spells it: a hex, or the
/// name of a terminal colour.
///
/// Read from the file so the tests follow the palette when it changes.
fn default_theme(role: &str) -> &'static str {
    include_str!("../../assets/themes/default.toml")
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role} = ")))
        .unwrap_or_else(|| panic!("the default theme names {role}"))
        .trim()
        .trim_matches('"')
}

/// A `#rrggbb` colour as three bytes.
pub fn rgb(said: &str) -> (u8, u8, u8) {
    let hex = said
        .strip_prefix('#')
        .unwrap_or_else(|| panic!("a hex colour: {said}"));
    let byte = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or_else(|_| panic!("a hex colour: {said}"))
    };
    (byte(0), byte(2), byte(4))
}

/// The SGR parameters tmux writes for truecolour text.
pub fn text_in((r, g, b): (u8, u8, u8)) -> String {
    format!("38;2;{r};{g};{b}")
}

/// The SGR parameters tmux writes for text in one of the eight named colours.
fn text_named(said: &str) -> String {
    let at = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ]
    .iter()
    .position(|name| *name == said)
    .unwrap_or_else(|| panic!("a colour of the terminal's own: {said}"));
    format!("38;5;{at}")
}

/// The SGR parameters for text in a default-theme role.
pub fn foreground(role: &str) -> String {
    let said = default_theme(role);
    match said.starts_with('#') {
        true => text_in(rgb(said)),
        false => text_named(said),
    }
}

/// The SGR parameters for a background in a default-theme role.
fn background(role: &str) -> String {
    let (r, g, b) = rgb(default_theme(role));
    format!("48;2;{r};{g};{b}")
}

/// The cursor bar's background.
pub fn bar() -> String {
    background("cursor")
}

/// One tmux format variable of a pane, window or session.
pub fn pane_field(amx: &Harness, pane: &str, format: &str) -> String {
    amx.tmux(&["display-message", "-p", "-t", pane, format])
}

/// Size the pane's window. A detached window defaults to 80x24.
pub fn resize(amx: &Harness, view: &str, width: u16, height: u16) {
    amx.tmux(&["set-option", "-w", "-t", view, "window-size", "manual"]);
    amx.tmux(&[
        "resize-window",
        "-t",
        view,
        "-x",
        &width.to_string(),
        "-y",
        &height.to_string(),
    ]);
}

/// A 60x24 pane showing exactly `rows`, standing in for an agent's pane.
pub fn a_pane_showing(amx: &Harness, rows: &[&str]) -> String {
    let drawn: String = rows.iter().map(|row| format!("{row}\\n")).collect();
    amx.tmux(&[
        "new-session",
        "-d",
        "-x",
        "60",
        "-y",
        "24",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        &format!("printf '{drawn}'; while :; do sleep 0.05; done"),
    ])
}

/// A tmux client attached to `session` from a pane of its own, as a person
/// running `tmux attach` has. Answers with that pane.
///
/// `TMUX` and `TMUX_PANE` are cleared, since a client inside tmux refuses to
/// nest.
pub fn watching(amx: &Harness, session: &str) -> String {
    amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "env",
        "-u",
        "TMUX",
        "-u",
        "TMUX_PANE",
        "tmux",
        "-L",
        amx.socket(),
        "-f",
        "/dev/null",
        "attach-session",
        "-t",
        session,
    ])
}

/// The ttys of the clients attached to `session`.
pub fn clients_on(amx: &Harness, session: &str) -> String {
    amx.tmux(&["list-clients", "-t", session, "-F", "#{client_tty}"])
}

/// A session that is not the agent under test.
///
/// Losing the last pane takes the server down, and a restarted server reuses
/// the dead pane's id.
pub fn something_else_on_the_server(amx: &Harness) {
    amx.tmux(&[
        "new-session",
        "-d",
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
}

/// Run git in `dir` with no global or system config and a fixed identity, and
/// answer with its stdout, trailing whitespace trimmed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "amx tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "amx tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Make `dir` a git repository on `main` with one commit.
pub fn a_repo_at(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.name", "amx tests"]);
    git(dir, &["config", "user.email", "tests@example.invalid"]);
    std::fs::write(dir.join("README.md"), "before\n").expect("a file to commit");
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-m", "first"]);
}

/// `git branch --list` in `repo`.
pub fn branches(repo: &Path) -> String {
    git(repo, &["branch", "--list"])
}

/// Spawn an agent playing `scenario` with a worktree of its own in `repo`, and
/// answer with the worktree's path.
pub fn with_a_worktree(amx: &Harness, id: &str, repo: &Path, scenario: &str) -> String {
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.meta(id)["worktree"]
        .as_str()
        .expect("a worktree")
        .to_string()
}

/// [`with_a_worktree`] played to the end of a turn that finishes.
pub fn an_ended_agent(amx: &Harness, id: &str, repo: &Path) -> String {
    let tree = with_a_worktree(amx, id, repo, "finishes");
    amx.until_state(id, "done");
    tree
}

/// Commit a file in `tree`, so the agent's branch has work main does not.
pub fn work_on_the_branch(tree: &str, name: &str) {
    let tree = Path::new(tree);
    std::fs::write(tree.join(name), "fn login() {}\n").expect("a file to commit");
    git(tree, &["add", name]);
    git(tree, &["commit", "-m", "fix the login bug"]);
}

/// The `pr.json` a forge lookup writes: request `number` on the agent's branch,
/// merged at the head `tree` stands on now.
pub fn a_merged_request(amx: &Harness, id: &str, number: u64, tree: &str) {
    let head = git(Path::new(tree), &["rev-parse", "HEAD"]);
    std::fs::write(
        amx.agent_dir(id).join("pr.json"),
        json!({
            "asked": now(),
            "branch": format!("amx/{id}"),
            "prs": [{ "number": number, "standing": "merged" }],
            "merged_heads": [head],
        })
        .to_string(),
    )
    .expect("writing pr.json");
}

/// Merge the agent's branch into the checked-out branch of `repo`.
pub fn merged_by_hand(repo: &Path, id: &str) {
    git(
        repo,
        &["merge", "--no-ff", "-m", "merge", &format!("amx/{id}")],
    );
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

/// These run in every binary that takes the harness in. They are cheap, and
/// what they hold is the rule the harness cleans up by: get the directory
/// wrong and the sockets pile up in silence, one per server, until new servers
/// time out.
#[test]
fn common_a_named_tmpdir_holds_the_socket_directory_under_it() {
    assert_eq!(
        sockets_under(Some("/run/user/1000".into())),
        PathBuf::from(format!("/run/user/1000/tmux-{}", uid())),
        "tmux puts tmux-<uid> under $TMUX_TMPDIR, not the sockets themselves"
    );
}

#[test]
fn common_no_tmpdir_named_is_the_socket_directory_under_tmp() {
    let under_tmp = PathBuf::from(format!("/tmp/tmux-{}", uid()));
    assert_eq!(sockets_under(None), under_tmp);
    assert_eq!(
        sockets_under(Some("".into())),
        under_tmp,
        "an empty $TMUX_TMPDIR names no directory, which is how tmux reads it"
    );
}
