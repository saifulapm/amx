//! The history stream: chunked bulk transfer of scrollback ranges.

use amx_core::RowRange;

use crate::error::FrameError;
use crate::rpc::RequestId;

/// One chunk of a history range transfer.
///
/// Chunked because a scrollback fetch is unbounded in size and the writer's
/// priority classes only help if bulk traffic can be interleaved: a chunk is a
/// yield point at which a control reply can overtake (04 §4).
///
/// The `request` id ties every chunk back to the call that asked for the range,
/// so two overlapping fetches on one stream stay distinguishable.
/// The rows themselves stay borrowed — they are read straight out of the
/// pane's history into a reused buffer — while the request id is owned, since
/// it is one small value per chunk rather than one per row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistoryChunk<'a> {
    /// The request this chunk answers.
    pub request: RequestId,
    /// The rows in *this chunk*, not in the whole requested range.
    pub range: RowRange,
    /// Whether more chunks follow for the same request.
    pub more: bool,
    /// Packed rows, in the same layout the grid stream packs cells.
    pub rows: &'a [u8],
}

impl HistoryChunk<'_> {
    /// Append the little-endian encoding of this chunk to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let _ = out;
        todo!("write the hand-rolled little-endian history chunk encoding")
    }
}

impl<'a> HistoryChunk<'a> {
    /// Decode a chunk that borrows from `bytes`.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, FrameError> {
        let _ = bytes;
        todo!("read the hand-rolled little-endian history chunk encoding")
    }
}
