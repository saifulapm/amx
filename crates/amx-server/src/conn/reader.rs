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
//! - **A long poll never stops the reading.** `wait` and `pane.wait_output` run
//!   until a condition holds, and their parameters document the other thing
//!   that ends them: "until the connection dies". A reader that awaited one in
//!   place could not observe the peer's disconnect while it waited, so the two
//!   go onto their own tasks and their replies are queued by id like any other.
//!   JSON-RPC correlates by id, so replies overtaking each other is ordinary.

use std::sync::{Arc, Mutex, PoisonError};

use amx_proto::error::FrameError;
use amx_proto::frame::FRAME_HEADER_LEN;
use amx_proto::rpc::{Notification, Request, RpcError};
use amx_proto::stream::{FlowControl, RawDirection, RawPaneIo};
use amx_proto::{FrameHeader, Response};
use bytes::Bytes;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::actor::PaneCommand;
use crate::conn::ConnError;
use crate::conn::events::{ConnEvents, is_long_poll};
use crate::conn::streams::ConnStreams;
use crate::conn::writer::{OutFrame, Outbound, OutboundError};
use crate::dispatch::Router;

/// The notification a client signals stream flow control with.
///
/// A notification rather than a call because flow control wants no reply —
/// pausing a stream must not itself queue more control traffic.
pub const FLOW_METHOD: &str = "stream.flow";

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
    /// Shared with the connection's stream bindings: `stream.bind` writes a
    /// channel's cap there and this reader starts admitting frames on it.
    caps: Arc<Mutex<StreamCaps>>,
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
            caps: Arc::new(Mutex::new(StreamCaps::default())),
        }
    }

    /// Adopt the connection's shared cap table, so binds made through
    /// dispatch reach this reader.
    pub fn share_caps(&mut self, caps: Arc<Mutex<StreamCaps>>) {
        self.caps = caps;
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
            // A plain-array read; a panic cannot happen inside the lock.
            let cap = self
                .caps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
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
    streams: &ConnStreams,
    events: &ConnEvents,
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
            // `read_frame` admitted the frame, so its channel is bound: raw
            // pane input is the one inbound stream kind, and its bytes go to
            // the pane's own mailbox — never through the `Core` — so a
            // keystroke pays one hand-off, not two (04 §4's latency budget).
            let Some(route) = streams.raw_route(frame.header.channel) else {
                return Err(ConnError::Frame(FrameError::UnboundChannel {
                    channel: frame.header.channel,
                }));
            };
            let message = RawPaneIo::decode(frame.payload)
                .map_err(|_| ConnError::Malformed("raw i/o frame does not decode"))?;
            if message.pane != route.pane {
                return Err(ConnError::Malformed(
                    "raw i/o frame names a pane its channel is not bound to",
                ));
            }
            if message.direction != RawDirection::ToPane {
                return Err(ConnError::Malformed(
                    "only to-pane raw i/o may arrive from a client",
                ));
            }
            let bytes = Bytes::copy_from_slice(message.bytes);
            // Await: the pane's bounded mailbox is the backpressure that keeps
            // a client flooding input from growing an unbounded queue.
            if route.handle.send(PaneCommand::Write(bytes)).await.is_err() {
                tracing::debug!(pane = %route.pane, "input for a pane that is gone");
            }
            continue;
        }

        let value: Value = serde_json::from_slice(frame.payload)
            .map_err(|_| ConnError::Malformed("control frame is not JSON"))?;
        // JSON-RPC distinguishes a call from a notification by the presence of
        // `id`, not by any tag, so that is what is checked.
        if value.get("id").is_none() {
            let notification: Notification = serde_json::from_value(value).map_err(|_| {
                ConnError::Malformed("control frame is neither call nor notification")
            })?;
            if notification.method == FLOW_METHOD {
                if let Some(flow) = notification
                    .params
                    .and_then(|params| serde_json::from_value::<FlowControl>(params).ok())
                {
                    streams.control(flow).await;
                }
                continue;
            }
            // Any other notification is dropped rather than refused: a newer
            // peer must be able to say something this build ignores.
            tracing::debug!(method = %notification.method, "ignoring unknown notification");
            continue;
        }

        let request: Request = serde_json::from_value(value)
            .map_err(|_| ConnError::Malformed("control frame is not a JSON-RPC call"))?;
        if request.is_jsonrpc_2() && is_long_poll(&request.method) {
            // A clone of the router, not a borrow: the poll outlives this
            // iteration by design, and a `Router` is two handles wide.
            let mut router = router.clone();
            let out = out.clone();
            events.spawn_wait(async move {
                let response =
                    match crate::dispatch::handle(&mut router, &request.method, request.params)
                        .await
                    {
                        Ok(result) => Response::ok(request.id, result),
                        Err(error) => Response::err(request.id, error),
                    };
                // A queue failure here means the writer has already stopped,
                // which is the connection ending: nothing to report to.
                let _ = send_response(&out, response);
            });
            continue;
        }
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
