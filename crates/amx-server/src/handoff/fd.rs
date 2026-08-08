//! The descriptor transfer: one pty master per message.
//!
//! One fd per message, whose 1-byte payload is the pane's manifest index
//! (D-M3-6 point 3). herdr batches every descriptor into a single `sendmsg`
//! capped at 64 panes, under the kernel's SCM_MAX_FD of 253, so a 65-pane
//! session cannot upgrade at all; per-pane messages have no such cliff, pair
//! descriptors with entries deterministically, and name the exact pane when a
//! receive fails. They also make one misconfiguration structurally impossible:
//! unix(7) has the kernel *close* the excess descriptors when the receiver's
//! ancillary buffer is too small, and a message that carries exactly one fd
//! cannot have an excess. The cost is a `sendmsg` per pane rather than one for
//! all of them (R-M3-11).
//!
//! The descriptors themselves come from
//! [`PtyActorHandle::dup_fd`](crate::pty::PtyActorHandle::dup_fd), which
//! answers on the actor thread and only in `Quiesced` (D-M3-3).
//!
//! # What the one-byte payload costs
//!
//! A `u8` index addresses [`MAX_PANES`] entries and no more. The cliff is
//! therefore moved from herdr's 64 to 256 rather than removed, and the
//! exporter refuses past it by name
//! ([`HandoffError::TooManyPanes`](super::protocol::HandoffError::TooManyPanes))
//! instead of wrapping into a silently mispaired descriptor. Widening the
//! payload is a one-line change behind the state machine if a session ever
//! gets there.
//!
//! # Receiving is not `read`
//!
//! A descriptor rides *beside* its byte, not in it. Anything that consumes
//! that byte with an ordinary `read(2)` gets the byte and the kernel closes
//! the descriptor — silently, with no error anywhere. That is why the line
//! codec beside this module never reads past a newline
//! ([`protocol::wire`](super::protocol::wire)) and why every descriptor
//! message is taken by [`recv`] alone.

use std::io;
use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::io::Errno;
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};

/// Payload bytes in a descriptor message: the pane's manifest index.
pub const PANE_INDEX_LEN: usize = 1;

/// Panes one handoff can carry, from the width of that index.
pub const MAX_PANES: usize = u8::MAX as usize + 1;

/// Flags for the receiving `recvmsg`.
///
/// `MSG_CMSG_CLOEXEC` closes the window in which a concurrent `exec` could
/// inherit a pane's terminal. Darwin has no such flag — its `recvmsg` cannot
/// be asked for it — so [`recv`] sets the bit explicitly afterwards on both
/// platforms; the flag only narrows the race where the kernel offers to.
#[cfg(not(target_os = "macos"))]
const RECV_FLAGS: RecvFlags = RecvFlags::CMSG_CLOEXEC;
#[cfg(target_os = "macos")]
const RECV_FLAGS: RecvFlags = RecvFlags::empty();

/// What went wrong carrying one descriptor across.
#[derive(Debug, thiserror::Error)]
pub enum FdError {
    /// The peer closed the socket instead of sending the next descriptor.
    #[error("the handoff peer closed the socket before the descriptor arrived")]
    PeerGone,
    /// A descriptor message arrived carrying more than the one fd it may.
    #[error("a descriptor message carried {count} descriptors, not one")]
    NotOne {
        /// How many the peer attached.
        count: usize,
    },
    /// A message arrived with the pane index but no descriptor beside it.
    #[error("a descriptor message carried its pane index and no descriptor")]
    Missing,
    /// The payload was not the single index byte the protocol defines.
    #[error("a descriptor message carried {bytes} payload bytes, not {PANE_INDEX_LEN}")]
    Payload {
        /// How many payload bytes the message actually had.
        bytes: usize,
    },
    /// The kernel truncated the ancillary data, so descriptors were dropped.
    ///
    /// Unreachable by construction — the receive buffer has room for twice
    /// what the protocol permits — and reported rather than assumed away,
    /// because the failure it stands for is a silently closed terminal.
    #[error("the kernel truncated a descriptor message's ancillary data")]
    Truncated,
    /// The socket itself failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Send one pty master with the pane's manifest index beside it.
///
/// `index` addresses the manifest entry this terminal belongs to, so the
/// receiver pairs the two without depending on arrival order having survived
/// anything.
pub fn send(socket: BorrowedFd<'_>, index: u8, master: BorrowedFd<'_>) -> Result<(), FdError> {
    let payload = [index];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let carried = [master];
    loop {
        let mut control = SendAncillaryBuffer::new(&mut space);
        // The buffer is sized for exactly one descriptor and this pushes
        // exactly one, so the message always fits. `push` still answers, and
        // an unchecked `true` would be the kind of assumption this module
        // exists to avoid.
        if !control.push(SendAncillaryMessage::ScmRights(&carried)) {
            return Err(FdError::Truncated);
        }
        let sent = sendmsg(
            socket,
            &[IoSlice::new(&payload)],
            &mut control,
            SendFlags::empty(),
        );
        return match sent {
            Ok(PANE_INDEX_LEN) => Ok(()),
            Ok(bytes) => Err(FdError::Payload { bytes }),
            Err(Errno::INTR) => continue,
            Err(err) => Err(FdError::Io(err.into())),
        };
    }
}

/// Receive one descriptor and the pane index that came with it.
///
/// The returned fd is the *same open file description* the sender holds —
/// unix(7): an SCM_RIGHTS transfer is "semantically equivalent to `dup(2)`
/// into the file descriptor table of another process" — so it shares the
/// sender's file position and flags, and the terminal survives until the last
/// reference to it closes.
pub fn recv(socket: BorrowedFd<'_>) -> Result<(u8, OwnedFd), FdError> {
    // Room for two descriptors so a peer that attached more than one is
    // *seen*. Sized to exactly one, the kernel would close the excess and the
    // message would look well-formed.
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
    let mut payload = [0u8; PANE_INDEX_LEN];
    loop {
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let message = match recvmsg(
            socket,
            &mut [IoSliceMut::new(&mut payload)],
            &mut control,
            RECV_FLAGS,
        ) {
            Ok(message) => message,
            Err(Errno::INTR) => continue,
            Err(err) => return Err(FdError::Io(err.into())),
        };

        // Anything taken out of the buffer below is owned here; anything left
        // in it is closed when the buffer drops. Every early return past this
        // point therefore closes what it refuses rather than leaking it.
        let mut received: Vec<OwnedFd> = Vec::with_capacity(1);
        for ancillary in control.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = ancillary {
                received.extend(fds);
            }
        }

        if message.bytes == 0 && received.is_empty() {
            return Err(FdError::PeerGone);
        }
        if message.flags.contains(ReturnFlags::CTRUNC) {
            return Err(FdError::Truncated);
        }
        if message.bytes != PANE_INDEX_LEN {
            return Err(FdError::Payload {
                bytes: message.bytes,
            });
        }
        let master = match received.len() {
            1 => received.remove(0),
            0 => return Err(FdError::Missing),
            count => return Err(FdError::NotOne { count }),
        };
        // Darwin cannot be asked for this during the receive, and on Linux it
        // is already set; doing it unconditionally keeps one rule for both.
        rustix::io::fcntl_setfd(master.as_fd(), rustix::io::FdFlags::CLOEXEC)
            .map_err(|err| FdError::Io(err.into()))?;
        return Ok((payload[0], master));
    }
}
