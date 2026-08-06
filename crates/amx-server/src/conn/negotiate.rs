//! Hello/Welcome: capabilities, not equality (04 §4).
//!
//! "Hello/Welcome negotiates capabilities, not equality: client sends
//! `{proto: [min, max], features: [...]}`; server picks the highest common
//! version and echoes the feature intersection."
//!
//! The handshake is two bare objects on the control channel, not JSON-RPC
//! calls: nothing can be dispatched until the version is agreed, so the frames
//! that agree it cannot themselves depend on a dispatch table whose shape is
//! still being negotiated.

use std::collections::BTreeSet;

use amx_proto::frame::CONTROL_CHANNEL;
use amx_proto::{Feature, FrameHeader, Hello, Welcome};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::conn::ConnError;
use crate::conn::reader::Reader;

/// Every feature this server build implements.
///
/// The intersection with the client's set is what
/// [`Hello::accept`](amx_proto::Hello::accept) echoes back. A feature named
/// here that a client never asks for is simply absent from the session, and a
/// feature a client asks for that is absent here is dropped silently — "I do
/// not have that" and "I have never heard of that" must look the same to a
/// peer, or a newer client cannot talk to an older server at all.
#[must_use]
pub fn supported_features() -> BTreeSet<Feature> {
    BTreeSet::from([
        Feature::GRID_STREAM,
        Feature::HISTORY_RANGES,
        Feature::RAW_PANE_IO,
        Feature::LOCAL_KEYBINDINGS,
    ])
}

/// Read the connection's opening frame and decode it as a [`Hello`].
///
/// A first frame that is not a hello ends the connection: there is no protocol
/// state in which anything else is meaningful yet.
pub async fn read_hello<R>(reader: &mut Reader<R>) -> Result<Hello, ConnError>
where
    R: AsyncRead + Unpin,
{
    let frame = reader
        .read_frame()
        .await?
        .ok_or(ConnError::ClosedBeforeHello)?;
    if !frame.header.is_control() {
        return Err(ConnError::Malformed(
            "hello must arrive on the control channel",
        ));
    }
    serde_json::from_slice(frame.payload).map_err(|_| ConnError::NotAHello)
}

/// Write `welcome` as the connection's answering frame.
///
/// Written straight to the socket rather than through the priority queue: the
/// writer does not own the socket until the handshake has completed, and there
/// is by construction nothing to prioritise against a single frame.
pub async fn write_welcome<W>(sink: &mut W, welcome: &Welcome) -> Result<(), ConnError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(welcome).map_err(|_| ConnError::Encode)?;
    let len = u32::try_from(payload.len()).map_err(|_| ConnError::Encode)?;
    sink.write_all(&FrameHeader::new(len, CONTROL_CHANNEL).encode())
        .await?;
    sink.write_all(&payload).await?;
    sink.flush().await?;
    Ok(())
}
