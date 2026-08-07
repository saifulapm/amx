//! `stream.*` and `pane.history` payloads: binding binary channels and
//! driving the history transfer that rides one.
//!
//! 04 §4: a binary stream is "bound by a control call" and then speaks only
//! its own kind. Binding is per-connection — the reply's channel byte means
//! something only on the connection that made the call.

use amx_core::{GridGeneration, PaneId, RowId, Seq};
use serde::{Deserialize, Serialize};

use crate::stream::{StreamId, StreamKind};

/// Parameters of `stream.bind`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BindParams {
    /// What the stream will carry, and for which pane.
    #[serde(flatten)]
    pub kind: StreamKind,
    /// The grid generation this client already holds for the pane.
    ///
    /// The reconnect half of 04 §6's "keyframes for stale grids", additive on
    /// the R-M1-8 terms every optional field here rides (D-M3-7): a fresh bind
    /// omits it and opens exactly as it always did, a re-bind after a server
    /// swap sends what it last saw, and the server opens delta-only when the
    /// generations agree or with a `KeyframeReason::Generation` keyframe when
    /// they do not.
    ///
    /// Meaningless on a non-grid stream, and ignored there rather than refused
    /// — the unknown-field contract read one level in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<GridGeneration>,
}

/// Reply to `stream.bind`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BindReply {
    /// The bound stream's id.
    pub stream: StreamId,
    /// The frame channel byte carrying it on this connection.
    pub channel: u8,
    /// The frame cap that applies to it, in bytes, both directions.
    pub max_frame: u32,
}

/// Parameters of `pane.history`.
///
/// The chunks answering this call arrive on the connection's bound history
/// stream for `pane` — binding one first is what tells the server (and the
/// client's own reader) which channel the bulk bytes belong to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HistoryParams {
    /// The pane whose history is wanted.
    pub pane: PaneId,
    /// The oldest row wanted.
    pub first: RowId,
    /// The newest row wanted (inclusive).
    pub last: RowId,
    /// Caller-chosen token carried on every chunk, so overlapping fetches on
    /// one stream stay distinguishable.
    pub request: u64,
}

/// Reply to `pane.history`.
///
/// Sent after the chunks are queued; the writer's strict priority means the
/// reply may still overtake them on the wire, so the caller matches chunks by
/// their `request` token, not by arrival order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HistoryReply {
    /// How many chunks were queued for the request.
    pub chunks: u32,
    /// The bus sequence at reply time.
    pub seq: Seq,
}
