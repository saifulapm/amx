//! An isolated machine per test: temp roots, the binary under test, processes.
//!
//! Modeled on the T17 CLI harness (`crates/amx/tests/support`), with one
//! difference forced by living outside the `amx` package: `CARGO_BIN_EXE_amx`
//! is only set for that package's own tests, so the binary is found next to
//! this test executable instead — `cargo test --workspace` (what CI runs)
//! builds it there before any test runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::term::{Terminal, open_pty, termios_of};

/// How long a test waits for something to happen before failing.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// How long a poll loop waits between looks at its condition.
pub const TICK: Duration = Duration::from_millis(5);

/// A directory under `$TMPDIR`, removed when the test ends.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("amx-t18-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    /// Where it is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An isolated machine to run `amx` on: its own runtime, state and home roots.
#[derive(Debug)]
pub struct Env {
    dir: TempDir,
    /// The session name every command in this environment uses.
    pub session: String,
    /// Extra variables set on every spawned command, e.g. `SHELL`.
    vars: Vec<(String, String)>,
}

impl Env {
    /// A fresh environment whose session name is unique to it.
    ///
    /// `SHELL` defaults to `/bin/sh` so seeded panes behave the same on every
    /// machine; a test that wants its own shell overrides it with
    /// [`Env::set_var`], which wins by coming later on the command.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        std::fs::create_dir_all(dir.path().join("run")).expect("create the runtime root");
        std::fs::create_dir_all(dir.path().join("state")).expect("create the state root");
        Self {
            dir,
            session: tag.to_owned(),
            vars: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        }
    }

    /// A scratch directory panes and tests can exchange files through.
    #[must_use]
    pub fn scratch(&self) -> PathBuf {
        let dir = self.dir.path().join("scratch");
        std::fs::create_dir_all(&dir).expect("create the scratch dir");
        dir
    }

    /// Set a variable on every command this environment spawns.
    ///
    /// The variable travels on the [`Command`], never through this process's
    /// own environment, so two environments in one test process stay isolated.
    pub fn set_var(&mut self, key: &str, value: &str) {
        self.vars.push((key.to_owned(), value.to_owned()));
    }

    /// The `amx` binary under test, found next to this test executable.
    #[must_use]
    pub fn exe(&self) -> PathBuf {
        amx_bin()
    }

    /// This environment's session socket.
    #[must_use]
    pub fn socket(&self) -> PathBuf {
        self.runtime_dir().join("sock")
    }

    /// This environment's session runtime directory.
    #[must_use]
    pub fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run").join("amx").join(&self.session)
    }

    /// An `amx` command carrying this environment's roots and session.
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new(self.exe());
        command
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("HOME", self.dir.path())
            .env("AMX_SESSION", &self.session);
        for (key, value) in &self.vars {
            command.env(key, value);
        }
        command
    }

    /// Run `args` to completion and return what it did.
    pub fn run(&self, args: &[&str]) -> Output {
        let out = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run amx");
        Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Start `amx server` in the foreground as a child of this test.
    ///
    /// A child rather than a daemon so the test owns the pid: memory can be
    /// read from `/proc` and shutdown can be asserted, not hoped for. Returns
    /// once the socket answers a connect probe.
    pub fn server(&self) -> ServerChild {
        let child = self
            .command()
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn amx server");
        let socket = self.socket();
        wait_until("the server answers its socket", || {
            amx_server::session::probe::probe(&socket).is_ok_and(|p| p.is_running())
        });
        ServerChild { child }
    }

    /// Start the real client on a fresh pseudoterminal of this size.
    pub fn attach_on_tty(&self, args: &[&str], rows: u16, cols: u16) -> Terminal {
        let pty = open_pty(rows, cols);
        let initial = termios_of(&pty.slave);
        let child = self
            .command()
            .args(args)
            .stdin(Stdio::from(pty.slave.try_clone().expect("dup the slave")))
            .stdout(Stdio::from(pty.slave.try_clone().expect("dup the slave")))
            .stderr(Stdio::from(pty.slave.try_clone().expect("dup the slave")))
            .spawn()
            .expect("spawn amx on a tty");
        Terminal::new(pty, child, initial)
    }

    /// Stop this environment's server, if one is running.
    pub fn stop(&self) {
        let _ = self.run(&["session", "stop"]);
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // A daemonized server outlives the test that started it by design, so
        // a test that failed half way must not leave one behind.
        self.stop();
    }
}

/// A foreground `amx server` child owned by the test.
#[derive(Debug)]
pub struct ServerChild {
    child: Child,
}

impl ServerChild {
    /// The server's pid.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether the process is still running.
    pub fn alive(&mut self) -> bool {
        self.child.try_wait().expect("try_wait").is_none()
    }

    /// The server's resident set size in bytes, read from `/proc`.
    #[must_use]
    pub fn rss_bytes(&self) -> u64 {
        let statm = std::fs::read_to_string(format!("/proc/{}/statm", self.pid()))
            .expect("the server has a /proc entry");
        let resident: u64 = statm
            .split_whitespace()
            .nth(1)
            .expect("statm has a resident field")
            .parse()
            .expect("resident is a number");
        resident * 4096
    }

    /// Bytes the server has read from anywhere, per `/proc/<pid>/io`.
    ///
    /// Under a flooding pane this is dominated by the pty; a flood test uses
    /// the delta to prove the server really ingested the flood it survived.
    #[must_use]
    pub fn read_bytes(&self) -> u64 {
        let io = std::fs::read_to_string(format!("/proc/{}/io", self.pid()))
            .expect("the server has a /proc io entry");
        io.lines()
            .find_map(|line| line.strip_prefix("rchar: "))
            .expect("io reports rchar")
            .trim()
            .parse()
            .expect("rchar is a number")
    }

    /// Ask the server to exit and wait for it, asserting a clean code.
    pub fn shutdown(mut self) {
        signal_term(self.child.id());
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for the server") {
                assert!(status.success(), "the server exited uncleanly: {status:?}");
                return;
            }
            assert!(Instant::now() < deadline, "the server ignored SIGTERM");
            std::thread::sleep(TICK);
        }
    }
}

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Send `SIGTERM` to `pid`.
fn signal_term(pid: u32) {
    let pid = rustix::process::Pid::from_raw(pid.cast_signed()).expect("a live child pid");
    let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
}

/// What a finished `amx` invocation did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Output {
    /// Exit code, or `None` if it was signalled.
    pub code: Option<i32>,
    /// Everything it wrote to stdout.
    pub stdout: String,
    /// Everything it wrote to stderr.
    pub stderr: String,
}

impl Output {
    /// Assert the command succeeded, and return its stdout.
    pub fn ok(&self) -> &str {
        assert_eq!(self.code, Some(0), "amx failed: {self:?}");
        &self.stdout
    }
}

/// Poll `cond` until it holds, failing the test if it never does.
///
/// This is the one place the harness waits: a condition and a deadline, so a
/// test can only ever be slow when the thing it waits for is slow — expiry is
/// a failure, never a green path.
pub fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(TICK);
    }
}

/// How many live processes have `marker` in their argv.
///
/// Read straight out of `/proc`: a claim like "the pane's process survived the
/// client dying" is about processes, and only the process table can attest to
/// it. Markers are per-test-unique strings planted in the spawned command.
#[must_use]
pub fn processes_with_arg(marker: &str) -> usize {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            nul_separated(&entry.path().join("cmdline"))
                .iter()
                .any(|arg| arg.to_string_lossy().contains(marker))
        })
        .count()
}

/// A `/proc` file of NUL-separated strings.
fn nul_separated(path: &Path) -> Vec<OsString> {
    use std::os::unix::ffi::OsStringExt as _;

    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| OsString::from_vec(part.to_vec()))
        .collect()
}

/// The `amx` binary this test run built.
fn amx_bin() -> PathBuf {
    // target/debug/deps/<test>-<hash> -> target/debug/amx
    let exe = std::env::current_exe().expect("current_exe");
    let deps = exe.parent().expect("test binaries live in deps/");
    let target = deps.parent().expect("deps/ lives in the profile dir");
    let bin = target.join("amx");
    assert!(
        bin.is_file(),
        "no amx binary at {}; build it first (`cargo test --workspace` does)",
        bin.display()
    );
    bin
}
