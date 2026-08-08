//! Taking one frame off the socket, and what is left when that is interrupted.
//!
//! # Reading is cancel-safe
//!
//! [`Session::read_frame_into`] is one arm of the client loop's `select!`, and
//! the other arms — a keystroke, a `SIGWINCH`, the resize debounce — win races
//! against it routinely. Winning drops the read future, so **nothing a read has
//! taken off the socket may live on that future's stack**: a half-read frame
//! left there would be bytes the socket no longer has and the reader no longer
//! holds, and the next read would take a payload byte for a channel byte. That
//! is a desync the peer never caused, and one the unbound-channel refusal in
//! [`super`] would report as if it had.
//!
//! So the progress lives in the session ([`ReadState`], `Session::pending`): a
//! cancelled read resumes exactly where it stopped, whichever buffer the next
//! caller passes. The one place a read may be dropped without consequence is
//! before its first byte, which is where a `select!` that never wins its race
//! leaves it.
//!
//! # What is left when the socket ends instead
//!
//! The same state answers a second question M3 asks of it. A connection that
//! dies mid-frame has taken bytes off the socket that will never become a whole
//! message, and [`Session::torn`] is how a reattaching client finds out which
//! stream lost them — the "mid-delta at the disconnect" case it must not vouch
//! for ([`crate::app::reconnect`]).

use std::io;

use amx_proto::FrameHeader;
use amx_proto::frame::FRAME_HEADER_LEN;
use amx_proto::rpc::Notification;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{MAX_PENDING_NOTIFICATIONS, NetError, Session};

/// How far the frame currently being read has got.
///
/// The whole of a read's progress, held by the session rather than by the read
/// future, so cancelling that future costs nothing but the poll.
#[derive(Clone, Copy, Debug)]
pub(super) enum ReadState {
    /// `got` bytes of the next frame's header have been read.
    Header {
        /// Header bytes in hand, `0..FRAME_HEADER_LEN`.
        got: usize,
    },
    /// The header is decoded and its channel admitted; `got` payload bytes are
    /// in `Session::pending`.
    Payload {
        /// The decoded header.
        header: FrameHeader,
        /// Payload bytes in hand.
        got: usize,
    },
}

/// What one control frame turned out to be, decided once as it is read.
///
/// Control frames are decoded exactly once. The reply's `Value` is kept here
/// rather than re-parsed by [`Session::call_with`], so adding the notification
/// path did not add a second parse to the reply path.
#[derive(Debug, Default)]
pub(super) enum ControlFrame {
    /// A JSON-RPC response, already decoded to a `Value`.
    Reply(Value),
    /// A server-initiated notification, already queued.
    Notification,
    /// JSON, but neither of those: the `Welcome` of the handshake, or
    /// something a newer server says that this build has no reading of.
    #[default]
    Other,
    /// Not JSON at all, which is a broken peer rather than a skew.
    NotJson,
}

/// What a session was part way through reading when it was last looked at.
///
/// The question a reattaching client asks its dead connection: everything the
/// socket delivered whole is in the model, and this says what was not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Torn {
    /// Nothing was in flight: the last frame finished, or none had started.
    Nothing,
    /// A frame whose own header was cut in half, so not even its channel is
    /// known. Nothing this connection carried can be vouched for.
    Unknown,
    /// A frame on this channel.
    Channel(u8),
}

impl Session {
    /// What this session was part way through reading.
    ///
    /// A connection that dies mid-frame loses whatever that frame carried, and
    /// the bytes already taken off the socket are not enough to say what it
    /// was. For a grid stream that is the case a reattach must not vouch for:
    /// the pane's cells may be half a damage rect behind the generation this
    /// client would otherwise present.
    #[must_use]
    pub const fn torn(&self) -> Torn {
        match self.reading {
            // The one place a read may stop without consequence, which is also
            // where a clean disconnect leaves it.
            ReadState::Header { got: 0 } => Torn::Nothing,
            ReadState::Header { .. } => Torn::Unknown,
            ReadState::Payload { header, .. } => Torn::Channel(header.channel),
        }
    }

    /// Read one frame into `buf`, validating its declared length first.
    ///
    /// The payload buffer is sized only after the length passes the cap for
    /// its channel: a peer declaring 4 GiB costs a rejection, not 4 GiB.
    ///
    /// Cancel-safe, as this module's header requires: every byte this has taken
    /// off the socket is in the session, so dropping the future part way
    /// through a frame loses nothing and the next call finishes that same
    /// frame. Each step commits its progress before the next await, which is
    /// the only place cancellation can land.
    ///
    /// A control frame is decoded here and nowhere else: a notification joins
    /// the queue [`Session::take_notifications`] drains, and a reply is kept
    /// decoded for [`Session::call_with`] to match against its id.
    pub async fn read_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<FrameHeader, NetError> {
        loop {
            match self.reading {
                ReadState::Header { got } if got < FRAME_HEADER_LEN => {
                    let read = self.read.read(&mut self.header[got..]).await?;
                    if read == 0 {
                        // A zero-length read before the first byte is the one
                        // place a disconnect is not an error — the same
                        // distinction the server's reader draws between a
                        // clean disconnect and a torn frame.
                        return Err(if got == 0 {
                            NetError::Closed
                        } else {
                            NetError::Io(io::Error::from(io::ErrorKind::UnexpectedEof))
                        });
                    }
                    self.reading = ReadState::Header { got: got + read };
                }
                ReadState::Header { .. } => {
                    let header = FrameHeader::decode(self.header)?;
                    if !header.is_control() {
                        let cap = self.caps[usize::from(header.channel)]
                            .ok_or(NetError::UnboundChannel(header.channel))?;
                        header.check_stream_len(cap)?;
                    }
                    self.pending.clear();
                    self.pending.resize(header.payload_len(), 0);
                    self.reading = ReadState::Payload { header, got: 0 };
                }
                ReadState::Payload { header, got } if got < header.payload_len() => {
                    let read = self.read.read(&mut self.pending[got..]).await?;
                    if read == 0 {
                        return Err(NetError::Io(io::Error::from(io::ErrorKind::UnexpectedEof)));
                    }
                    self.reading = ReadState::Payload {
                        header,
                        got: got + read,
                    };
                }
                ReadState::Payload { header, .. } => {
                    self.reading = ReadState::Header { got: 0 };
                    buf.clear();
                    std::mem::swap(buf, &mut self.pending);
                    if header.is_control() {
                        self.last_control = self.absorb_control(buf);
                    }
                    return Ok(header);
                }
            }
        }
    }

    /// Decide what a control frame is, queueing it if it is a notification.
    ///
    /// A frame carrying an `id` is a reply and belongs to whichever call is
    /// waiting; one without is either a notification or something this build
    /// has no reading of, and both are skipped by the call loop rather than
    /// failing it — the same catch-all rule the server's reader follows, and
    /// what lets a newer peer speak without costing this one its session.
    fn absorb_control(&mut self, payload: &[u8]) -> ControlFrame {
        let Ok(value) = serde_json::from_slice::<Value>(payload) else {
            return ControlFrame::NotJson;
        };
        if value.get("id").is_some() {
            return ControlFrame::Reply(value);
        }
        let Ok(notification) = serde_json::from_value::<Notification>(value) else {
            return ControlFrame::Other;
        };
        // Nobody is draining a queue, so there is no queue. It is still not the
        // reply a call is waiting for, and the call loop skips it either way.
        if self.collecting {
            if self.notifications.len() >= MAX_PENDING_NOTIFICATIONS {
                self.notifications_lost = true;
            } else {
                self.notifications.push(notification);
            }
        }
        ControlFrame::Notification
    }
}
