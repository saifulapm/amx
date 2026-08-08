//! An isolated machine per test: temp roots, the binary under test, processes.
//!
//! Every root travels on the spawned [`Command`], never through this process's
//! own environment, so two environments in one test binary stay isolated even
//! though `cargo test` runs their tests on threads of one process.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::tty::{Terminal, open_pty, termios_of};

/// How long a test waits for something to happen.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// How much of a `sun_path` a harness may spend below `$TMPDIR`.
///
/// `sun_path` is 104 bytes on darwin — the tighter of the two tier-1 platforms
/// (Linux gives 108) — and darwin's per-user `$TMPDIR`,
/// `/var/folders/<2>/<28>/T`, has already spent 48 of them before a test names
/// anything. 52 is that rent rounded up; what is left is everything a harness
/// adds: the temp root, `run/amx/`, the session name and `sock`.
///
/// Charged on every platform, deliberately. A check that only a macOS runner
/// can fail is a check that only CI runs, and this suite has already shipped
/// one socket path that fit in a developer's `/tmp` and not in a runner's.
pub const SUN_BUDGET: usize = 104 - 52;

/// How long a test sleeps between polls.
pub const TICK: Duration = Duration::from_millis(5);

/// A directory under `$TMPDIR`, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Kept short deliberately: this directory prefixes a unix socket path,
        // and darwin's $TMPDIR alone eats half the sun_path budget (see
        // [`SUN_BUDGET`]). A four-char tag survives for debuggability.
        let brief: String = tag.chars().take(4).collect();
        let path = std::env::temp_dir().join(format!("a{}-{n}-{brief}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the temp dir");
        Self(path)
    }

    /// Where it is.
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
pub struct Env {
    dir: TempDir,
    /// The session name every command in this harness uses.
    pub session: String,
}

impl Env {
    /// A fresh environment, with a session name unique to it.
    ///
    /// The socket this environment will bind is measured here, against
    /// [`SUN_BUDGET`], so a tag too long to bind on darwin fails at the line
    /// that chose it rather than at the first probe on a macOS runner.
    pub fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        assert_sun_path_fits(dir.path(), tag);
        std::fs::create_dir_all(dir.path().join("run")).expect("create the runtime root");
        std::fs::create_dir_all(dir.path().join("state")).expect("create the state root");
        Self {
            dir,
            session: tag.to_owned(),
        }
    }

    /// The `amx` binary under test.
    pub fn exe(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_amx"))
    }

    /// This environment's temp root.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// This environment's session socket.
    pub fn socket(&self) -> PathBuf {
        self.runtime_dir().join("sock")
    }

    /// This environment's session runtime directory.
    pub fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run").join("amx").join(&self.session)
    }

    /// An `amx` command with this environment's roots and session.
    ///
    /// `$AMX_SESSION` carries the name rather than a `--session` flag on every
    /// call, so the tests also exercise the environment path of 04 §1's
    /// "`--session work` / `AMX_SESSION` select a named server instance".
    pub fn command(&self) -> Command {
        let mut command = Command::new(self.exe());
        roots_on(&mut command, self.dir.path(), &self.session);
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
        Output::of(&out)
    }

    /// Start `args` in the background, with no terminal.
    pub fn spawn(&self, args: &[&str]) -> Child {
        self.command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn amx")
    }

    /// Start `args` attached to a fresh pseudoterminal.
    pub fn spawn_on_tty(&self, args: &[&str], rows: u16, cols: u16) -> Terminal {
        spawn_on_tty(&mut self.command(), args, rows, cols)
    }

    /// Stop this environment's server, if one is running.
    pub fn stop(&self) {
        let _ = self.run(&["session", "stop"]);
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // A daemon outlives the test that started it by design, so a test that
        // fails half way must not leave one behind holding a deleted socket.
        self.stop();
    }
}

/// Point `command` at the roots below `root`, for session `session`.
///
/// The one place the environment of a command under test is assembled, so the
/// [`Env`] harness and the [`super::rig::Rig`] one cannot drift apart about
/// what an isolated machine is.
pub fn roots_on(command: &mut Command, root: &Path, session: &str) {
    command
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("XDG_STATE_HOME", root.join("state"))
        // Pinned rather than left to the `HOME` fallback: an exported
        // `XDG_CONFIG_HOME` is inherited, and would aim the command under
        // test at the developer's own `config.toml`.
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("HOME", root)
        .env("AMX_SESSION", session);
}

/// Run `command` with `args` on a fresh pseudoterminal of this size.
///
/// By `&mut` rather than by value so a caller can finish configuring the
/// command — extra variables, a doctored `PATH` — in the builder style
/// [`Command`] itself uses.
pub fn spawn_on_tty(command: &mut Command, args: &[&str], rows: u16, cols: u16) -> Terminal {
    let pty = open_pty(rows, cols);
    // Captured before the child exists, so "restored" can be compared
    // against this terminal's own attributes rather than a fresh pty's.
    let initial = termios_of(&pty.slave);
    let child = command
        .args(args)
        .stdin(Stdio::from(pty.slave.try_clone().expect("dup")))
        .stdout(Stdio::from(pty.slave.try_clone().expect("dup")))
        .stderr(Stdio::from(pty.slave.try_clone().expect("dup")))
        .spawn()
        .expect("spawn amx on a tty");
    Terminal::new(pty, child, initial)
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
    /// Read one out of a finished process.
    pub fn of(out: &std::process::Output) -> Self {
        Self {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Assert it succeeded, and return its stdout.
    pub fn ok(&self) -> &str {
        assert_eq!(self.code, Some(0), "amx failed: {self:?}");
        &self.stdout
    }

    /// Assert it failed, and return its stderr.
    pub fn failed(&self) -> &str {
        assert_ne!(self.code, Some(0), "amx succeeded: {self:?}");
        &self.stderr
    }
}

/// Assert the socket an environment rooted at `dir` and named `tag` will bind
/// spends no more than [`SUN_BUDGET`] below `$TMPDIR`.
///
/// Measured below `$TMPDIR` rather than absolutely, because the absolute
/// length is the runner's business: `/tmp` on Linux and a 48-byte private
/// directory on darwin, for the same harness and the same tag.
pub fn assert_sun_path_fits(dir: &Path, tag: &str) {
    let socket = dir.join("run").join("amx").join(tag).join("sock");
    let tmp = std::env::temp_dir();
    let root = tmp.to_string_lossy().trim_end_matches('/').len();
    let spent = socket.as_os_str().len().saturating_sub(root);
    assert!(
        spent <= SUN_BUDGET,
        "the socket for {tag:?} spends {spent} bytes below $TMPDIR and the \
         budget is {SUN_BUDGET}; darwin would refuse to bind it. Shorten the \
         tag — it names both the temp root and the session.\n{}",
        socket.display()
    );
}

/// Whether `haystack` contains `needle`.
pub fn window(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Poll `cond` until it holds, failing the test if it never does.
pub fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(TICK);
    }
}
