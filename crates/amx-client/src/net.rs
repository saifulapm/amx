//! The session socket: connect, negotiate, make control calls (04 §4).
//!
//! [`Session`] is the client's half of the same framing and handshake
//! `amx-server`'s `conn` module implements: `[u32 len][u8 channel][payload]`
//! over a `UnixStream`, a `Hello`/`Welcome` exchange, and JSON-RPC calls on
//! the control channel. All three are real against the merged gateway (T10).
//!
//! **Binary streams are not read here.** `amx_proto::stream::grid::
//! GridMessage::{encode,decode}` are still `todo!()` — no per-cell wire
//! layout is defined anywhere in the tree, no golden pins one, and no task in
//! `docs/06-m0-plan.md`'s DAG claims `amx-proto/src/stream/**` as scope (T04
//! owns `frame`/`hello`/`rpc`/`version`/`control`, not `stream`). The control
//! method that would bind a stream to a channel (`reader.rs`'s own comment:
//! "bound by a control call that does not exist yet") is T16 scope and also
//! unmerged. So a stream-channel frame reaching a client's reader is not a
//! condition this build can act on yet: [`Session::call`] treats it as a
//! protocol violation rather than pretending to decode it. Pane grids in this
//! crate are populated by applying already-decoded updates
//! (`amx_client::model::PaneGrid::apply_*`) directly — the seam a real
//! decoder plugs into once the codec lands.

use std::io;
use std::path::Path;

use amx_proto::frame::{CONTROL_CHANNEL, FRAME_HEADER_LEN};
use amx_proto::rpc::{Request, RequestId, Response, RpcOutcome};
use amx_proto::{ClientInfo, Feature, FrameError, FrameHeader, Hello, Resume, Welcome};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

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
    /// A frame arrived on a channel this build does not read from yet.
    #[error("frame on channel {0}, unbound in this build")]
    UnboundChannel(u8),
    /// A frame's payload was not the shape its channel requires.
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
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

/// Read one whole frame from `source`.
pub async fn read_frame<R>(source: &mut R) -> Result<(FrameHeader, Vec<u8>), NetError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; FRAME_HEADER_LEN];
    // A zero-length read of the first byte is the one place a disconnect is
    // not an error — the same distinction `amx-server`'s reader draws between
    // a clean disconnect and a torn frame.
    let read = source.read(&mut header[..1]).await?;
    if read == 0 {
        return Err(NetError::Closed);
    }
    source.read_exact(&mut header[1..]).await?;
    let header = FrameHeader::decode(header)?;
    let mut payload = vec![0_u8; header.payload_len()];
    source.read_exact(&mut payload).await?;
    Ok((header, payload))
}

/// Write one frame to `sink` and flush it.
pub async fn write_frame<W>(sink: &mut W, channel: u8, payload: &[u8]) -> Result<(), NetError>
where
    W: AsyncWrite + Unpin,
{
    let len = u32::try_from(payload.len()).map_err(|_| NetError::Malformed("payload too large"))?;
    sink.write_all(&FrameHeader::new(len, channel).encode())
        .await?;
    sink.write_all(payload).await?;
    sink.flush().await?;
    Ok(())
}

/// A negotiated connection: `Hello`/`Welcome` is done, control calls can be
/// made.
#[derive(Debug)]
pub struct Session {
    stream: UnixStream,
    next_id: u64,
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
    pub async fn attach(
        mut stream: UnixStream,
        client: ClientInfo,
        attach: bool,
        resume: Option<Resume>,
    ) -> Result<(Self, Welcome), NetError> {
        let hello = Hello {
            proto: amx_proto::version::window(),
            features: offered_features(),
            client,
            attach,
            resume,
        };
        let payload =
            serde_json::to_vec(&hello).map_err(|_| NetError::Malformed("encode hello"))?;
        write_frame(&mut stream, CONTROL_CHANNEL, &payload).await?;

        let (header, payload) = read_frame(&mut stream).await?;
        if !header.is_control() {
            return Err(NetError::UnboundChannel(header.channel));
        }
        let welcome: Welcome =
            serde_json::from_slice(&payload).map_err(|_| NetError::Malformed("decode welcome"))?;
        Ok((Self { stream, next_id: 1 }, welcome))
    }

    /// Make one JSON-RPC call and wait for its reply.
    ///
    /// M0 has no server-to-client push (no bus subscription, no bound
    /// streams), so replies arrive strictly in request order and this can
    /// simply read until it sees its own id — a peer sending anything else on
    /// the control channel first would be the skew case JSON-RPC ids exist to
    /// survive, so a mismatched id is skipped rather than treated as an
    /// error.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, NetError> {
        let id = RequestId::Number(self.next_id);
        self.next_id += 1;
        let request = Request::new(id.clone(), method, Some(params));
        let payload =
            serde_json::to_vec(&request).map_err(|_| NetError::Malformed("encode request"))?;
        write_frame(&mut self.stream, CONTROL_CHANNEL, &payload).await?;

        loop {
            let (header, payload) = read_frame(&mut self.stream).await?;
            if !header.is_control() {
                return Err(NetError::UnboundChannel(header.channel));
            }
            let response: Response = serde_json::from_slice(&payload)
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
}
