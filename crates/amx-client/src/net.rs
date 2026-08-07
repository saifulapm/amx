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
//! failing on them, and JSON-RPC notifications — id-less by definition — are
//! skipped rather than treated as malformed, mirroring the server's reader.

use std::io;
use std::path::Path;

use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN, MAX_CONTROL_FRAME};
use amx_proto::rpc::{Request, RequestId, Response, RpcOutcome};
use amx_proto::{ClientInfo, Feature, FrameError, FrameHeader, Hello, Resume, Welcome};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// Channels are one byte, so a session can hold at most this many bindings.
const CHANNELS: usize = u8::MAX as usize + 1;

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
    /// Bytes read off the socket but not yet returned as a whole frame.
    ///
    /// The wired client awaits [`Session::read_frame_into`] inside `select!`,
    /// where losing to another branch drops the read future. Everything read
    /// lands here before the next await, so a dropped call strands nothing
    /// and the next one resumes mid-frame instead of desyncing the stream.
    inbound: Vec<u8>,
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
            inbound: Vec::new(),
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

    /// Read one frame into `buf`, validating its declared length first.
    ///
    /// The payload buffer is sized only after the length passes the cap for
    /// its channel: a peer declaring 4 GiB costs a rejection, not 4 GiB.
    ///
    /// Cancel-safe, which the wired client's `select!` depends on: each turn
    /// of the loop awaits one cancel-safe `read` and banks what it got in
    /// [`Session::inbound`] before awaiting again, so a future dropped
    /// mid-frame loses nothing and the next call carries on from the same
    /// byte.
    pub async fn read_frame_into(&mut self, buf: &mut Vec<u8>) -> Result<FrameHeader, NetError> {
        loop {
            if self.inbound.len() >= FRAME_HEADER_LEN {
                let header = FrameHeader::decode_slice(&self.inbound)?;
                if !header.is_control() {
                    let cap = self.caps[usize::from(header.channel)]
                        .ok_or(NetError::UnboundChannel(header.channel))?;
                    header.check_stream_len(cap)?;
                }
                let total = FRAME_HEADER_LEN + header.payload_len();
                if self.inbound.len() >= total {
                    buf.clear();
                    buf.extend_from_slice(&self.inbound[FRAME_HEADER_LEN..total]);
                    self.inbound.drain(..total);
                    return Ok(header);
                }
            }
            let mut chunk = [0_u8; 4096];
            let read = self.read.read(&mut chunk).await?;
            if read == 0 {
                // A disconnect on a frame boundary is the peer leaving; one
                // mid-frame is the peer or the transport breaking — the same
                // distinction the server's reader draws.
                return Err(if self.inbound.is_empty() {
                    NetError::Closed
                } else {
                    NetError::Io(std::io::ErrorKind::UnexpectedEof.into())
                });
            }
            self.inbound.extend_from_slice(&chunk[..read]);
        }
    }

    /// Make one JSON-RPC call and wait for its reply, handing every stream
    /// frame that arrives meanwhile to `on_frame`.
    ///
    /// Notifications (no `id`) are ignored, and a reply for some other id is
    /// skipped rather than treated as an error — both mirror the server's own
    /// reader, and both are what lets a newer peer say things this build does
    /// not understand without costing it the session.
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
            let value: Value = serde_json::from_slice(&buf)
                .map_err(|_| NetError::Malformed("control frame is not JSON"))?;
            if value.get("id").is_none() {
                // A notification. Nothing here consumes one yet; dropped, not
                // fatal.
                continue;
            }
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
