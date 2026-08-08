//! The staged commit, as two typed state machines.
//!
//! `docs/09-m3-plan.md` §3 has the stages, the ownership at each step, and the
//! crash table; D-M3-6 has the five places amx departs from herdr's protocol
//! and why. [`Exporter`] and [`Importer`] are state machines whose transitions
//! are the only public surface: each one consumes the state it leaves and
//! returns the state it reached, so a caller cannot send descriptors before
//! validation — there is no method on the state it is holding. Illegal
//! orderings are unrepresentable rather than rejected.
//!
//! Stage timeouts are herdr's — 30 s per stage, 500 ms for the advisory `owned`
//! ack, 5 s for the socket-free probe loop ([`Timeouts`]) — and the abort rule
//! is herdr's kept strict: **no partial import ever serves.** The importer's
//! only two endings are "owned everything" and "exited having touched nothing
//! but descriptors that die with it", which is what [`Ending`] names.
//!
//! The token rides the importer's stdin, never argv: `/proc/*/cmdline` is
//! world-readable on Linux, and a secret that leaks is worse than no secret
//! (D-M3-6 point 1). [`Token::read_from`] is the importer's end of that pipe.
//!
//! # Blocking, on purpose
//!
//! Both machines are blocking calls over a `std::os::unix::net::UnixStream`,
//! bounded by `SO_RCVTIMEO`/`SO_SNDTIMEO`. Descriptor passing is `recvmsg` on
//! a raw descriptor and ancillary data has no async surface worth inventing
//! for a path that runs once per upgrade; the exporter's orchestrator (W06)
//! and the importer's assembly (W07) drive these from `spawn_blocking`.
//!
//! # Where each stage lives
//!
//! | §3 step | Exporter | Importer |
//! |---|---|---|
//! | 1–3 | [`HandoffListener::accept`], [`Exporter::authenticate`] | [`Importer::connect`] |
//! | 5 | [`Exporter::send_manifest`] | [`Importer::recv_manifest`] |
//! | 6–7 | [`Exporter::await_validated`] | [`Importer::validated`] |
//! | 8–9 | [`Exporter::send_masters`] | [`Importer::recv_masters`] |
//! | 10 | [`Exporter::await_restored`] | [`Importer::restored`] |
//! | 11–12 | [`Exporter::await_ready`] | [`Importer::ready`] |
//! | 13 | [`Exporter::commit`] | [`Importer::await_commit`] |
//! | 14–15 | [`Exporter::await_owned`] | [`Importer::owned`] |
//!
//! Steps 4, 11 and 13's own work — quiescing panes, retiring the gateway,
//! disarming the final Persist push — is the orchestrator's, and the state
//! names say where it goes: an [`Exporter<Retiring>`] is an exporter that has
//! not retired its gateway yet.

pub mod exporter;
pub mod importer;
pub mod wire;

use std::fmt;
use std::fs;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

use crate::handoff::fd::FdError;

pub use exporter::Exporter;
pub use importer::Importer;
pub use wire::Token;

/// Mode of the handoff socket: nobody but this user, ever.
const SOCKET_MODE: u32 = 0o600;

/// Where the exporter's private socket goes (§3).
///
/// Named for the exporter's pid so two sessions upgrading at once cannot
/// collide on it.
pub fn socket_path(runtime_dir: &Path, pid: u32) -> PathBuf {
    runtime_dir.join(format!("handoff-{pid}.sock"))
}

/// How long each stage may take.
///
/// The values are herdr's, kept: a stage that has not moved in 30 s is a peer
/// that is not coming back, and waiting longer turns an abort into a hang.
/// Constructing this explicitly is how tests drive the machines fast.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// Every stage's own bound.
    pub stage: Duration,
    /// The `owned` ack, which is advisory: ownership already transferred.
    pub owned_ack: Duration,
    /// How long the importer may wait for the session socket path to come
    /// free before binding it (§3 step 12; the loop itself is W07's).
    pub socket_free: Duration,
}

impl Timeouts {
    /// §3's constants.
    pub const DEFAULT: Self = Self {
        stage: Duration::from_secs(30),
        owned_ack: Duration::from_millis(500),
        socket_free: Duration::from_secs(5),
    };
}

impl Default for Timeouts {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Where in §3 a handoff was when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Steps 1–3: the socket, and the token that opens it.
    Token,
    /// Step 5: the manifest line.
    Manifest,
    /// Steps 6–7: the importer's verdict on it.
    Validated,
    /// Steps 8–9: the descriptors.
    Masters,
    /// Step 10: the successor assembled.
    Restored,
    /// Steps 11–12: the session socket changing hands.
    Ready,
    /// Step 13: the commit itself.
    Committed,
    /// Steps 14–15: the advisory ack, after ownership moved.
    Owned,
}

impl fmt::Display for Stage {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Token => "token",
            Self::Manifest => "manifest",
            Self::Validated => "validated",
            Self::Masters => "descriptor",
            Self::Restored => "restored",
            Self::Ready => "ready",
            Self::Committed => "committed",
            Self::Owned => "owned",
        })
    }
}

/// The two endings §3 allows, whichever side is left standing.
///
/// There is no third. A handoff that fails anywhere before the commit lands
/// leaves the exporter resuming its panes and going on serving, and the
/// importer exiting having touched nothing but descriptors that die with it;
/// a handoff that fails after it leaves the importer owning everything and the
/// exporter done regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Pre-commit. Undo, and serve what was already serving.
    Abort,
    /// Post-commit. Ownership moved; what failed was advisory.
    Committed,
}

/// What a stage refused with.
#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    /// The peer stopped answering inside the stage's bound.
    #[error("the handoff peer went quiet during the {stage} stage")]
    Timeout {
        /// Where it stalled.
        stage: Stage,
    },
    /// The peer's socket closed — it exited, or it was killed.
    #[error("the handoff peer closed the socket during the {stage} stage")]
    PeerGone {
        /// Where it went.
        stage: Stage,
    },
    /// The peer said so itself.
    #[error("the handoff peer aborted during the {stage} stage: {reason}")]
    Aborted {
        /// Where it aborted.
        stage: Stage,
        /// What it said.
        reason: String,
    },
    /// A well-formed message, at the wrong moment.
    #[error("expected {expected} during the {stage} stage, the peer sent {got}")]
    OutOfOrder {
        /// Where the conversation was.
        stage: Stage,
        /// What belonged there.
        expected: &'static str,
        /// What arrived instead.
        got: &'static str,
    },
    /// The importer did not prove it was the process the exporter spawned.
    #[error("the handoff token was refused")]
    BadToken,
    /// A descriptor arrived paired with the wrong manifest entry.
    #[error("descriptor {position} carries pane index {index}")]
    PaneIndexMismatch {
        /// Which descriptor of the run this was.
        position: usize,
        /// The index it claimed.
        index: u8,
    },
    /// More panes than a one-byte index can address (`fd::MAX_PANES`).
    #[error("a handoff carries at most {max} panes; this session has {panes}")]
    TooManyPanes {
        /// How many the session has.
        panes: usize,
        /// How many one handoff can carry.
        max: usize,
    },
    /// A descriptor did not cross.
    #[error("the descriptor for pane {position} did not cross: {source}")]
    Descriptor {
        /// Which descriptor of the run this was.
        position: usize,
        /// What the transfer said.
        source: FdError,
    },
    /// A line arrived that is not a message of this protocol.
    #[error("the {stage} line is not a protocol message: {source}")]
    Malformed {
        /// Where it arrived.
        stage: Stage,
        /// What the codec said.
        source: serde_json::Error,
    },
    /// A line ran past its cap.
    #[error("the {stage} line ran past its {cap}-byte cap")]
    LineTooLong {
        /// Where it arrived.
        stage: Stage,
        /// The cap in force.
        cap: usize,
    },
    /// The socket itself failed.
    #[error("the handoff socket failed during the {stage} stage: {source}")]
    Io {
        /// Where it failed.
        stage: Stage,
        /// What the kernel said.
        source: io::Error,
    },
}

impl HandoffError {
    /// Where in §3 this happened.
    pub const fn stage(&self) -> Stage {
        match self {
            Self::Timeout { stage }
            | Self::PeerGone { stage }
            | Self::Aborted { stage, .. }
            | Self::OutOfOrder { stage, .. }
            | Self::Malformed { stage, .. }
            | Self::LineTooLong { stage, .. }
            | Self::Io { stage, .. } => *stage,
            Self::BadToken => Stage::Token,
            Self::PaneIndexMismatch { .. }
            | Self::TooManyPanes { .. }
            | Self::Descriptor { .. } => Stage::Masters,
        }
    }

    /// What §3's crash table leaves the survivor holding.
    ///
    /// Everything before the commit aborts; the only failures past it are in
    /// the advisory ack, where ownership has already moved and neither side
    /// may undo anything.
    pub const fn ending(&self) -> Ending {
        match self.stage() {
            Stage::Owned => Ending::Committed,
            _ => Ending::Abort,
        }
    }
}

/// Turn a wire failure into a staged one.
fn staged(stage: Stage, err: wire::WireError) -> HandoffError {
    match err {
        wire::WireError::PeerGone => HandoffError::PeerGone { stage },
        wire::WireError::TooLong { cap } => HandoffError::LineTooLong { stage, cap },
        wire::WireError::Malformed(source) => HandoffError::Malformed { stage, source },
        wire::WireError::Io(source) if timed_out(&source) => HandoffError::Timeout { stage },
        wire::WireError::Io(source) => HandoffError::Io { stage, source },
    }
}

/// Whether an error is a stage bound expiring rather than a broken socket.
///
/// `SO_RCVTIMEO` surfaces as `EAGAIN` on both platforms this tree builds for,
/// which std reports as `WouldBlock`; `TimedOut` is here because the same
/// expiry is `ETIMEDOUT` on some socket types and the distinction is not one
/// this protocol wants to make.
fn timed_out(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// The socket and the bounds it is read under, shared by both machines.
#[derive(Debug)]
struct Wire {
    stream: UnixStream,
    timeouts: Timeouts,
}

impl Wire {
    /// Send one line.
    fn send(&mut self, stage: Stage, line: &wire::Line) -> Result<(), HandoffError> {
        wire::write_line(&mut self.stream, line).map_err(|err| staged(stage, err))
    }

    /// Send one line and do not care whether it arrived.
    ///
    /// Aborts and the advisory `owned` are the only messages that qualify: by
    /// the time either is written the sender's next move is the same whether
    /// the peer hears it or not.
    fn tell(&mut self, line: &wire::Line) {
        let _ = wire::write_line(&mut self.stream, line);
    }

    /// Read one line, mapping an abort into the failure it is.
    fn recv(&mut self, stage: Stage, cap: usize) -> Result<wire::Line, HandoffError> {
        self.deadline(stage, self.timeouts.stage)?;
        let read = wire::read_line(&mut self.stream, cap).map_err(|err| staged(stage, err));
        match read {
            Ok(wire::Line::Abort { reason }) => Err(HandoffError::Aborted { stage, reason }),
            Ok(line) => Ok(line),
            // A line this side judged unreadable is this side's verdict, and
            // the peer is owed it; a timeout or a closed socket has nobody
            // left to tell.
            Err(fault @ (HandoffError::Malformed { .. } | HandoffError::LineTooLong { .. })) => {
                Err(self.refuse(fault))
            }
            Err(fault) => Err(fault),
        }
    }

    /// Read the one line at this stage that belongs there.
    fn expect(&mut self, stage: Stage, want: &wire::Line) -> Result<(), HandoffError> {
        let line = self.recv(stage, wire::MAX_STAGE_LINE)?;
        if &line == want {
            Ok(())
        } else {
            Err(self.refuse(HandoffError::OutOfOrder {
                stage,
                expected: want.name(),
                got: line.name(),
            }))
        }
    }

    /// Report a fault this side found, so the peer hears why rather than
    /// meeting a socket that simply closed.
    ///
    /// A failed transition takes its state machine with it — that is what
    /// makes an illegal ordering unrepresentable — so this is the only chance
    /// to say anything at all.
    fn refuse(&mut self, fault: HandoffError) -> HandoffError {
        self.tell(&wire::Line::Abort {
            reason: fault.to_string(),
        });
        fault
    }

    /// Put this stage's bound on the socket.
    fn deadline(&self, stage: Stage, within: Duration) -> Result<(), HandoffError> {
        self.stream
            .set_read_timeout(Some(within))
            .and_then(|()| self.stream.set_write_timeout(Some(within)))
            .map_err(|source| HandoffError::Io { stage, source })
    }
}

/// The exporter's private socket: `<runtime_dir>/handoff-<pid>.sock`, 0600.
///
/// §3 has it removed once the importer has connected and authenticated, which
/// is what dropping this does — [`Exporter::authenticate`] drops it on the way
/// through, and any earlier failure unlinks it too.
#[derive(Debug)]
pub struct HandoffListener {
    listener: UnixListener,
    path: PathBuf,
}

impl HandoffListener {
    /// Bind the path, refusing to serve one that already exists.
    ///
    /// A leftover file here is not a stale session socket to be probed — it is
    /// a name nothing should have taken — so this reports it rather than
    /// clearing it.
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_MODE))?;
        Ok(Self { listener, path })
    }

    /// Where it is, for the importer's argv.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Wait for the importer to connect (§3 steps 1–2).
    pub fn accept(
        self,
        token: Token,
        timeouts: Timeouts,
    ) -> Result<Exporter<exporter::Authenticating>, HandoffError> {
        let stage = Stage::Token;
        self.listener
            .set_nonblocking(true)
            .map_err(|source| HandoffError::Io { stage, source })?;
        let bound = Timespec {
            tv_sec: timeouts.stage.as_secs().min(i64::MAX as u64) as _,
            tv_nsec: timeouts.stage.subsec_nanos() as _,
        };
        loop {
            let mut watched = [PollFd::from_borrowed_fd(
                self.listener.as_fd(),
                PollFlags::IN,
            )];
            match poll(&mut watched, Some(&bound)) {
                // A poll that saw nothing is the stage's bound expiring: the
                // importer never started, or never got far enough to connect.
                Ok(0) => return Err(HandoffError::Timeout { stage }),
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(err) => {
                    return Err(HandoffError::Io {
                        stage,
                        source: err.into(),
                    });
                }
            }
            return match self.listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .map_err(|source| HandoffError::Io { stage, source })?;
                    Ok(Exporter::accepted(stream, timeouts, self, token))
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                Err(source) => Err(HandoffError::Io { stage, source }),
            };
        }
    }
}

impl Drop for HandoffListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The stages at which a handoff may still be undone.
///
/// Implemented by every state before ownership transfers and by no state
/// after, so `abort` simply does not exist on a machine that has committed.
/// Nothing outside this module implements it, and nothing outside can profit
/// from doing so: the state machines' fields are private and their only
/// constructors are the transitions, so there is no way to hold an
/// `Exporter<T>` for a `T` this module did not name.
pub trait PreCommit {}
