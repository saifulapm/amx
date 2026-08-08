//! A successor that is not a process: W05's importer, on a thread.
//!
//! The exporter's peer has to speak SCM_RIGHTS, so it cannot be a script, and
//! the real one is `amx server --handoff-import` — W07's, and not written yet.
//! What *is* written is the importer state machine, so this drives it directly
//! over the same socket the real successor would, through the
//! [`Successor`](amx_server::handoff::export::Successor) seam the orchestrator
//! takes for exactly this reason.
//!
//! It is scriptable in one dimension only: where it stops. Every stop is a real
//! `abort` line at a real stage, which is what makes the abort matrix a matrix
//! rather than one path with three names.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc as sync_mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use amx_server::handoff::export::Successor;
use amx_server::handoff::manifest::Manifest;
use amx_server::handoff::protocol::{Importer, Timeouts, Token};
use amx_server::session::probe::{self, Probe};

/// How long the fake waits for the session socket to come free after the
/// exporter retires (§3 step 12's own constant).
const SOCKET_FREE: Duration = Duration::from_secs(5);

/// How often the probe loop looks.
const TICK: Duration = Duration::from_millis(5);

/// Where the fake importer stops, if it stops.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StopAt {
    /// Walk the whole protocol and take the session.
    Nowhere,
    /// Offer a token the exporter never minted (§3 step 3).
    ///
    /// Not a stop the successor chooses, but the earliest place the exporter
    /// can refuse one — and the exporter's obligation there is the same as at
    /// every other pre-commit stage: resume, and go on serving.
    Authenticating,
    /// Abort instead of validating the manifest (§3 step 7).
    Validating,
    /// Abort after taking the descriptors, instead of `restored` (step 10).
    Restoring,
    /// Abort after `restored`, instead of `ready` (step 12).
    ///
    /// The one that matters most: the exporter has already retired its gateway
    /// by the time it reads this, so aborting here is what makes it take its
    /// own socket back.
    Binding,
}

/// What the fake saw, in the order it saw it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Seen {
    /// It connected and its token was accepted.
    Authenticated,
    /// The manifest arrived, with this many pane entries.
    Manifest(usize),
    /// This many descriptors crossed.
    Masters(usize),
    /// `restored` was sent.
    Restored,
    /// The session socket, read the instant before `restored` went out.
    SocketBeforeRestored(Probe),
    /// The session socket, once the exporter had retired.
    SocketAfterRetire(Probe),
    /// Whether the socket *file* was still there after the retirement.
    FileAfterRetire(bool),
    /// `ready` was sent, with the session socket bound.
    Ready,
    /// `committed` arrived.
    Committed,
    /// `owned` was sent.
    Owned,
    /// It gave up, and why.
    Aborted(String),
}

/// The peer the orchestrator is handed.
pub struct Fake {
    session_socket: PathBuf,
    stop_at: StopAt,
    timeouts: Timeouts,
    seen: sync_mpsc::Sender<Seen>,
    thread: Option<JoinHandle<()>>,
}

impl Fake {
    /// A fake that walks the whole protocol.
    pub fn new(session_socket: PathBuf) -> (Self, sync_mpsc::Receiver<Seen>) {
        let (seen, log) = sync_mpsc::channel();
        (
            Self {
                session_socket,
                stop_at: StopAt::Nowhere,
                timeouts: Timeouts::DEFAULT,
                seen,
                thread: None,
            },
            log,
        )
    }

    /// Stop at `stop_at` and abort there.
    pub fn stopping_at(mut self, stop_at: StopAt) -> Self {
        self.stop_at = stop_at;
        self
    }

    /// Tighten the stage bounds, for a test that must not wait 30 s.
    pub fn within(mut self, stage: Duration) -> Self {
        self.timeouts.stage = stage;
        self
    }
}

impl Successor for Fake {
    fn start(&mut self, socket: &Path, token: &Token) -> io::Result<()> {
        let socket = socket.to_path_buf();
        let session_socket = self.session_socket.clone();
        let token = if self.stop_at == StopAt::Authenticating {
            Token::mint()
        } else {
            token.clone()
        };
        let stop_at = self.stop_at;
        let timeouts = self.timeouts;
        let seen = self.seen.clone();
        self.thread = Some(std::thread::spawn(move || {
            let told = |what: Seen| {
                let _ = seen.send(what);
            };
            if let Err(err) = walk(&socket, &session_socket, &token, timeouts, stop_at, &told) {
                told(Seen::Aborted(err));
            }
        }));
        Ok(())
    }

    fn disown(&mut self) {}
}

impl Drop for Fake {
    /// Join the thread rather than leaving it holding a socket.
    ///
    /// A real successor is a process the exporter kills; this one is a thread
    /// in the exporter's own address space, and a test that let it outlive its
    /// `Fake` would be a test whose next case races the previous case's peer.
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// §3 steps 2–14, from the successor's side.
fn walk(
    socket: &Path,
    session_socket: &Path,
    token: &Token,
    timeouts: Timeouts,
    stop_at: StopAt,
    told: &impl Fn(Seen),
) -> Result<(), String> {
    let importer = Importer::connect(socket, token, timeouts).map_err(|err| err.to_string())?;
    told(Seen::Authenticated);
    let (document, importer) = importer.recv_manifest().map_err(|err| err.to_string())?;
    let manifest: Manifest =
        serde_json::from_value(document).map_err(|err| format!("manifest: {err}"))?;
    manifest.check_version().map_err(|err| err.to_string())?;
    told(Seen::Manifest(manifest.panes.len()));
    if stop_at == StopAt::Validating {
        importer.abort("the test stops before validating");
        told(Seen::Aborted("validating".to_owned()));
        return Ok(());
    }

    let importer = importer
        .validated(manifest.panes.len())
        .map_err(|err| err.to_string())?;
    let (masters, importer) = importer.recv_masters().map_err(|err| err.to_string())?;
    told(Seen::Masters(masters.len()));
    if stop_at == StopAt::Restoring {
        importer.abort("the test stops before restoring");
        told(Seen::Aborted("restoring".to_owned()));
        return Ok(());
    }

    // The instant before `restored` goes out the exporter is still serving:
    // that is what keeps a racing `amx` from finding an absent server and
    // daemonizing a rival (D-M3-6 point 4).
    told(Seen::SocketBeforeRestored(
        probe::probe(session_socket).map_err(|err| err.to_string())?,
    ));
    let importer = importer.restored().map_err(|err| err.to_string())?;
    told(Seen::Restored);

    if stop_at == StopAt::Binding {
        importer.abort("the test stops before binding");
        told(Seen::Aborted("binding".to_owned()));
        return Ok(());
    }

    // §3 step 12's probe loop, and the two readings that prove the retirement
    // happened: the path answers nothing, and the file itself is gone.
    let free = wait_for_a_free_socket(session_socket)?;
    told(Seen::SocketAfterRetire(free));
    told(Seen::FileAfterRetire(session_socket.exists()));
    let listener =
        std::os::unix::net::UnixListener::bind(session_socket).map_err(|err| err.to_string())?;
    let importer = importer.ready().map_err(|err| err.to_string())?;
    told(Seen::Ready);

    let importer = importer.await_commit().map_err(|err| err.to_string())?;
    told(Seen::Committed);
    importer.owned();
    told(Seen::Owned);
    // Held until here so the socket really is this peer's for the length of the
    // commit, and released so the test's temp directory can be removed.
    drop(listener);
    let _ = std::fs::remove_file(session_socket);
    // The descriptors die with this thread's frame, which is exactly what a
    // real importer's would do if its process ended (§3's 8–10 row).
    drop(masters);
    Ok(())
}

/// Poll the session socket path until it can be bound (§3 step 12).
///
/// "Free" is two readings, not one, and the second is what makes this
/// deterministic: a retirement drops the listener before it unlinks the file,
/// so there is a window in which the path is refused (`Stale`) and the entry is
/// still there — and a peer that bound on the strength of the first reading
/// alone would be racing the exporter's `unlink`, not waiting for it.
fn wait_for_a_free_socket(path: &Path) -> Result<Probe, String> {
    let deadline = Instant::now() + SOCKET_FREE;
    loop {
        match probe::probe(path) {
            // Read again once the entry is gone, because the reading taken
            // *during* that window is `Stale` and the one after it is `Absent`
            // — a difference of one syscall's timing, and not something a test
            // should be asserting about.
            Ok(found) if !found.is_running() && !path.exists() => {
                return probe::probe(path).map_err(|err| err.to_string());
            }
            Ok(_) | Err(_) if Instant::now() >= deadline => {
                return Err(format!(
                    "the session socket {} was not free after {SOCKET_FREE:?}",
                    path.display()
                ));
            }
            _ => std::thread::sleep(TICK),
        }
    }
}
