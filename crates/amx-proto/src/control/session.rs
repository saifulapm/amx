//! `session.*` payloads.

use amx_core::{Layout, PaneId, RowId, Seq, SessionId, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::hello::ServerInfo;

/// Parameters of `ping`.
///
/// Empty, and deliberately a struct rather than a unit: adding a field later
/// must not change the wire shape from `null` to an object.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PingParams {}

/// Reply to `ping`.
///
/// Carries the bus sequence, like every state-carrying reply (04 §2), so even
/// a liveness probe tells the caller where the event stream is.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PingReply {
    /// Who answered.
    pub server: ServerInfo,
    /// The session instance that answered.
    pub session: SessionId,
    /// The bus head at reply time.
    pub seq: Seq,
}

/// Parameters of `session.state`.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct StateParams {}

/// One workspace, as `session.state` reports it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// Its label, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The BSP layout tree, verbatim — the client mirrors it as state (04 §3).
    pub layout: Layout,
    /// The pane focused inside this workspace, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<PaneId>,
}

/// One pane, as `session.state` reports it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PaneState {
    /// The pane.
    pub pane: PaneId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// The pane grid's current rows.
    pub rows: u16,
    /// The pane grid's current columns.
    pub cols: u16,
    /// One past the newest row committed to this pane's history.
    pub history_head: RowId,
    /// The oldest history row still fetchable (the eviction floor).
    pub history_floor: RowId,
}

/// Reply to `session.state`: the full snapshot a fresh client folds into its
/// model.
///
/// Carries the bus sequence it was captured at (04 §2): "every state-query
/// response … carries the bus sequence number at which it was captured", so a
/// future event subscription can resume from `seq` without a gap between the
/// snapshot and the stream.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct StateReply {
    /// The bus sequence this snapshot was captured at.
    pub seq: Seq,
    /// The workspace the session currently has focused, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_workspace: Option<WorkspaceId>,
    /// Every workspace, in creation order.
    pub workspaces: Vec<WorkspaceState>,
    /// Every pane, across all workspaces.
    pub panes: Vec<PaneState>,
}
