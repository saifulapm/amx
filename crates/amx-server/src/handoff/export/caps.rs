//! The pre-flight probe, and the successor it decides to start.
//!
//! D-M3-6 point 2: herdr validates its successor *after* pausing every pane,
//! and pays a full quiesce and rollback for a binary it could have rejected for
//! free. amx runs `<binary> _handoff-caps` first — one exec, JSON on stdout —
//! and refuses before a single pane is touched. The check is a **window** check
//! on both surfaces, not an equality check: a successor that reads manifest v1
//! takes a v1 session whatever its own package version is, which is what lets
//! the handoff surface itself skew N/N−1.
//!
//! The probe runs on the connection task that asked for the handoff, which is
//! the whole reason "refused before any pane is touched" is structural rather
//! than a matter of ordering inside one function: `Core` never sees a
//! `session.handoff` whose pre-flight has not already finished.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use amx_core::SessionName;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::handoff::manifest;
use crate::handoff::protocol::Token;

/// How long `<binary> _handoff-caps` may take.
///
/// Far below §3's 30 s stage bound on purpose: this is one exec that prints
/// three constants, so a binary that has not answered in ten seconds is not a
/// binary this session should be handed to. The child is killed when the
/// future is dropped, so a hung successor costs the bound and not a process.
pub const PRE_FLIGHT_BOUND: Duration = Duration::from_secs(10);

/// Bytes of `_handoff-caps` output that will be read.
///
/// The document is under a hundred bytes; the cap is what keeps a wrong binary
/// that streams megabytes from being read into memory before it is refused.
const MAX_CAPS_BYTES: usize = 4 << 10;

/// What a binary can be handed, as `amx _handoff-caps` prints it.
///
/// Both windows are inclusive `[min, max]` pairs, which serde writes as JSON
/// arrays — the shape D-M3-6 names.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Caps {
    /// The binary's own package version, for the audit trail.
    pub version: String,
    /// The manifest schema versions it can read.
    pub handoff: (u32, u32),
    /// The control-protocol versions it speaks.
    pub proto: (u16, u16),
}

impl Caps {
    /// What this build answers with.
    ///
    /// The handoff window is [`manifest::READ_WINDOW`]'s ends rather than a
    /// second constant, so a build that learns to read another schema version
    /// says so here without anybody remembering to edit two places.
    #[must_use]
    pub fn of_this_build() -> Self {
        let lo = manifest::READ_WINDOW.iter().copied().min().unwrap_or(0);
        let hi = manifest::READ_WINDOW.iter().copied().max().unwrap_or(0);
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            handoff: (lo, hi),
            proto: amx_proto::version::window(),
        }
    }

    /// Whether this successor can be handed a manifest this build writes.
    fn reads_our_manifest(&self) -> bool {
        let (lo, hi) = self.handoff;
        lo <= manifest::VERSION && manifest::VERSION <= hi
    }

    /// Whether this successor still speaks to the clients this server does.
    ///
    /// The overlap, not equality: a client inside both windows keeps working
    /// across the swap, which is the skew clause 04 §4 exists for.
    fn overlaps_our_proto(&self) -> bool {
        let (their_lo, their_hi) = self.proto;
        let (our_lo, our_hi) = amx_proto::version::window();
        their_lo <= their_hi && their_lo.max(our_lo) <= their_hi.min(our_hi)
    }
}

/// The staged binary refused the pre-flight, or never answered it.
#[derive(Debug, Error)]
pub enum PreFlightError {
    /// The binary could not be run at all.
    #[error("could not run {binary} _handoff-caps: {source}")]
    Spawn {
        /// What was tried.
        binary: PathBuf,
        /// Why it did not start.
        source: io::Error,
    },
    /// It ran and did not answer inside [`PRE_FLIGHT_BOUND`].
    #[error("{binary} _handoff-caps did not answer within {bound:?}")]
    TimedOut {
        /// What was tried.
        binary: PathBuf,
        /// The bound it overran.
        bound: Duration,
    },
    /// It ran and failed.
    #[error("{binary} _handoff-caps exited {status}: {message}")]
    Refused {
        /// What was tried.
        binary: PathBuf,
        /// How it exited, as the platform spells it.
        status: String,
        /// The first line of its standard error, when it wrote one.
        message: String,
    },
    /// It ran, succeeded, and printed something else.
    #[error("{binary} _handoff-caps did not print handoff capabilities: {source}")]
    Unreadable {
        /// What was tried.
        binary: PathBuf,
        /// What the codec said.
        source: serde_json::Error,
    },
    /// It cannot read the manifest this build writes.
    #[error(
        "{binary} reads handoff manifests {lo}..={hi} and this server writes v{ours}; \
         nothing was touched"
    )]
    ManifestWindow {
        /// What was tried.
        binary: PathBuf,
        /// The oldest schema it reads.
        lo: u32,
        /// The newest schema it reads.
        hi: u32,
        /// The schema this build writes.
        ours: u32,
    },
    /// Its protocol window does not meet this server's.
    #[error(
        "{binary} speaks protocol {their_lo}..={their_hi} and this server speaks \
         {our_lo}..={our_hi}; every attached client would be stranded"
    )]
    ProtoWindow {
        /// What was tried.
        binary: PathBuf,
        /// The successor's oldest version.
        their_lo: u16,
        /// The successor's newest version.
        their_hi: u16,
        /// This build's oldest.
        our_lo: u16,
        /// This build's newest.
        our_hi: u16,
    },
}

/// Run `<binary> _handoff-caps` and decide whether this session may be handed
/// to it (§3 step 0).
///
/// Nothing in the session is touched by this function, by construction: it
/// takes a path and returns a verdict.
///
/// # Errors
///
/// [`PreFlightError`], one variant per way a staged binary can be the wrong
/// one to hand a live session to.
pub async fn pre_flight(binary: &Path) -> Result<Caps, PreFlightError> {
    let child = std::process::Command::new(binary)
        .arg("_handoff-caps")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| PreFlightError::Spawn {
            binary: binary.to_path_buf(),
            source,
        })?;
    // The pid stays here so the bound can be enforced: reading the child's
    // pipes to end-of-file is a blocking wait, and the only way to end one
    // early is to end the child. Nothing has reaped it yet — that is what the
    // blocking task is doing — so the pid is still this process's to signal.
    let pid = child.id();
    let waiting = tokio::task::spawn_blocking(move || child.wait_with_output());
    let output = match tokio::time::timeout(PRE_FLIGHT_BOUND, waiting).await {
        Ok(Ok(Ok(output))) => output,
        Ok(Ok(Err(source))) => {
            return Err(PreFlightError::Spawn {
                binary: binary.to_path_buf(),
                source,
            });
        }
        Ok(Err(join)) => {
            return Err(PreFlightError::Spawn {
                binary: binary.to_path_buf(),
                source: io::Error::other(join.to_string()),
            });
        }
        Err(_elapsed) => {
            kill_pid(pid);
            return Err(PreFlightError::TimedOut {
                binary: binary.to_path_buf(),
                bound: PRE_FLIGHT_BOUND,
            });
        }
    };

    if !output.status.success() {
        return Err(PreFlightError::Refused {
            binary: binary.to_path_buf(),
            status: output.status.to_string(),
            message: first_line(&output.stderr),
        });
    }
    let printed = &output.stdout[..output.stdout.len().min(MAX_CAPS_BYTES)];
    let caps: Caps =
        serde_json::from_slice(printed).map_err(|source| PreFlightError::Unreadable {
            binary: binary.to_path_buf(),
            source,
        })?;

    if !caps.reads_our_manifest() {
        return Err(PreFlightError::ManifestWindow {
            binary: binary.to_path_buf(),
            lo: caps.handoff.0,
            hi: caps.handoff.1,
            ours: manifest::VERSION,
        });
    }
    if !caps.overlaps_our_proto() {
        let (our_lo, our_hi) = amx_proto::version::window();
        return Err(PreFlightError::ProtoWindow {
            binary: binary.to_path_buf(),
            their_lo: caps.proto.0,
            their_hi: caps.proto.1,
            our_lo,
            our_hi,
        });
    }
    Ok(caps)
}

/// End a pre-flight that overran its bound.
///
/// `SIGKILL` rather than `SIGTERM`: this process has already been given ten
/// seconds to print three constants, and what is wanted is the blocking wait
/// behind it to end, not a negotiation.
fn kill_pid(pid: u32) {
    let Ok(raw) = i32::try_from(pid) else { return };
    if let Some(target) = rustix::process::Pid::from_raw(raw) {
        let _ = rustix::process::kill_process(target, rustix::process::Signal::KILL);
    }
}

/// The first line of a child's standard error, trimmed, or a stand-in.
fn first_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(&stderr[..stderr.len().min(MAX_CAPS_BYTES)]);
    let line = text.lines().find(|line| !line.trim().is_empty());
    line.map_or_else(
        || "it printed nothing".to_owned(),
        |line| line.trim().to_owned(),
    )
}

/// The process this session is being handed to (§3 step 1).
///
/// One implementation ships — [`StagedBinary`] — and the trait exists for one
/// reason: the only peer that can walk §3 is a process that speaks SCM_RIGHTS,
/// and the alternative to naming this seam is shipping a second binary whose
/// whole job is to be that peer in a test.
pub trait Successor: Send + 'static {
    /// Start the importer, with the token on its standard input.
    ///
    /// Blocking, like everything else on this path: the caller runs it inside
    /// the same `spawn_blocking` region as the protocol itself.
    ///
    /// # Errors
    ///
    /// Whatever starting the process cost.
    fn start(&mut self, socket: &Path, token: &Token) -> io::Result<()>;

    /// Ownership has transferred: this process is no longer ours to kill.
    ///
    /// Called exactly once, after `committed` is on the wire. Everything else
    /// — an abort at any stage, a panic, a dropped orchestrator — leaves the
    /// successor to be killed and reaped, which is §3's "kill+reap importer".
    fn disown(&mut self);
}

/// The successor as a real process:
/// `<binary> --session <name> server --handoff-import <socket>`.
///
/// The token rides the child's stdin and never its argv (D-M3-6 point 1):
/// `/proc/*/cmdline` is world-readable on Linux, and a secret that leaks is
/// worse than no secret.
///
/// **`--session` is load-bearing and was missing.** A server started as `amx
/// --session live server` selects its session from the flag, and the flag is
/// not in its environment — `AMX_SESSION` is the *other* way to say it and may
/// be unset. A successor spawned without it therefore built its paths for
/// `default`, and an upgrade of a named session committed happily onto the
/// wrong socket: `live`'s socket unlinked, `default`'s bound, and the panes
/// held by a server nobody could reach by name. Observed against the real
/// binary on 2026-08-08. The importer checks the same fact from its own side
/// (`Manifest::session_name`), so a future way of losing it is a refusal
/// instead of a silent swap.
#[derive(Debug)]
pub struct StagedBinary {
    binary: PathBuf,
    session: SessionName,
    child: Option<std::process::Child>,
    disowned: bool,
}

impl StagedBinary {
    /// A successor to be started from `binary`, for `session`.
    #[must_use]
    pub const fn new(binary: PathBuf, session: SessionName) -> Self {
        Self {
            binary,
            session,
            child: None,
            disowned: false,
        }
    }
}

impl Successor for StagedBinary {
    fn start(&mut self, socket: &Path, token: &Token) -> io::Result<()> {
        use std::io::Write as _;

        let mut child = std::process::Command::new(&self.binary)
            .arg("--session")
            .arg(self.session.as_str())
            .arg("server")
            .arg("--handoff-import")
            .arg(socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Taken, written and dropped: the importer reads one line and the
        // closed pipe is what ends its read even if it looks for more.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("the importer was started without a standard input"))?;
        let written = stdin
            .write_all(token.expose().as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        drop(stdin);
        if let Err(err) = written {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        self.child = Some(child);
        Ok(())
    }

    fn disown(&mut self) {
        self.disowned = true;
    }
}

impl Drop for StagedBinary {
    /// Kill and reap an importer that never took ownership.
    ///
    /// §3's 5–7 row says the exporter kills and reaps a refused importer, and
    /// every other pre-commit row ends the same way — an importer that is not
    /// serving must not be left behind holding descriptors. After
    /// [`disown`](Successor::disown) it is the session's server and this
    /// process is the one that is about to end.
    fn drop(&mut self) {
        if self.disowned {
            return;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
