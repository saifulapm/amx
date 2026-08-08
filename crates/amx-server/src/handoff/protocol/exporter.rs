//! The running server's side of the staged commit.
//!
//! One type per stage, and one method per transition. The method consumes the
//! state it leaves, so the orchestrator (W06) cannot send descriptors before
//! the importer validated the manifest, cannot commit before the importer is
//! ready, and cannot abort after it has committed — none of those methods
//! exist on the state it is holding.
//!
//! The states are named for what the exporter still owes, not for what it has
//! done: an [`Exporter<Retiring>`] is one whose gateway is still answering.

use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;

use super::{Ending, HandoffError, HandoffListener, PreCommit, Stage, Timeouts, Token, Wire, wire};
use crate::handoff::fd;

/// Step 3: the importer has connected and its token line is on the way.
#[derive(Debug)]
pub struct Authenticating {
    listener: HandoffListener,
    token: Token,
}

/// Steps 4–5: the panes are the caller's to quiesce, then the manifest goes.
#[derive(Debug)]
pub struct SendingManifest;

/// Steps 6–7: the importer is reading the manifest.
#[derive(Debug)]
pub struct AwaitingValidated;

/// Step 8: the descriptors go, one message per pane.
#[derive(Debug)]
pub struct SendingMasters;

/// Steps 9–10: the importer is assembling itself.
#[derive(Debug)]
pub struct AwaitingRestored;

/// Step 11: the gateway is still answering and is the caller's to retire.
#[derive(Debug)]
pub struct Retiring;

/// Step 13: the final Persist push is the caller's to disarm, then commit.
#[derive(Debug)]
pub struct Committing;

/// Steps 14–15: ownership has moved; only the advisory ack is left.
#[derive(Debug)]
pub struct AwaitingOwned;

impl PreCommit for Authenticating {}
impl PreCommit for SendingManifest {}
impl PreCommit for AwaitingValidated {}
impl PreCommit for SendingMasters {}
impl PreCommit for AwaitingRestored {}
impl PreCommit for Retiring {}
impl PreCommit for Committing {}

/// The exporter, at one stage of §3.
#[derive(Debug)]
pub struct Exporter<S> {
    wire: Wire,
    state: S,
}

impl Exporter<Authenticating> {
    /// Wrap the connection [`HandoffListener::accept`] just took.
    pub(super) fn accepted(
        stream: UnixStream,
        timeouts: Timeouts,
        listener: HandoffListener,
        token: Token,
    ) -> Self {
        Self {
            wire: Wire { stream, timeouts },
            state: Authenticating { listener, token },
        }
    }

    /// Step 3: read the token line and check it.
    ///
    /// The handoff socket is unlinked on the way through, as §3 has it: from
    /// here on the connection is the whole channel, and the path in the
    /// runtime directory is one more thing that could be connected to.
    pub fn authenticate(mut self) -> Result<Exporter<SendingManifest>, HandoffError> {
        let stage = Stage::Token;
        let line = self.wire.recv(stage, wire::MAX_STAGE_LINE)?;
        let wire::Line::Token { token } = line else {
            return Err(self.wire.refuse(HandoffError::OutOfOrder {
                stage,
                expected: "token",
                got: line.name(),
            }));
        };
        if !self.state.token.matches(&token) {
            self.wire.tell(&wire::Line::Abort {
                reason: "the handoff token was refused".to_owned(),
            });
            return Err(HandoffError::BadToken);
        }
        // Dropping the listener unlinks the socket file.
        drop(self.state.listener);
        Ok(Exporter {
            wire: self.wire,
            state: SendingManifest,
        })
    }
}

impl Exporter<SendingManifest> {
    /// Step 5: send the manifest, after the panes are quiesced and captured.
    ///
    /// The manifest is the codec's document (W04) and passes through here
    /// unread.
    pub fn send_manifest(
        mut self,
        manifest: serde_json::Value,
    ) -> Result<Exporter<AwaitingValidated>, HandoffError> {
        let bound = self.wire.timeouts.stage;
        self.wire.deadline(Stage::Manifest, bound)?;
        self.wire
            .send(Stage::Manifest, &wire::Line::Manifest { manifest })?;
        Ok(Exporter {
            wire: self.wire,
            state: AwaitingValidated,
        })
    }
}

impl Exporter<AwaitingValidated> {
    /// Steps 6–7: wait for the importer's verdict on the manifest.
    pub fn await_validated(mut self) -> Result<Exporter<SendingMasters>, HandoffError> {
        self.wire.expect(Stage::Validated, &wire::Line::Validated)?;
        Ok(Exporter {
            wire: self.wire,
            state: SendingMasters,
        })
    }
}

impl Exporter<SendingMasters> {
    /// Step 8: one `sendmsg` per pane, in manifest order.
    ///
    /// The index each descriptor carries is its position in `masters`, so the
    /// pairing the importer checks is one this code cannot get wrong: there is
    /// no index argument to pass badly.
    pub fn send_masters(
        mut self,
        masters: &[BorrowedFd<'_>],
    ) -> Result<Exporter<AwaitingRestored>, HandoffError> {
        if masters.len() > fd::MAX_PANES {
            return Err(self.wire.refuse(HandoffError::TooManyPanes {
                panes: masters.len(),
                max: fd::MAX_PANES,
            }));
        }
        let bound = self.wire.timeouts.stage;
        self.wire.deadline(Stage::Masters, bound)?;
        for (position, master) in masters.iter().enumerate() {
            // In range: the length was checked against `MAX_PANES` just above,
            // and `MAX_PANES` is exactly what a `u8` addresses.
            let index = position as u8;
            if let Err(source) = fd::send(self.wire.stream.as_fd(), index, *master) {
                return Err(HandoffError::Descriptor { position, source });
            }
        }
        Ok(Exporter {
            wire: self.wire,
            state: AwaitingRestored,
        })
    }
}

impl Exporter<AwaitingRestored> {
    /// Step 10: wait for the successor to say it is assembled.
    ///
    /// Until this returns the session socket is still the exporter's and still
    /// answering, which is what keeps a racing `amx` from finding an absent
    /// server and daemonizing a rival (D-M3-6 point 4).
    pub fn await_restored(mut self) -> Result<Exporter<Retiring>, HandoffError> {
        self.wire.expect(Stage::Restored, &wire::Line::Restored)?;
        Ok(Exporter {
            wire: self.wire,
            state: Retiring,
        })
    }
}

impl Exporter<Retiring> {
    /// Step 12: wait for the successor to hold the session socket.
    ///
    /// Call after retiring the gateway — stop accepting, close connections,
    /// unlink the socket file — because the importer's probe loop is waiting
    /// for exactly that path to come free.
    pub fn await_ready(mut self) -> Result<Exporter<Committing>, HandoffError> {
        self.wire.expect(Stage::Ready, &wire::Line::Ready)?;
        Ok(Exporter {
            wire: self.wire,
            state: Committing,
        })
    }
}

impl Exporter<Committing> {
    /// Step 13: hand ownership over.
    ///
    /// Call after disarming the final Persist push: past this line the
    /// snapshot on disk is the successor's, and the exporter's dying capture
    /// would overwrite it (D-M3-6 point 5, R-M3-10).
    ///
    /// This is the last point at which anything can be undone. The state it
    /// returns has no `abort`.
    pub fn commit(mut self) -> Result<Exporter<AwaitingOwned>, HandoffError> {
        let bound = self.wire.timeouts.stage;
        self.wire.deadline(Stage::Committed, bound)?;
        self.wire.send(Stage::Committed, &wire::Line::Committed)?;
        Ok(Exporter {
            wire: self.wire,
            state: AwaitingOwned,
        })
    }
}

impl Exporter<AwaitingOwned> {
    /// Step 15: read the advisory ack, briefly.
    ///
    /// Returns whether it arrived. It cannot fail, because there is nothing a
    /// failure could mean: ownership moved at [`commit`](Exporter::commit) and
    /// the exporter's next move — release the pane actors, cancel, drain,
    /// exit — is the same either way.
    pub fn await_owned(mut self) -> bool {
        let bound = self.wire.timeouts.owned_ack;
        self.wire.deadline(Stage::Owned, bound).is_ok()
            && matches!(
                self.wire.recv(Stage::Owned, wire::MAX_STAGE_LINE),
                Ok(wire::Line::Owned)
            )
    }

    /// What this exporter is holding whatever happens next.
    ///
    /// §3's "after 13" row: the exporter is done regardless, and a leaked old
    /// process is the worst case rather than a lost session.
    pub const fn ending(&self) -> Ending {
        Ending::Committed
    }
}

impl<S: PreCommit> Exporter<S> {
    /// Tell the importer this handoff is over, and why.
    ///
    /// Best effort: the importer may already be gone, and the exporter's own
    /// obligation — resume every pane it quiesced, keep serving — does not
    /// depend on the line arriving. Exists only before the commit; there is no
    /// such method on an [`Exporter<AwaitingOwned>`].
    pub fn abort(mut self, reason: &str) -> Ending {
        self.wire.tell(&wire::Line::Abort {
            reason: reason.to_owned(),
        });
        Ending::Abort
    }
}
