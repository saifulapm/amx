//! The successor's side of the staged commit.
//!
//! The mirror of [`exporter`](super::exporter), with the strict abort rule
//! written into the types: the only state that can serve is the one
//! [`await_commit`](Importer::await_commit) returns, and the only way to reach
//! it is to have received the manifest, validated it, taken every descriptor,
//! restored, bound the session socket, and heard `committed`. There is no
//! method anywhere that skips a step, so **no partial import ever serves** is
//! a property of the API rather than a rule the assembly has to remember.

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;

use super::{HandoffError, PreCommit, Stage, Timeouts, Token, Wire, wire};
use crate::handoff::fd;

/// Step 5: the manifest is on the way.
#[derive(Debug)]
pub struct AwaitingManifest;

/// Steps 6–7: the manifest is in hand and the verdict is the caller's.
#[derive(Debug)]
pub struct Validating;

/// Steps 8–9: the descriptors are on the way.
#[derive(Debug)]
pub struct ReceivingMasters {
    panes: usize,
}

/// Step 10: the successor is the caller's to assemble, quiescent.
#[derive(Debug)]
pub struct Restoring;

/// Steps 11–12: the session socket is the caller's to probe for and bind.
#[derive(Debug)]
pub struct Binding;

/// Step 13: waiting for the commit, with nothing serving yet.
#[derive(Debug)]
pub struct AwaitingCommit;

/// Step 14: the session is this process's; resume the panes, then say so.
#[derive(Debug)]
pub struct Resuming;

impl PreCommit for AwaitingManifest {}
impl PreCommit for Validating {}
impl PreCommit for ReceivingMasters {}
impl PreCommit for Restoring {}
impl PreCommit for Binding {}
impl PreCommit for AwaitingCommit {}

/// The importer, at one stage of §3.
#[derive(Debug)]
pub struct Importer<S> {
    wire: Wire,
    state: S,
}

impl Importer<AwaitingManifest> {
    /// Step 2: connect to the exporter's socket and prove who this is.
    ///
    /// `token` came off this process's stdin ([`Token::read_from`]) and never
    /// off its command line.
    pub fn connect(path: &Path, token: &Token, timeouts: Timeouts) -> Result<Self, HandoffError> {
        let stage = Stage::Token;
        let stream =
            UnixStream::connect(path).map_err(|source| HandoffError::Io { stage, source })?;
        let mut wire = Wire { stream, timeouts };
        wire.deadline(stage, timeouts.stage)?;
        wire.send(
            stage,
            &wire::Line::Token {
                token: token.expose().to_owned(),
            },
        )?;
        Ok(Self {
            wire,
            state: AwaitingManifest,
        })
    }

    /// Step 5 arriving: the session, as the codec's document.
    ///
    /// A refused token surfaces here rather than at [`connect`](Self::connect):
    /// the exporter answers a bad one with an abort, so what this reads is
    /// [`HandoffError::Aborted`] at the manifest stage.
    pub fn recv_manifest(
        mut self,
    ) -> Result<(serde_json::Value, Importer<Validating>), HandoffError> {
        let stage = Stage::Manifest;
        let line = self.wire.recv(stage, wire::MAX_MANIFEST_LINE)?;
        let wire::Line::Manifest { manifest } = line else {
            return Err(self.wire.refuse(HandoffError::OutOfOrder {
                stage,
                expected: "manifest",
                got: line.name(),
            }));
        };
        Ok((
            manifest,
            Importer {
                wire: self.wire,
                state: Validating,
            },
        ))
    }
}

impl Importer<Validating> {
    /// Step 7: the manifest reads, and `panes` descriptors are expected.
    ///
    /// `panes` comes from the manifest the caller just checked — its read
    /// window, its session directory identity, its pane count (§3 step 6) —
    /// so the number of descriptors this importer will accept is fixed before
    /// the first one arrives. A manifest that does not check out goes to
    /// [`abort`](Importer::abort) instead, and nothing is received at all.
    pub fn validated(mut self, panes: usize) -> Result<Importer<ReceivingMasters>, HandoffError> {
        if panes > fd::MAX_PANES {
            return Err(self.wire.refuse(HandoffError::TooManyPanes {
                panes,
                max: fd::MAX_PANES,
            }));
        }
        self.wire.send(Stage::Validated, &wire::Line::Validated)?;
        Ok(Importer {
            wire: self.wire,
            state: ReceivingMasters { panes },
        })
    }
}

impl Importer<ReceivingMasters> {
    /// Step 8 arriving: exactly the descriptors the manifest promised.
    ///
    /// Each message's one payload byte is the manifest index its descriptor
    /// belongs to, and it must be the position this receive is at. A mismatch
    /// is not repaired by reordering — it means the two sides disagree about
    /// what the manifest says — so it aborts, and the position in the returned
    /// vector is the manifest index for every entry.
    pub fn recv_masters(mut self) -> Result<(Vec<OwnedFd>, Importer<Restoring>), HandoffError> {
        let bound = self.wire.timeouts.stage;
        self.wire.deadline(Stage::Masters, bound)?;
        let mut masters = Vec::with_capacity(self.state.panes);
        for position in 0..self.state.panes {
            let received = fd::recv(self.wire.stream.as_fd());
            let (index, master) = match received {
                Ok(taken) => taken,
                Err(fd::FdError::PeerGone) => {
                    return Err(HandoffError::PeerGone {
                        stage: Stage::Masters,
                    });
                }
                Err(source) if timed_out_fd(&source) => {
                    return Err(HandoffError::Timeout {
                        stage: Stage::Masters,
                    });
                }
                Err(source) => {
                    return Err(self
                        .wire
                        .refuse(HandoffError::Descriptor { position, source }));
                }
            };
            if usize::from(index) != position {
                return Err(self
                    .wire
                    .refuse(HandoffError::PaneIndexMismatch { position, index }));
            }
            masters.push(master);
        }
        Ok((
            masters,
            Importer {
                wire: self.wire,
                state: Restoring,
            },
        ))
    }
}

impl Importer<Restoring> {
    /// Step 10: the successor is assembled and touching no terminal.
    ///
    /// Saying this is what makes the exporter retire its gateway, so it is a
    /// claim about having everything, not about being ready to serve.
    pub fn restored(mut self) -> Result<Importer<Binding>, HandoffError> {
        let bound = self.wire.timeouts.stage;
        self.wire.deadline(Stage::Restored, bound)?;
        self.wire.send(Stage::Restored, &wire::Line::Restored)?;
        Ok(Importer {
            wire: self.wire,
            state: Binding,
        })
    }
}

impl Importer<Binding> {
    /// Step 12: the session socket is bound and new connections are answered.
    ///
    /// Call after the probe loop found the path free
    /// ([`Timeouts::socket_free`]) and the gateway bound it. The session is
    /// visible from here on, with frozen grids and full state — and still
    /// uncommitted, which is why every pane stays quiescent.
    pub fn ready(mut self) -> Result<Importer<AwaitingCommit>, HandoffError> {
        let bound = self.wire.timeouts.stage;
        self.wire.deadline(Stage::Ready, bound)?;
        self.wire.send(Stage::Ready, &wire::Line::Ready)?;
        Ok(Importer {
            wire: self.wire,
            state: AwaitingCommit,
        })
    }
}

impl Importer<AwaitingCommit> {
    /// Step 13: wait for ownership to transfer.
    ///
    /// A failure here is §3's hardest row: **strict abort.** An exporter that
    /// is wedged rather than dead must never meet a live importer, so a commit
    /// that does not arrive means unlink anything this process bound and exit
    /// non-zero, serving nothing. Recovery is the user's `amx`, which restores
    /// from the snapshot the exporter never overwrote.
    pub fn await_commit(mut self) -> Result<Importer<Resuming>, HandoffError> {
        self.wire.expect(Stage::Committed, &wire::Line::Committed)?;
        Ok(Importer {
            wire: self.wire,
            state: Resuming,
        })
    }
}

impl Importer<Resuming> {
    /// Step 14: say the session is being served, having resumed every pane.
    ///
    /// Advisory, and deliberately unfailable: ownership moved at the commit,
    /// so an exporter that never hears this is one that waited 500 ms and got
    /// on with dying.
    pub fn owned(mut self) {
        self.wire.tell(&wire::Line::Owned);
    }
}

/// Whether a descriptor receive expired on the stage's bound rather than
/// failing.
///
/// `SO_RCVTIMEO` reaches `recvmsg` the same way it reaches a line read, and a
/// silent peer at the descriptor stage means the same thing there as anywhere
/// else in §3.
fn timed_out_fd(err: &fd::FdError) -> bool {
    matches!(err, fd::FdError::Io(source) if matches!(
        source.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ))
}

impl<S: PreCommit> Importer<S> {
    /// Refuse this handoff, and say why.
    ///
    /// The only ending an importer has other than owning everything: unlink
    /// whatever it bound, close whatever descriptors it took — the kernel does
    /// that when the process exits — and serve nothing. Exists only before the
    /// commit; there is no such method on an [`Importer<Resuming>`].
    pub fn abort(mut self, reason: &str) {
        self.wire.tell(&wire::Line::Abort {
            reason: reason.to_owned(),
        });
    }
}
