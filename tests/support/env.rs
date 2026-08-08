//! An isolated machine per test: temp roots, the binary under test, processes.
//!
//! Modeled on the T17 CLI harness (`crates/amx/tests/support`), with one
//! difference forced by living outside the `amx` package: `CARGO_BIN_EXE_amx`
//! is only set for that package's own tests, so the binary is found next to
//! this test executable instead — `cargo test --workspace` (what CI runs)
//! builds it there before any test runs.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use amx_server::runtime::CENSUS_FILE;

use crate::term::{Terminal, open_pty, termios_of};

/// How long a test waits for something to happen before failing.
///
/// Generous on purpose: every wait here is condition-based, so a green run
/// never pays this — only a genuine hang does. Shared CI runners (macOS
/// especially) stretch a debug-build attach + shell spawn + typed round trip
/// well past what a dev box suggests, and a deadline exists to catch hangs,
/// not to benchmark the runner.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// How long a poll loop waits between looks at its condition.
pub const TICK: Duration = Duration::from_millis(5);

/// How much of a `sun_path` a harness may spend below `$TMPDIR`.
///
/// `sun_path` is 104 bytes on darwin — the tighter of the two tier-1 platforms
/// (Linux gives 108) — and darwin's per-user `$TMPDIR`,
/// `/var/folders/<2>/<28>/T`, has already spent 48 of them before a test names
/// anything. 52 is that rent rounded up; what is left is everything a harness
/// adds: the temp root, `run/amx/`, the session name and `sock`.
///
/// Charged on every platform, deliberately. A check that only a macOS runner
/// can fail is a check that only CI runs. The T17 harness
/// (`crates/amx/tests/support`) states the same budget for the same reason.
pub const SUN_BUDGET: usize = 104 - 52;

/// A directory under `$TMPDIR`, removed when the test ends.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// A directory nobody else in this process will pick.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Kept short deliberately: this directory prefixes a unix socket path,
        // and darwin's $TMPDIR alone eats half the sun_path budget (~104
        // bytes). A four-char tag survives for debuggability.
        let brief: String = tag.chars().take(4).collect();
        let path = std::env::temp_dir().join(format!("a{}-{n}-{brief}", std::process::id()));
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
    ///
    /// The socket this environment will bind is measured here, against
    /// [`SUN_BUDGET`], so a tag too long to bind on darwin fails at the line
    /// that chose it rather than at the first probe on a macOS runner.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        assert_sun_path_fits(dir.path(), tag);
        std::fs::create_dir_all(dir.path().join("run")).expect("create the runtime root");
        std::fs::create_dir_all(dir.path().join("state")).expect("create the state root");
        Self {
            dir,
            session: tag.to_owned(),
            vars: vec![("SHELL".to_owned(), "/bin/sh".to_owned())],
        }
    }

    /// This environment's `$HOME`, and the root every other path hangs off.
    ///
    /// Named because a suite sometimes has to write into the home of the
    /// process under test — `~/.ssh/config` for the loopback-ssh suite, which
    /// is the only way to hand ssh a port, a key and a host alias without a
    /// flag amx would have to grow to pass through.
    #[must_use]
    pub fn home(&self) -> &Path {
        self.dir.path()
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

    /// This environment's session state directory — where the snapshot and the
    /// scrollback sidecars live (`$XDG_STATE_HOME/amx/<session>`, D-M1-4).
    ///
    /// The half of the "restart preserving state" helper that the crash suite
    /// reads: the environment outlives any number of servers started on it, so
    /// killing one and starting the next is the reboot M1 exits on, and this is
    /// the directory that has to survive it. Named here rather than assembled
    /// in a suite because the layout is the server's, not a test's.
    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        self.dir
            .path()
            .join("state")
            .join("amx")
            .join(&self.session)
    }

    /// This environment's config file, and the directory it needs to sit in.
    ///
    /// Writing it is how a suite turns a `[persist]` toggle on for the server
    /// it is about to start; a session with no file at all is the default
    /// configuration, which is why nothing creates this by itself.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        let dir = self.dir.path().join("config").join("amx");
        std::fs::create_dir_all(&dir).expect("create the config dir");
        dir.join("config.toml")
    }

    /// An `amx` command carrying this environment's roots and session.
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new(self.exe());
        command
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            // Pinned, not just left to the `HOME` fallback: this process
            // inherits the developer's environment, and an exported
            // `XDG_CONFIG_HOME` would point the server under test at the
            // developer's own `config.toml`.
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
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
        ServerChild {
            child,
            runtime_dir: self.runtime_dir(),
        }
    }

    /// Start `amx server` and return the instant its socket file appears.
    ///
    /// [`Env::server`] waits until the socket *answers*, which is the whole
    /// assembly — the restore included. This one returns as soon as the path
    /// exists, which is the moment `Gateway::bind` claims the session, and it
    /// exists so a test can signal a server that is still assembling itself.
    /// Nothing else should want it.
    pub fn server_while_it_assembles(&self) -> ServerChild {
        let child = self
            .command()
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn amx server");
        let socket = self.socket();
        // Spun rather than ticked, and this is the one place that is right:
        // what comes *after* the bind is the assembly this wants to interrupt,
        // and it is milliseconds long. A [`TICK`] between looks would usually
        // be spent watching the thing under test finish.
        let deadline = Instant::now() + PATIENCE;
        while !socket.exists() {
            assert!(
                Instant::now() < deadline,
                "the server never bound {}",
                socket.display(),
            );
            std::hint::spin_loop();
        }
        ServerChild {
            child,
            runtime_dir: self.runtime_dir(),
        }
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
    /// Where the server writes its drain census if a shutdown overruns
    /// (`amx_server::runtime::CENSUS_FILE`). Held so a wedge report can name
    /// the task the drain is stuck on, not only the threads it is stuck in.
    runtime_dir: PathBuf,
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

    /// The server's resident set size in bytes.
    #[must_use]
    pub fn rss_bytes(&self) -> u64 {
        crate::platform::rss_bytes(self.pid())
    }

    /// Bytes the server has read from anywhere, or `None` where the platform
    /// cannot account for io (see [`crate::platform::read_bytes`]).
    ///
    /// Under a flooding pane this is dominated by the pty; a flood test uses
    /// the delta to prove the server really ingested the flood it survived.
    #[must_use]
    pub fn read_bytes(&self) -> Option<u64> {
        crate::platform::read_bytes(self.pid())
    }

    /// `SIGKILL` the server and reap it, the way a power cut would end it.
    ///
    /// The other half of "restart preserving state": nothing is flushed, no
    /// drain runs, no final capture is pushed — whatever is on disk is
    /// whatever the debounced save last committed, which is exactly the claim
    /// the crash suite is about. Consuming, because a killed server is not a
    /// server any more, and reaping here rather than in [`Drop`] so the test
    /// that killed it cannot leave a zombie behind for the next one to trip
    /// over. `SIGKILL` and not `SIGTERM` on purpose: a term would run the very
    /// shutdown path this suite must not depend on (R-M1-2).
    pub fn kill_dash_9(mut self) {
        self.child.kill().expect("SIGKILL the server");
        self.child.wait().expect("reap the killed server");
    }

    /// Ask the server to exit and wait for it, asserting a clean code.
    pub fn shutdown(self) {
        if let Err(stuck) = self.shutdown_within(PATIENCE) {
            panic!("{stuck}");
        }
    }

    /// Ask the server to exit and answer, within `budget`, whether it did.
    ///
    /// The bounded form the shutdown tripwire needs (R-M1-2). A wedge in the
    /// `JoinSet` drain is a server that took the signal and never finished
    /// joining, and a test that waits out [`PATIENCE`] and then asserts turns
    /// one wedge into a minute of silence per repetition. So this returns
    /// instead of asserting, and — the part that matters — it *kills what it
    /// gave up on*: a wedged server left running would hold its socket, its
    /// state directory and its panes' children against the next repetition,
    /// and one hang would become a suite that hangs.
    ///
    /// The error carries what every thread of the wedged process was doing at
    /// the moment the budget expired — and, since W01, the server's own drain
    /// census: the names of the supervised tasks that had not returned
    /// (`amx_server::runtime`). Threads say where the process is stuck;
    /// the census says which actor it is stuck on, which is the half nobody
    /// had ever managed to observe about this flake.
    ///
    /// [`PRESERVE`] suspends the kill. Set only by `scripts/spike/wedge.py`,
    /// which needs the process alive to take backtraces and a descriptor
    /// inventory off it, and which owns the cleanup afterwards. A suite run
    /// with it set can leave processes behind, which is exactly why nothing
    /// sets it by default.
    pub fn shutdown_within(mut self, budget: Duration) -> Result<(), String> {
        let pid = self.child.id();
        signal_term(pid);
        let deadline = Instant::now() + budget;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for the server") {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("the server exited uncleanly: {status:?}"))
                };
            }
            if Instant::now() >= deadline {
                let stuck = crate::platform::thread_states(pid);
                let census = self.census();
                let fate = if preserve() {
                    // Consuming `self` is not enough to stop `Drop` from
                    // killing it, so the handle is leaked on purpose: the
                    // harness that asked for this is the one that will clean up.
                    std::mem::forget(self);
                    "it has been left running for the spike harness to examine"
                } else {
                    // Consuming `self` is not enough: `Drop` kills, but only
                    // once this value is dropped, and the caller is owed a live
                    // process table in the message it is about to print.
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    "it has been SIGKILLed so the suite can go on"
                };
                return Err(format!(
                    "the server did not finish shutting down within {budget:?} of SIGTERM \
                     — the drain wedge (R-M1-2) is what this looks like; {fate}.\n\
                     {census}{stuck}"
                ));
            }
            std::thread::sleep(TICK);
        }
    }

    /// What the server said it was still draining, or why it said nothing.
    fn census(&self) -> String {
        let path = self.runtime_dir.join(CENSUS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(census) => format!("drain census ({}):\n{census}", path.display()),
            Err(err) => format!(
                "no drain census at {} ({err}) — the drain had not yet overrun \
                 its census interval, or could not write there\n",
                path.display()
            ),
        }
    }
}

/// Whether a wedged server is left alive instead of killed.
///
/// See [`ServerChild::shutdown_within`].
fn preserve() -> bool {
    std::env::var_os(PRESERVE).is_some()
}

/// The variable that turns the preserve behaviour on.
const PRESERVE: &str = "AMX_SPIKE_PRESERVE";

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

/// Assert the socket an environment rooted at `dir` and named `tag` will bind
/// spends no more than [`SUN_BUDGET`] below `$TMPDIR`.
///
/// Measured below `$TMPDIR` rather than absolutely, because the absolute length
/// is the runner's business: `/tmp` on Linux and a 48-byte private directory on
/// darwin, for the same harness and the same tag.
///
/// # Panics
///
/// If the path is over budget — which is the point.
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

/// Poll `cond` until it holds, failing the test if it never does.
///
/// This is the one place the harness waits: a condition and a deadline, so a
/// test can only ever be slow when the thing it waits for is slow — expiry is
/// a failure, never a green path.
///
/// This wait reads nothing. While a [`crate::term::Terminal`] is attached,
/// wait on it instead ([`crate::term::Terminal::wait_until`]): a client
/// nobody drains wedges on darwin's shallow pty queue, and input already
/// typed stops taking effect while this loop watches for its consequences.
pub fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(TICK);
    }
}

/// One poll interval, awaited.
///
/// The async sibling of [`wait_until`]'s tick, and it lives here for the same
/// reason that one does: `hygiene.rs` allows a nap in the harness's wait
/// helpers and nowhere else, so a suite whose condition has to be `await`ed —
/// anything asked over the wire — paces its loop with this rather than growing
/// its own interval. The deadline is still the suite's, and its expiry is still
/// the failure path.
pub async fn tick() {
    tokio::time::sleep(TICK).await;
}

/// [`wait_until`], with a diagnostic to name what was actually observed when
/// the wait expires — a timeout that only says "not yet" leaves a CI-only
/// failure undiagnosable.
pub fn wait_until_or(what: &str, mut cond: impl FnMut() -> bool, mut seen: impl FnMut() -> String) {
    let deadline = Instant::now() + PATIENCE;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting until {what}: {}",
            seen()
        );
        std::thread::sleep(TICK);
    }
}

/// How many live processes have `marker` in their argv.
///
/// A claim like "the pane's process survived the client dying" is about
/// processes, and only the process table can attest to it. Markers are
/// per-test-unique strings planted in the spawned command. The reader is
/// per-platform (see [`crate::platform::processes_with_arg`]) and panics
/// rather than answer a vacuous zero.
#[must_use]
pub fn processes_with_arg(marker: &str) -> usize {
    crate::platform::processes_with_arg(marker)
}

/// The pids of every live process whose argv holds `marker`.
///
/// [`processes_with_arg`] answers "how many", which is enough for "the pane's
/// process survived". A live upgrade needs the sharper question — M3's exit
/// criterion is "every child pid alive across the swap", and five processes
/// before and five after is also what a session that killed five and started
/// five would look like.
#[must_use]
pub fn pids_with_arg(marker: &str) -> Vec<u32> {
    crate::platform::pids_with_arg(marker)
}

/// Send `SIGWINCH` by hand to every live process whose argv holds `marker`;
/// the answer is how many were signalled.
///
/// A resize diagnostic, not a wait: when a `trap ... WINCH` never fired, the
/// question is which half died — the winsize change, or the signal the kernel
/// owes the terminal's foreground group for it. A hand-delivered signal that
/// makes the trap record the *new* size convicts the lost signal; one that
/// makes it re-record the old size convicts the resize itself.
pub fn signal_winch(marker: &str) -> usize {
    let pids = crate::platform::pids_with_arg(marker);
    for &pid in &pids {
        if let Some(pid) = rustix::process::Pid::from_raw(pid.cast_signed()) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::WINCH);
        }
    }
    pids.len()
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
