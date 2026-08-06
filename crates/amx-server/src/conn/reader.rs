//! Frame decode and inbound routing.
//!
//! Two properties this module exists to hold:
//!
//! - **A declared length is never trusted with an allocation.** The 5-byte
//!   header is decoded on its own and
//!   [`FrameHeader::decode`](amx_proto::FrameHeader::decode) rejects an
//!   oversized control frame from it; a stream frame is checked against the cap
//!   negotiated for its channel. Only then is the payload buffer sized. A peer
//!   claiming 4 GiB costs a rejection, not 4 GiB.
//! - **A skew failure is a reply, not a disconnect.** An unknown method or bad
//!   parameters produce a JSON-RPC error and the connection continues; only a
//!   framing or transport violation ends it.

use amx_proto::error::FrameError;
use amx_proto::frame::FRAME_HEADER_LEN;
use amx_proto::rpc::{Notification, Request, RpcError};
use amx_proto::{FrameHeader, Response};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::conn::ConnError;
use crate::conn::writer::{OutFrame, Outbound, OutboundError};
use crate::dispatch::Router;

/// Channels are a single byte, so a connection can bind at most this many.
const CHANNELS: usize = u8::MAX as usize + 1;

/// The frame cap negotiated for each bound inbound stream.
///
/// A channel with no cap is not bound, and a frame arriving on it is a protocol
/// violation rather than a frame with a default size — binding is what tells
/// the reader how big a payload on that channel is allowed to be, so accepting
/// unbound traffic would mean accepting an unbounded length.
#[derive(Debug)]
pub struct StreamCaps {
    caps: Box<[Option<u32>; CHANNELS]>,
}

impl Default for StreamCaps {
    fn default() -> Self {
        Self {
            caps: Box::new([None; CHANNELS]),
        }
    }
}

impl StreamCaps {
    /// Bind `channel` with the frame cap negotiated for it.
    pub fn bind(&mut self, channel: u8, cap: u32) {
        self.caps[channel as usize] = Some(cap);
    }

    /// Unbind `channel`.
    pub fn unbind(&mut self, channel: u8) {
        self.caps[channel as usize] = None;
    }

    /// The cap for `channel`, or `None` if nothing is bound to it.
    #[must_use]
    pub fn cap(&self, channel: u8) -> Option<u32> {
        self.caps[channel as usize]
    }
}

/// One decoded frame, borrowing the reader's payload buffer.
#[derive(Debug)]
pub struct Frame<'a> {
    /// The frame's header.
    pub header: FrameHeader,
    /// The payload, exactly `header.len` bytes.
    pub payload: &'a [u8],
}

/// The read half of a connection.
#[derive(Debug)]
pub struct Reader<R> {
    source: R,
    header: [u8; FRAME_HEADER_LEN],
    payload: Vec<u8>,
    caps: StreamCaps,
}

impl<R> Reader<R>
where
    R: AsyncRead + Unpin,
{
    /// Read frames from `source`.
    #[must_use]
    pub fn new(source: R) -> Self {
        Self {
            source,
            header: [0; FRAME_HEADER_LEN],
            payload: Vec::new(),
            caps: StreamCaps::default(),
        }
    }

    /// The inbound stream bindings, so a control call that binds a stream can
    /// tell the reader what to expect on its channel.
    pub fn caps_mut(&mut self) -> &mut StreamCaps {
        &mut self.caps
    }

    /// Read the next frame.
    ///
    /// `Ok(None)` is a clean disconnect: the peer closed on a frame boundary.
    /// A close *inside* a frame is [`ConnError::Truncated`] — the two are
    /// distinguished because only the first is a normal end of session.
    pub async fn read_frame(&mut self) -> Result<Option<Frame<'_>>, ConnError> {
        // Read the first header byte on its own: a zero-length read here is the
        // only place a disconnect is not an error, and `read_exact` would
        // flatten it into the same `UnexpectedEof` as a torn frame.
        let read = self.source.read(&mut self.header[..1]).await?;
        if read == 0 {
            return Ok(None);
        }
        self.source
            .read_exact(&mut self.header[1..])
            .await
            .map_err(ConnError::truncated)?;

        let header = FrameHeader::decode(self.header)?;
        if !header.is_control() {
            let cap = self
                .caps
                .cap(header.channel)
                .ok_or(FrameError::UnboundChannel {
                    channel: header.channel,
                })?;
            header.check_stream_len(cap)?;
        }

        // Only now, with the length checked against a cap this side chose, is a
        // buffer sized from it. The buffer is reused across frames, so a steady
        // stream of similar frames does not reallocate.
        self.payload.clear();
        self.payload.resize(header.payload_len(), 0);
        self.source
            .read_exact(&mut self.payload)
            .await
            .map_err(ConnError::truncated)?;

        Ok(Some(Frame {
            header,
            payload: &self.payload,
        }))
    }
}

/// Read frames until the peer disconnects, `cancel` fires, or the connection is
/// violated; route each control call and queue its reply.
pub async fn run<R>(
    reader: &mut Reader<R>,
    router: &mut Router,
    out: &Outbound,
    cancel: &CancellationToken,
) -> Result<(), ConnError>
where
    R: AsyncRead + Unpin,
{
    loop {
        let frame = tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            frame = reader.read_frame() => frame?,
        };
        let Some(frame) = frame else { return Ok(()) };

        if !frame.header.is_control() {
            // Inbound binary streams — raw pane I/O in the control direction —
            // are bound by a control call that does not exist yet (T16). Until
            // one does, `StreamCaps` binds nothing and `read_frame` has already
            // refused the channel, so reaching here would mean a binding was
            // added without a consumer.
            return Err(ConnError::Frame(FrameError::UnboundChannel {
                channel: frame.header.channel,
            }));
        }

        let value: Value = serde_json::from_slice(frame.payload)
            .map_err(|_| ConnError::Malformed("control frame is not JSON"))?;
        // JSON-RPC distinguishes a call from a notification by the presence of
        // `id`, not by any tag, so that is what is checked.
        if value.get("id").is_none() {
            let notification: Notification = serde_json::from_value(value).map_err(|_| {
                ConnError::Malformed("control frame is neither call nor notification")
            })?;
            // No client-to-server notification exists in M0's method table. An
            // unknown one is dropped rather than refused: a newer peer must be
            // able to say something this build ignores.
            tracing::debug!(method = %notification.method, "ignoring unknown notification");
            continue;
        }

        let request: Request = serde_json::from_value(value)
            .map_err(|_| ConnError::Malformed("control frame is not a JSON-RPC call"))?;
        let response = if request.is_jsonrpc_2() {
            match crate::dispatch::handle(router, &request.method, request.params).await {
                Ok(result) => Response::ok(request.id, result),
                Err(error) => Response::err(request.id, error),
            }
        } else {
            Response::err(
                request.id,
                RpcError::new(
                    RpcError::INVALID_REQUEST,
                    format!("unsupported jsonrpc version: {}", request.jsonrpc),
                ),
            )
        };
        send_response(out, response)?;
    }
}

/// Queue one response, degrading an unsendable reply into a sendable error.
///
/// A result too large for a control frame must not become a frame the peer is
/// obliged to reject: the call failed to *answer*, and that is what the caller
/// is told.
fn send_response(out: &Outbound, response: Response) -> Result<(), ConnError> {
    let id = response.id.clone();
    let payload = serde_json::to_vec(&response).map_err(|_| ConnError::Encode)?;
    match OutFrame::control(payload) {
        Ok(frame) => Ok(out.send(frame)?),
        Err(OutboundError::TooLarge { len, cap }) => {
            let error = RpcError::new(
                RpcError::INTERNAL_ERROR,
                format!("reply of {len} bytes exceeds the {cap} byte control frame cap"),
            );
            let payload =
                serde_json::to_vec(&Response::err(id, error)).map_err(|_| ConnError::Encode)?;
            Ok(out.send(OutFrame::control(payload)?)?)
        }
        Err(err) => Err(err.into()),
    }
}
