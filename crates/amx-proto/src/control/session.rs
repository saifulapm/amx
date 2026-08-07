//! `session.*` payloads.

use std::path::PathBuf;

use amx_core::agent::AgentSnapshot;
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
    /// Its label, if one was set.
    ///
    /// Additive and optional, like the workspace label beside it: absent on
    /// the wire when unset, so a peer built before `pane.rename` existed reads
    /// exactly the bytes it always did (R-M1-8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The pane grid's current rows.
    pub rows: u16,
    /// The pane grid's current columns.
    pub cols: u16,
    /// One past the newest row committed to this pane's history.
    pub history_head: RowId,
    /// The oldest history row still fetchable (the eviction floor).
    pub history_floor: RowId,
    /// What `AgentHub` makes of the program in this pane, when it tracks one.
    ///
    /// Additive and optional, like the label above it: absent on the wire when
    /// the pane has no tracked agent, so a peer built before M2 reads exactly
    /// the bytes it always did. The R-M1-8 precedent — additive optional fields
    /// under the unknown-field contract stay inside protocol v1.
    ///
    /// The same value `AgentHub`'s `StatusView` holds, not a projection of it:
    /// a client rendering one shape while a wait evaluates another is how the
    /// status line and `amx wait` come to disagree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
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
    /// The attention queue, in queue order: agents waiting on the user.
    ///
    /// The *same* queue the status line renders `⚑N` from, `agent.next`
    /// focuses the head of, and the reference notifier consumes — D-M2-8 makes
    /// `session.state` the query, so there is no second "read the queue"
    /// method that could answer differently. Ordered by block time; a pane that
    /// unblocked and blocked again is at the tail.
    ///
    /// Additive: empty on a session with no blocked agents, and absent from the
    /// bytes a pre-M2 peer parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<PaneId>,
    /// What this server's startup restore cost, if it performed one.
    ///
    /// Counts only: the entries are served whole by `session.report`. They
    /// ride here so an attaching client can render the status-line indicator
    /// without a second call — the indicator is the reason 04 §6 says restore
    /// loss is "never log-only". Absent when the server started fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreSummary>,
}

/// Parameters of `session.report`.
///
/// Empty, and a struct rather than a unit for the same reason [`PingParams`]
/// is: a field added later must not change the wire shape.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ReportParams {}

/// Reply to `session.report`: the restore report, entry by entry.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReportReply {
    /// The bus sequence this report was read at.
    pub seq: Seq,
    /// The report itself, empty when the restore lost nothing.
    pub report: RestoreReport,
}

/// What a restore cost, in counts.
///
/// The summary of a [`RestoreReport`], and derivable from it — but carried
/// separately on `session.state` so the common case (render an indicator)
/// never fetches the entries.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RestoreSummary {
    /// Workspaces and panes that came back.
    pub restored: u32,
    /// Entries that were pruned outright.
    pub lost: u32,
    /// Entries that came back with something missing.
    pub degraded: u32,
}

impl RestoreSummary {
    /// Whether anything at all went wrong.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.lost == 0 && self.degraded == 0
    }
}

/// Everything a restore could not do faithfully.
///
/// 04 §6: restore loss is "shown in the status line and queryable via `amx
/// session report` — never log-only", which is why this is a reply payload and
/// not a `warn!`. An empty report is the successful case, not an absent one.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RestoreReport {
    /// Every entry, in the order restore produced them.
    #[serde(default)]
    pub entries: Vec<RestoreLoss>,
}

impl RestoreReport {
    /// Count this report's entries by severity.
    ///
    /// `restored` is not derivable from the entries — restore knows it and the
    /// report does not — so the caller supplies it.
    #[must_use]
    pub fn summary(&self, restored: u32) -> RestoreSummary {
        let count = |severity| {
            u32::try_from(
                self.entries
                    .iter()
                    .filter(|entry| entry.severity == severity)
                    .count(),
            )
            .unwrap_or(u32::MAX)
        };
        RestoreSummary {
            restored,
            lost: count(RestoreSeverity::Lost),
            degraded: count(RestoreSeverity::Degraded),
        }
    }
}

/// How badly one entry fared.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreSeverity {
    /// It was pruned: a failed spawn, or a workspace left with no panes.
    Lost,
    /// It came back with something missing: a vanished cwd, an unreadable
    /// scrollback sidecar.
    Degraded,
}

/// What a restore entry is about.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreEntity {
    /// The snapshot as a whole — unreadable, or from a newer amx.
    Session,
    /// One workspace.
    Workspace,
    /// One pane.
    Pane,
}

/// One thing a restore lost or degraded.
///
/// Named for the common case; a `Degraded` entry is a partial loss and lands
/// here too, because the surfaces that consume this — the status line, `amx
/// session report` — want one list, ordered as restore produced it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RestoreLoss {
    /// How badly this one fared.
    pub severity: RestoreSeverity,
    /// What kind of thing it is.
    pub entity: RestoreEntity,
    /// The workspace, when the entry names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// The pane, when the entry names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneId>,
    /// The label the entity had in the snapshot, if it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The path the entry is about: a pane's saved cwd, a sidecar file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Why, in one line, for a human reading `amx session report`.
    pub reason: String,
}
