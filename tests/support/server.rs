//! The `amx server` child a test owns, and what it can be asked about.
//!
//! Split out of [`crate::env`] when that module crossed the soft budget for the
//! second time — W01 raised it and named this as the seam, and W14 grew it
//! again. It is a self-contained concept: a pid to read `/proc` for, a bounded
//! stop that reports a wedge instead of hanging the suite, and the drain census
//! the server writes when one happens.

use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use amx_server::runtime::CENSUS_FILE;

use crate::env::{PATIENCE, TICK};

/// A foreground `amx server` child owned by the test.
#[derive(Debug)]
pub struct ServerChild {
    pub(crate) child: Child,
    /// Where the server writes its drain census if a shutdown overruns
    /// (`amx_server::runtime::CENSUS_FILE`). Held so a wedge report can name
    /// the task the drain is stuck on, not only the threads it is stuck in.
    pub(crate) runtime_dir: PathBuf,
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
