//! The session socket: connect, negotiate, make control calls, carry streams.
//!
//! [`Session`] is the client's half of the same framing and handshake
//! `amx-server`'s `conn` module implements: `[u32 len][u8 channel][payload]`
//! over a `UnixStream`, a `Hello`/`Welcome` exchange, JSON-RPC calls on the
//! control channel, and bound binary streams on the rest.
//!
//! The same header discipline as the server's reader applies in this
//! direction too: **a declared length is never trusted with an allocation.**
//! Control frames are capped by the protocol
//! ([`MAX_CONTROL_FRAME`], enforced from the header by
//! [`FrameHeader::decode`]); stream frames are checked against the cap
//! [`Session::bind_channel`] recorded from the bind reply, and a frame on a
//! channel nothing bound is a protocol violation rather than a buffer size.
//! Outbound, the same caps apply before anything is written, so this client
//! can never send a frame the server is obliged to reject.
//!
//! Replies interleave with stream frames on one socket, so every read path
//! hands non-control frames to the caller ([`Session::call_with`]) instead of
//! failing on them.
//!
//! Taking a frame off the socket is [`read`]'s: it is cancel-safe by
//! construction — the whole of a read's progress lives in the session rather
//! than on the read future's stack — and its module header is where that, and
//! the [`Torn`] a dead connection is asked about, are explained.
//!
//! # Notifications
//!
//! JSON-RPC notifications — id-less by definition — are the server→client event
//! path (D-M2-5). Until V11 built the server's half there was nothing to
//! receive and this module dropped them; now a session that asked to
//! ([`Session::collect_notifications`]) *queues* every one, so an event that
//! arrives while a call is in flight is folded after the reply rather than
//! lost. [`Session::take_notifications`] is where the caller collects them.
//!
//! Asked to, not always: `amx events --json` reads frames off this same
//! `Session` and decodes the deliveries itself, and a queue it never drains
//! would be memory it never uses. A connection that has not said it wants them
//! behaves exactly as it did before this path existed.
//!
//! The queue is bounded, because a peer can publish faster than this client
//! folds and an unbounded buffer would be a leak with a nicer name. Overflow is
//! **reported, never silent**: `take_notifications` says that something was
//! dropped, and the client answers exactly as it answers a
//! [`Delivery::Gap`](amx_core::Delivery::Gap) — re-query state. Loss stays
//! visible and recoverable at this layer too (04 §2).

mod read;

use std::io;
use std::path::Path;

use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, MAX_CONTROL_FRAME};
use amx_proto::rpc::{Notification, Request, RequestId, Response, RpcOutcome};
use amx_proto::{ClientInfo, Feature, FrameError, FrameHeader, Hello, Resume, Welcome};
use serde_json::Value;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

pub use self::read::Torn;
use self::read::{ControlFrame, ReadState};

/// Channels are one byte, so a session can hold at most this many bindings.
const CHANNELS: usize = u8::MAX as usize + 1;

/// How many unfolded notifications a session holds before it starts reporting
/// loss instead of growing.
///
/// Generous on purpose: the client drains after every wake and after every
/// call, so reaching this means the consumer has stopped consuming, which is
/// the case the bound exists for. A resync costs one round trip; an unbounded
/// queue costs the process.
const MAX_PENDING_NOTIFICATIONS: usize = 1024;

/// A session-socket operation failed.
#[derive(Debug, Error)]
pub enum NetError {
    /// The socket failed.
    #[error("connection i/o failed: {0}")]
    Io(#[from] io::Error),
    /// The framing layer rejected a frame.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The peer closed the connection on a frame boundary.
    #[error("the server closed the connection")]
    Closed,
    /// A frame arrived on a channel nothing bound.
    #[error("frame on unbound channel {0}")]
    UnboundChannel(u8),
    /// A frame's payload was not the shape its channel requires.
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
    /// The server negotiated a protocol version this build does not speak.
    #[error("the server picked protocol {0}, outside this build's window")]
    BadProto(u16),
    /// The call reached the server and it refused it.
    #[error("call failed: {message} ({code})", message = .0.message, code = .0.code)]
    Call(amx_proto::RpcError),
}

impl NetError {
    /// Whether this failure is the connection itself going away.
    ///
    /// The distinction a reconnect turns on: a transport failure says nothing
    /// about the session, so redialling it is honest. Everything else — a
    /// refused call, a frame this build cannot read, a version nobody agreed
    /// on — would say exactly the same thing on a fresh connection, and
    /// retrying it would only hide it.
    #[must_use]
    pub const fn is_transport(&self) -> bool {
        matches!(self, Self::Io(_) | Self::Closed)
    }

    /// Whether this failure is a long poll the session abandoned rather than an
    /// answer to it.
    ///
    /// The other half of what a reconnect turns on, and the reason it is a code
    /// and not a message: a session handing over cancels every standing wait and
    /// says so *in a reply*, which arrives on a socket that is about to close
    /// but has not yet. A caller that can re-ask its question — a state
    /// predicate — should redial and ask it, exactly as it does when the socket
    /// dies first ([`RpcError::WAIT_ABANDONED`](amx_proto::RpcError::WAIT_ABANDONED)).
    #[must_use]
    pub const fn is_abandoned_wait(&self) -> bool {
        match self {
            Self::Call(error) => error.code == amx_proto::RpcError::WAIT_ABANDONED,
            _ => false,
        }
    }
}

/// Every feature this client build offers at handshake.
#[must_use]
pub fn offered_features() -> std::collections::BTreeSet<Feature> {
    std::collections::BTreeSet::from([
        Feature::GRID_STREAM,
        Feature::HISTORY_RANGES,
        Feature::RAW_PANE_IO,
        Feature::LOCAL_KEYBINDINGS,
    ])
}

/// Connect to the session socket at `path`.
pub async fn connect(path: &Path) -> Result<UnixStream, NetError> {
    Ok(UnixStream::connect(path).await?)
}

/// A negotiated connection: `Hello`/`Welcome` is done, control calls can be
/// made and bound streams read and written.
#[derive(Debug)]
pub struct Session {
    read: OwnedReadHalf,
    write: OwnedWriteHalf,
    next_id: u64,
    /// The frame cap per bound inbound channel; `None` means unbound.
    caps: Box<[Option<u32>; CHANNELS]>,
    /// How far the frame being read has got.
    reading: ReadState,
    /// The header bytes read so far, for the [`ReadState::Header`] phase.
    header: [u8; FRAME_HEADER_LEN],
    /// The payload bytes read so far.
    ///
    /// Swapped with the caller's buffer once a frame is whole, so a steady
    /// stream of frames trades two buffers back and forth rather than
    /// allocating one per frame.
    pending: Vec<u8>,
    /// What the frame just read turned out to be, when it was a control one.
    last_control: ControlFrame,
    /// Whether this session queues notifications for its caller at all.
    collecting: bool,
    /// Notifications read but not yet handed to the caller.
    notifications: Vec<Notification>,
    /// A notification was dropped because [`MAX_PENDING_NOTIFICATIONS`] was
    /// reached. Cleared by [`Session::take_notifications`], which reports it.
    notifications_lost: bool,
}

impl Session {
    /// Send `hello` and read the answering `Welcome`.
    ///
    /// `stream` must be freshly connected: this is the first exchange the
    /// protocol allows (04 §4). `attach` declares which of 04 §1's two roles
    /// this connection is: `true` for an attached client rendering the
    /// session (the server seeds a first workspace for an empty session on
    /// such a connection), `false` for a one-shot verb, which must never
    /// mutate the session by connecting.
    ///
    /// The negotiated version is checked against this build's own window —
    /// a welcome naming a version we never offered is a broken peer, and
    /// continuing would mean speaking a protocol nobody agreed on.
    pub async fn attach(
        stream: UnixStream,
        client: ClientInfo,
        attach: bool,
        resume: Option<Resume>,
    ) -> Result<(Self, Welcome), NetError> {
        let (read, write) = stream.into_split();
        let mut session = Self {
            read,
            write,
            next_id: 1,
            caps: Box::new([None; CHANNELS]),
            reading: ReadState::Header { got: 0 },
            header: [0; FRAME_HEADER_LEN],
            pending: Vec::new(),
            last_control: ControlFrame::default(),
            collecting: false,
            notifications: Vec::new(),
            notifications_lost: false,
        };

        let hello = Hello {
            proto: amx_proto::version::window(),
            features: offered_features(),
            client,
            attach,
            resume,
        };
        let payload =
            serde_json::to_vec(&hello).map_err(|_| NetError::Malformed("encode hello"))?;
        session.write_control(&payload).await?;

        let mut buf = Vec::new();
        let header = session.read_frame_into(&mut buf).await?;
        if !header.is_control() {
            return Err(NetError::UnboundChannel(header.channel));
        }
        let welcome: Welcome =
            serde_json::from_slice(&buf).map_err(|_| NetError::Malformed("decode welcome"))?;
        if !amx_proto::version::supports(welcome.proto) {
            return Err(NetError::BadProto(welcome.proto));
        }
        Ok((session, welcome))
    }

    /// Record a bound channel's frame cap, from a `stream.bind` reply.
    ///
    /// Binding is what admits frames on the channel at all — in both
    /// directions.
    pub fn bind_channel(&mut self, channel: u8, cap: u32) {
        if channel != CONTROL_CHANNEL {
            self.caps[usize::from(channel)] = Some(cap);
        }
    }

    /// Start queueing server-initiated notifications for
    /// [`Session::take_notifications`].
    ///
    /// Call it *before* `events.subscribe`, not after: the subscription's first
    /// deliveries can reach the socket ahead of its own reply, and a session
    /// that started collecting afterwards would have dropped them.
    pub fn collect_notifications(&mut self) {
        self.collecting = true;
    }

    /// Move every notification read since the last call into `into`, which is
    /// cleared first so one buffer can serve forever.
    ///
    /// Returns whether any notification was **dropped** on the way — the queue
    /// filled while nothing was draining it. A caller must treat `true`
    /// exactly as it treats a [`Delivery::Gap`](amx_core::Delivery::Gap): the
    /// deliveries are gone but the state they described is not, so re-query
    /// state and carry on. That equivalence is the whole reason this is
    /// reported rather than logged.
    #[must_use = "a dropped notification is a gap: the caller owes a state resync"]
    pub fn take_notifications(&mut self, into: &mut Vec<Notification>) -> bool {
        into.clear();
        std::mem::swap(&mut self.notifications, into);
        std::mem::take(&mut self.notifications_lost)
    }

    /// Send one JSON-RPC notification and wait for nothing.
    ///
    /// The client→server half of the id-less path, and the only shape flow
    /// control has: `stream.flow` is defined as a notification because a pause
    /// is a statement about what this client wants rather than a question about
    /// the stream, and the server's reader routes it without answering
    /// (`amx-server/src/conn/reader.rs`). Nothing is read here — a caller that
    /// waited for a reply to a notification would wait for the next frame the
    /// server happened to send.
    pub async fn notify(&mut self, notification: &Notification) -> Result<(), NetError> {
        let payload = serde_json::to_vec(notification)
            .map_err(|_| NetError::Malformed("encode notification"))?;
        self.write_control(&payload).await
    }

    /// Make one JSON-RPC call and wait for its reply, handing every stream
    /// frame that arrives meanwhile to `on_frame`.
    ///
    /// Notifications (no `id`) are queued rather than answered, and a reply for
    /// some other id is skipped rather than treated as an error — both mirror
    /// the server's own reader, and both are what lets a newer peer say things
    /// this build does not understand without costing it the session. An event
    /// that lands mid-call is therefore folded after the reply, not lost.
    pub async fn call_with(
        &mut self,
        method: &str,
        params: Value,
        mut on_frame: impl FnMut(FrameHeader, &[u8]),
    ) -> Result<Value, NetError> {
        let id = RequestId::Number(self.next_id);
        self.next_id += 1;
        let request = Request::new(id.clone(), method, Some(params));
        let payload =
            serde_json::to_vec(&request).map_err(|_| NetError::Malformed("encode request"))?;
        self.write_control(&payload).await?;

        let mut buf = Vec::new();
        loop {
            let header = self.read_frame_into(&mut buf).await?;
            if !header.is_control() {
                on_frame(header, &buf);
                continue;
            }
            let value = match std::mem::take(&mut self.last_control) {
                ControlFrame::Reply(value) => value,
                // Queued by `absorb_control`, or unreadable by this build.
                // Either way it is not the answer this call is waiting for.
                ControlFrame::Notification | ControlFrame::Other => continue,
                ControlFrame::NotJson => {
                    return Err(NetError::Malformed("control frame is not JSON"));
                }
            };
            let response: Response = serde_json::from_value(value)
                .map_err(|_| NetError::Malformed("decode response"))?;
            if response.id != id {
                continue;
            }
            return match response.outcome {
                RpcOutcome::Result(value) => Ok(value),
                RpcOutcome::Error(error) => Err(NetError::Call(error)),
            };
        }
    }

    /// Make one JSON-RPC call on a connection with no bound streams.
    ///
    /// What a one-shot verb uses; a stream frame arriving here is already a
    /// protocol violation (nothing was bound), and `read_frame_into` reports
    /// it as such.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, NetError> {
        self.call_with(method, params, |_, _| {}).await
    }

    /// Write one frame on a bound stream channel.
    pub async fn write_stream(&mut self, channel: u8, payload: &[u8]) -> Result<(), NetError> {
        let cap = self.caps[usize::from(channel)].filter(|_| channel != CONTROL_CHANNEL);
        let Some(cap) = cap else {
            return Err(NetError::UnboundChannel(channel));
        };
        let len =
            u32::try_from(payload.len()).map_err(|_| NetError::Malformed("payload too large"))?;
        if len > cap {
            return Err(NetError::Frame(FrameError::StreamFrameTooLarge {
                stream: amx_proto::stream::StreamId::new(u16::from(channel)),
                len: payload.len(),
                cap,
            }));
        }
        self.write_all(channel, len, payload).await
    }

    /// Write one control frame, enforcing the control cap outbound.
    async fn write_control(&mut self, payload: &[u8]) -> Result<(), NetError> {
        if payload.len() > MAX_CONTROL_FRAME {
            return Err(NetError::Frame(FrameError::ControlFrameTooLarge {
                len: payload.len(),
            }));
        }
        let len =
            u32::try_from(payload.len()).map_err(|_| NetError::Malformed("payload too large"))?;
        self.write_all(CONTROL_CHANNEL, len, payload).await
    }

    async fn write_all(&mut self, channel: u8, len: u32, payload: &[u8]) -> Result<(), NetError> {
        self.write
            .write_all(&FrameHeader::new(len, channel).encode())
            .await?;
        self.write.write_all(payload).await?;
        self.write.flush().await?;
        Ok(())
    }
}
