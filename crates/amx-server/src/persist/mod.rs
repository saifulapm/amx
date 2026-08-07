//! The on-disk state format: the snapshot schema and the sidecar header.
//!
//! 04 §6: "One snapshot file (`session.json`): layout tree, per-pane
//! cwd/label/agent identity, keyed by UUID. Written atomic tmp+rename + fsync
//! of file and directory. Version field with an N/N−1 read window." This module
//! is the *shape* of that file and of the per-pane scrollback sidecars beside
//! it — pure types and pure validation, no I/O.
//!
//! ```text
//! $XDG_STATE_HOME/amx/<session>/
//!   session.json            the snapshot
//!   history/
//!     <pane-uuid>.rows      opt-in scrollback sidecar, one per pane
//! ```
//!
//! Keyed by UUID, flat, exactly like `session.state` on the wire: herdr zipped
//! workspaces and tabs positionally and desynced when the lists disagreed, and
//! a flat table keyed by identity makes that failure mode unrepresentable.
//! [`Layout`] is stored verbatim rather than projected, so the snapshot and the
//! wire share one algebra.
//!
//! # Task ownership
//!
//! U01 froze the types here; **U02** adds [`io`], [`snapshot`] and [`sidecar`]
//! beside them (`docs/07-m1-plan.md` §4) — the atomic writer, the reader that
//! applies [`READ_WINDOW`], and the sidecar codec that packs rows through the
//! wire's own `put_row`.

pub mod io;
pub mod sidecar;
pub mod snapshot;

use std::path::PathBuf;

use amx_core::{Layout, PaneId, RowRange, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The snapshot file's name inside a session's state directory.
pub const SNAPSHOT_NAME: &str = "session.json";

/// The directory scrollback sidecars live in, inside the state directory.
pub const HISTORY_DIR: &str = "history";

/// The extension a scrollback sidecar carries.
///
/// `.rows`, not the `.ansi` 04 §6 writes: the file holds packed *text* rows in
/// the wire's own layout, not an ANSI replay stream, and history carries no
/// styling today (`docs/notes/scrollback-identity.md`). Recorded as R-M1-1 —
/// 04 §6 wants the one-word correction, the mechanism is as designed.
pub const SIDECAR_EXTENSION: &str = "rows";

/// The schema version this build writes.
pub const VERSION: u32 = 1;

/// The versions this build reads.
///
/// The N/N−1 window of 04 §6, as a table: supporting a new version is adding a
/// row here and nothing else, the same shape `tests/skew.rs` gives the protocol
/// window. At v1 there is no predecessor, so the window is just `{1}`.
pub const READ_WINDOW: &[u32] = &[VERSION];

/// A snapshot could not be read.
///
/// Distinct variants because restore turns each into a different report entry
/// (D-M1-9): a snapshot from the future is a loss the user must be told about,
/// not a warning to log and move past.
#[derive(Debug, Error)]
pub enum PersistError {
    /// The snapshot names a version outside [`READ_WINDOW`].
    #[error("snapshot from a newer amx: version {found}, this build reads {READ_WINDOW:?}")]
    NewerVersion {
        /// The version the file declared.
        found: u32,
    },
    /// The snapshot is not the shape this build expects.
    #[error("unreadable snapshot: {0}")]
    Malformed(String),
    /// A sidecar's header is not a sidecar header.
    #[error("unreadable scrollback sidecar: {0}")]
    BadSidecar(String),
    /// The file could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// One session, as it survives a restart.
///
/// Deliberately *not* a mirror of everything the server holds: no argv (M1
/// restore respawns shells in saved cwds, and argv-as-data is M2), and nothing
/// client-side — presentation state lives in the client (04 §6), so the schema
/// has nowhere to put it.
///
/// Unknown fields are ignored, matching the wire's tolerance rule in both
/// directions: a v2 writer can add fields without stranding a v1 reader inside
/// the window.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// The schema version these bytes were written at.
    pub version: u32,
    /// The workspace that was focused, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_workspace: Option<WorkspaceId>,
    /// Every workspace, in short-number order.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSnapshot>,
    /// Every pane, across all workspaces, in short-number order.
    #[serde(default)]
    pub panes: Vec<PaneSnapshot>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::empty()
    }
}

impl Snapshot {
    /// A snapshot of nothing, at the current version.
    ///
    /// What a session with no workspaces captures, and what a restore that
    /// finds no file behaves as if it read.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            version: VERSION,
            focused_workspace: None,
            workspaces: Vec::new(),
            panes: Vec::new(),
        }
    }

    /// Whether there is anything here to restore.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.workspaces.is_empty() && self.panes.is_empty()
    }

    /// Check this snapshot's version against [`READ_WINDOW`].
    ///
    /// # Errors
    ///
    /// [`PersistError::NewerVersion`] for a version above the window — the
    /// case restore reports as a whole-session loss rather than guessing at
    /// bytes it does not understand. A version *below* the window is the same
    /// refusal: this build cannot honestly read it either.
    pub const fn check_version(&self) -> Result<(), PersistError> {
        let mut i = 0;
        while i < READ_WINDOW.len() {
            if READ_WINDOW[i] == self.version {
                return Ok(());
            }
            i += 1;
        }
        Err(PersistError::NewerVersion {
            found: self.version,
        })
    }
}

/// One workspace, as the snapshot holds it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Its identity — the same UUID after a restart, which is what makes
    /// sidecars and layouts join back up without positional guessing.
    pub id: WorkspaceId,
    /// The short number it was issued.
    ///
    /// Persisted because 04 §6 requires short numbers "stable across
    /// restarts". They are a display mapping, never something a client sends
    /// back, so restoring them is restoring what the user typed yesterday.
    pub short: ShortNumber,
    /// Its label, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The BSP layout tree, verbatim.
    pub layout: Layout,
    /// The pane focused inside it, if it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<PaneId>,
}

/// One pane, as the snapshot holds it.
///
/// No argv: M1 restore respawns a shell in the saved directory (05, M1). The
/// version window is what makes adding argv later a field addition rather than
/// a schema break.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PaneSnapshot {
    /// Its identity.
    pub id: PaneId,
    /// The short number it was issued. See [`WorkspaceSnapshot::short`].
    pub short: ShortNumber,
    /// Its label, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The directory to respawn its shell in.
    ///
    /// As fresh as the last save: amx has no OSC 7 plumbing, so capture
    /// refreshes this through the pane's foreground-cwd probe and the shutdown
    /// push uses the last known value (R-M1-7). A directory that has since
    /// vanished degrades the pane to `$HOME` and is reported, never silently
    /// substituted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// The magic four bytes every scrollback sidecar opens with.
pub const SIDECAR_MAGIC: [u8; 4] = *b"AMXR";

/// The sidecar format version this build writes.
///
/// Separate from [`VERSION`]: the snapshot is JSON and the sidecar is packed
/// binary, and they will move independently — the sidecar bumps when history
/// gains the packed `Cells` layout and rows can finally carry styling
/// (D-M1-6), which has nothing to do with the snapshot's shape.
pub const SIDECAR_VERSION: u16 = 1;

/// A scrollback sidecar's fixed-size header.
///
/// The pane's UUID is repeated inside the file even though the file name
/// carries it, so a sidecar that was renamed, copied or restored under the
/// wrong name is detectable rather than replayed into the wrong pane. The
/// [`RowRange`] records which rows the body holds; the ids themselves are *not*
/// preserved across a restart — replayed lines re-wrap at the current width and
/// take fresh ids through the normal tracker, which the eviction-floor contract
/// makes safe (D-M1-6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SidecarHeader {
    /// The format version the body is packed at.
    pub version: u16,
    /// The pane these rows came from.
    pub pane: PaneId,
    /// The rows the body holds, by the id they had when it was written.
    pub range: RowRange,
}

impl SidecarHeader {
    /// Encoded length: magic, version, pane UUID, and the range's two ids.
    pub const ENCODED_LEN: usize = SIDECAR_MAGIC.len() + 2 + 16 + 8 + 8;

    /// A header for `range` of `pane`, at the current format version.
    #[must_use]
    pub const fn new(pane: PaneId, range: RowRange) -> Self {
        Self {
            version: SIDECAR_VERSION,
            pane,
            range,
        }
    }
}

/// The file name a pane's sidecar takes inside [`HISTORY_DIR`].
///
/// Keyed by UUID, one file per pane, so turning history off can wipe them and
/// a single unreadable file costs exactly one pane's scrollback (04 §6).
#[must_use]
pub fn sidecar_name(pane: PaneId) -> String {
    format!("{pane}.{SIDECAR_EXTENSION}")
}
