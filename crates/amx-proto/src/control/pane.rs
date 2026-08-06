//! `pane.*` payloads.

use std::path::PathBuf;

use amx_core::{PaneId, Seq, ShortNumber};
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
