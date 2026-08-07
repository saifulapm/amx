//! Scaffolding for the T17 CLI acceptance suite: the real binary, a real
//! terminal, a real socket.
//!
//! These tests are process-level on purpose. Daemonization, the bind race and
//! terminal restoration are all properties of *processes* — a `setsid` that is
//! never execed, a socket two processes contend for, a `termios` a third
//! process has to put back — and none of them can be observed from inside one
//! test process pretending to be several.

#![allow(dead_code, reason = "each test binary uses a subset of the harness")]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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

/// The alt-screen sequences the client writes on the way in and out.
pub const ALT_ENTER: &[u8] = b"\x1b[?1049h";
/// See [`ALT_ENTER`].
pub const ALT_LEAVE: &[u8] = b"\x1b[?1049l";

/// `ctrl+a`, the prefix key (04 §7).
pub const PREFIX: u8 = 0x01;

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
        command
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            // Pinned rather than left to the `HOME` fallback: an exported
            // `XDG_CONFIG_HOME` is inherited, and would aim the command under
            // test at the developer's own `config.toml`.
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("HOME", self.dir.path())
            .env("AMX_SESSION", &self.session);
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
        let pty = open_pty(rows, cols);
        // Captured before the child exists, so "restored" can be compared
        // against this terminal's own attributes rather than a fresh pty's.
        let initial = termios_of(&pty.slave);
        let child = self
            .command()
            .args(args)
            .stdin(Stdio::from(pty.slave.try_clone().expect("dup")))
            .stdout(Stdio::from(pty.slave.try_clone().expect("dup")))
            .stderr(Stdio::from(pty.slave.try_clone().expect("dup")))
            .spawn()
            .expect("spawn amx on a tty");
        Terminal {
            pty,
            child,
            initial,
            seen: Vec::new(),
        }
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
    /// Assert it succeeded, and return its stdout.
    pub fn ok(&self) -> &str {
        assert_eq!(self.code, Some(0), "amx failed: {self:?}");
        &self.stdout
    }
}

/// A pseudoterminal pair.
pub struct Pty {
    /// The side a test reads from and writes to.
    pub master: File,
    /// The side the child process uses as its terminal.
    pub slave: File,
}

/// Open a pty pair sized `rows` by `cols`.
///
/// A freshly opened pty reports a 0x0 window, which starves every layout
/// computation of any area to tile, so the size is set before anything is
/// spawned onto it.
pub fn open_pty(rows: u16, cols: u16) -> Pty {
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .expect("openpt");
    rustix::pty::grantpt(&master).expect("grantpt");
    rustix::pty::unlockpt(&master).expect("unlockpt");
    let name = rustix::pty::ptsname(&master, Vec::new()).expect("ptsname");
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PathBuf::from(name.to_string_lossy().into_owned()))
        .expect("open the pty slave");
    // Sized through the slave, the way openpty(3) does it: the window lives on
    // the terminal, not on a descriptor, and the slave takes TIOCSWINSZ on
    // every platform where darwin's master refuses it until the slave is open.
    set_size(&slave, rows, cols);
    // Non-blocking on the master: a test reads what is there and moves on
    // rather than blocking on a child that has nothing more to say.
    let flags = rustix::fs::fcntl_getfl(&master).expect("getfl");
    rustix::fs::fcntl_setfl(&master, flags | rustix::fs::OFlags::NONBLOCK).expect("setfl");
    Pty {
        master: File::from(master),
        slave,
    }
}

fn set_size<Fd: std::os::fd::AsFd>(fd: Fd, rows: u16, cols: u16) {
    rustix::termios::tcsetwinsize(
        fd,
        rustix::termios::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .expect("set the pty window size");
}

/// The `Debug` form of `fd`'s terminal attributes.
///
/// `Termios` has no `PartialEq`, but its `Debug` form is complete, so two of
/// these compare exactly what "the terminal is how you found it" means.
pub fn termios_of<Fd: std::os::fd::AsFd>(fd: Fd) -> String {
    format!("{:?}", rustix::termios::tcgetattr(fd).expect("tcgetattr"))
}

/// A child process running on a pseudoterminal.
pub struct Terminal {
    pty: Pty,
    child: Child,
    initial: String,
    seen: Vec<u8>,
}

impl Terminal {
    /// The child's pid.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything the child has written so far.
    pub fn output(&self) -> &[u8] {
        &self.seen
    }

    /// The terminal's current attributes.
    ///
    /// Read from the test's own slave descriptor, which is the same terminal
    /// the child put into raw mode.
    pub fn termios(&self) -> String {
        termios_of(&self.pty.slave)
    }

    /// The terminal's attributes before the child was started.
    pub fn initial_termios(&self) -> &str {
        &self.initial
    }

    /// Resize the terminal, which sends the child a `SIGWINCH`.
    pub fn resize(&self, rows: u16, cols: u16) {
        set_size(&self.pty.slave, rows, cols);
    }

    /// Send bytes to the child's stdin.
    pub fn send(&mut self, bytes: &[u8]) {
        self.pty.master.write_all(bytes).expect("write to the pty");
        self.pty.master.flush().expect("flush the pty");
    }

    /// Send the prefix key and then `key`.
    pub fn chord(&mut self, key: u8) {
        self.send(&[PREFIX, key]);
    }

    /// Read whatever is waiting, appending it to [`Self::output`].
    pub fn drain(&mut self) {
        let mut buf = [0_u8; 4096];
        loop {
            match self.pty.master.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => self.seen.extend_from_slice(&buf[..n]),
                // WouldBlock: nothing more right now. Other errors are the
                // child having closed the slave, which is equally "no more".
                Err(err) if err.kind() == ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    /// Read until the output contains `needle`, or fail.
    pub fn wait_for(&mut self, needle: &[u8]) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.drain();
            if window(&self.seen, needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {:?} in {} bytes of output",
                String::from_utf8_lossy(needle),
                self.seen.len()
            );
            std::thread::sleep(TICK);
        }
    }

    /// Wait for the child to exit and return its status code.
    pub fn wait(&mut self) -> Option<i32> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.drain();
            match self.child.try_wait().expect("wait") {
                Some(status) => {
                    self.drain();
                    return status.code();
                }
                None => assert!(Instant::now() < deadline, "the client did not exit"),
            }
            std::thread::sleep(TICK);
        }
    }

    /// Kill the child, for a test that has finished with it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

/// How many `amx server` processes for `session` are alive.
///
/// "Exactly one server survives the race" is a claim about processes, and only
/// the process table can attest to it: a socket has only one peer whether the
/// loser exited or is sitting there unreachable. Test binaries run their tests
/// in parallel threads of one process, so every test's servers are the same
/// executable and only the session separates them — which is why every server
/// this counts is started with an explicit `--session` in its argv.
///
/// Linux reads `/proc` and checks `AMX_SESSION` in the environment as well;
/// macOS has no `/proc` and another process's environment is not readable
/// there, so it matches the argv `ps` reports.
#[cfg(target_os = "linux")]
pub fn server_processes(exe: &Path, session: &str) -> usize {
    let marker = OsString::from(format!("AMX_SESSION={session}"));
    let entries = std::fs::read_dir("/proc").expect("/proc is readable");
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let argv = nul_separated(&entry.path().join("cmdline"));
            let is_server = argv.len() >= 2 && argv[0] == exe.as_os_str() && argv[1] == "server";
            is_server && nul_separated(&entry.path().join("environ")).contains(&marker)
        })
        .count()
}

/// See the Linux twin above; `ps -o command=` is the process table here.
#[cfg(not(target_os = "linux"))]
pub fn server_processes(exe: &Path, session: &str) -> usize {
    let out = Command::new("ps")
        .args(["-axww", "-o", "command="])
        .output()
        .expect("run ps");
    assert!(out.status.success(), "ps failed: {out:?}");
    let server = format!("{} server --session {session}", exe.display());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.trim_end() == server)
        .count()
}

/// A `/proc` file of NUL-separated strings.
#[cfg(target_os = "linux")]
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
