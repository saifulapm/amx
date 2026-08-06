//! `pane.*` payloads.

use std::path::PathBuf;

use amx_core::{PaneId, Seq, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Which way a split cuts the pane it is applied to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    /// Cut left/right: the new pane sits beside the old one.
    Vertical,
    /// Cut top/bottom: the new pane sits below the old one.
    Horizontal,
}

/// Parameters of `pane.split`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SplitParams {
    /// The pane to split.
    pub pane: PaneId,
    /// Which way to cut it.
    pub direction: SplitDirection,
    /// Command to run, or the user's shell when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Working directory override.
    ///
    /// Absent means the default, which is *not* the source pane's own cwd but
    /// its **foreground process** cwd (04 §7: "split and land in the same
    /// directory"), falling back to the pane's cwd when that is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// Reply to `pane.split`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SplitReply {
    /// The new pane.
    pub pane: PaneId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// The bus sequence at which the pane existed.
    pub seq: Seq,
}

/// Parameters of `pane.zoom`.
///
/// Zoom toggles rather than taking a target state: 04 §7's prefix-mode `zoom`
/// is one-shot, and T16's acceptance test
/// `zoom_projects_one_pane_to_the_full_workspace_and_restores_exactly` only
/// makes sense if the second call is what restores it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ZoomParams {
    /// The pane to toggle zoom on.
    pub pane: PaneId,
}

/// Reply to `pane.zoom`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ZoomReply {
    /// Whether the pane is zoomed after this call.
    pub zoomed: bool,
    /// The bus sequence at which the zoom state changed.
    pub seq: Seq,
}

/// Parameters of `pane.swap`.
///
/// Exchanges two panes' positions in the layout tree. Neither pane's identity
/// nor its process is touched (T16's `swap_does_not_restart_either_process`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwapParams {
    /// One pane to swap.
    pub pane: PaneId,
    /// The pane to swap it with.
    pub with: PaneId,
}

/// Reply to `pane.swap`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwapReply {
    /// The bus sequence at which the swap took effect.
    pub seq: Seq,
}

/// Parameters of `pane.move`.
///
/// Moves a pane into a different workspace; 04 §7 has the picker choose the
/// target. The process in the pane is never restarted (T16's
/// `move_pane_between_workspaces_does_not_restart_the_process`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MoveParams {
    /// The pane to move.
    pub pane: PaneId,
    /// The workspace to move it into.
    pub to: WorkspaceId,
}

/// Reply to `pane.move`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MoveReply {
    /// The bus sequence at which the move took effect.
    pub seq: Seq,
}

/// Parameters of `pane.close`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CloseParams {
    /// The pane to close.
    pub pane: PaneId,
}

/// Reply to `pane.close`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CloseReply {
    /// The bus sequence at which the pane was gone.
    pub seq: Seq,
}
