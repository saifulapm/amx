//! `stream.bind` and `pane.history`: the handlers that touch connection state.
//!
//! Both need what no other handler does — this connection's bindings — so the
//! `Core`'s part is deliberately small: it resolves a pane to its live
//! plumbing ([`StreamCall::Wiring`]) and everything else happens here, on the
//! connection that asked.

use amx_core::{RowId, RowRange};
use amx_proto::control::stream::{BindParams, BindReply, HistoryParams, HistoryReply};
use amx_proto::rpc::{RequestId, RpcError};
use amx_proto::stream::{HistoryChunk, Priority};
use bytes::Bytes;
use tokio::sync::oneshot;

use super::Router;
use crate::actor::{CoreCommand, PaneCommand, StreamCall};
use crate::conn::streams::ConnStreams;
use crate::conn::writer::OutFrame;

/// Rows per history chunk: the same yield-point granularity the parser-side
/// chunking uses ([`crate::history::CHUNK_ROWS`]).
const CHUNK_ROWS: usize = 256;

/// The frame cap history chunks are written against.
const CHUNK_CAP: u32 = amx_proto::frame::DEFAULT_STREAM_FRAME;

/// Bind one stream for this connection.
///
/// The whole call is a resolve and a hand-off, including the additive
/// `generation` a reconnecting client sends (`docs/09-m3-plan.md` D-M3-7):
/// deciding what a re-bound grid opens with needs the pane's live generation,
/// which arrives with the wiring, and this connection's resume block, which
/// only [`ConnStreams`] holds. So the parameters travel whole rather than
/// being unpacked here.
pub async fn bind(router: &mut Router, params: BindParams) -> Result<BindReply, RpcError> {
    let streams = conn_streams(router)?.clone();
    let pane = params.kind.pane();
    let wiring = router
        .call(|reply| CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
        .await?;
    streams.bind(params, &wiring)
}

/// Serve one history range as chunks on the bound history stream.
pub async fn history(router: &mut Router, params: HistoryParams) -> Result<HistoryReply, RpcError> {
    let streams = conn_streams(router)?.clone();
    let Some(route) = streams.history_route(params.pane) else {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            "no history stream bound for the pane; call stream.bind first",
        ));
    };

    let (tx, rx) = oneshot::channel();
    let range = RowRange::new(params.first, params.last);
    route
        .handle
        .send(PaneCommand::HistoryRange { range, reply: tx })
        .await
        .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "the pane is gone"))?;
    let rows = rx
        .await
        .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "the pane dropped the read"))?
        .map_err(|err| RpcError::new(RpcError::INVALID_PARAMS, err.to_string()))?;

    // Split the packed buffer into chunks on row boundaries. Chunks are yield
    // points: each is queued at bulk priority, waiting for queue room, so a
    // big fetch throttles itself instead of bloating the writer (04 §4).
    let bytes = &rows.rows;
    let mut offsets = Vec::new();
    let mut at = 0_usize;
    let torn = || RpcError::new(RpcError::INTERNAL_ERROR, "the pane served torn rows");
    while at < bytes.len() {
        // Row layout: u32 length, text, one flag byte.
        let len_bytes: [u8; 4] = bytes
            .get(at..at + 4)
            .ok_or_else(torn)?
            .try_into()
            .map_err(|_| torn())?;
        let end = at + 4 + u32::from_le_bytes(len_bytes) as usize + 1;
        if end > bytes.len() {
            return Err(torn());
        }
        offsets.push((at, end));
        at = end;
    }

    let mut chunks = 0_u32;
    let total = offsets.len();
    let mut payload = Vec::new();
    for (index, group) in offsets.chunks(CHUNK_ROWS).enumerate() {
        let first_row = rows.range.first.get() + (index * CHUNK_ROWS) as u64;
        let last_row = first_row + group.len() as u64 - 1;
        let start = group[0].0;
        let end = group[group.len() - 1].1;
        let more = (index + 1) * CHUNK_ROWS < total;
        payload.clear();
        HistoryChunk {
            request: RequestId::Number(params.request),
            range: RowRange::new(RowId::from_raw(first_row), RowId::from_raw(last_row)),
            more,
            rows: &bytes[start..end],
        }
        .encode(&mut payload);
        let frame = OutFrame::stream(
            route.channel,
            Priority::Bulk,
            CHUNK_CAP,
            Bytes::copy_from_slice(&payload),
        )
        .map_err(|err| RpcError::new(RpcError::INTERNAL_ERROR, err.to_string()))?;
        streams
            .outbound()
            .send_when_ready(frame)
            .await
            .map_err(|err| RpcError::new(RpcError::INTERNAL_ERROR, err.to_string()))?;
        chunks += 1;
    }

    Ok(HistoryReply {
        chunks,
        seq: router.head().await?,
    })
}

fn conn_streams(router: &Router) -> Result<&ConnStreams, RpcError> {
    router.streams().ok_or_else(|| {
        RpcError::new(
            RpcError::INTERNAL_ERROR,
            "stream binding needs a client connection",
        )
    })
}
