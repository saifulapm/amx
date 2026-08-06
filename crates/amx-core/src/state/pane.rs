//! A pane: one leaf of a workspace's layout tree.

use std::path::{Path, PathBuf};

use crate::id::PaneId;

/// A pane's session-state metadata.
///
/// The pane's UUID is its identity for the pane's whole lifetime (`id.rs`);
/// everything here is display sugar layered on top and never consulted by
/// layout operations, which is what lets splits, closes, swaps and moves
/// leave it untouched.
#[derive(Clone, Debug)]
pub struct Pane {
    id: PaneId,
    label: Option<String>,
    cwd: Option<PathBuf>,
}

impl Pane {
    /// A freshly minted pane with no label and no recorded cwd.
    #[must_use]
    pub(crate) fn new(id: PaneId) -> Self {
        Self {
            id,
            label: None,
            cwd: None,
        }
    }

    /// This pane's stable identity.
    #[must_use]
    pub const fn id(&self) -> PaneId {
        self.id
    }

    /// The pane's user-visible label, if one was set.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Set the pane's label.
    pub(crate) fn set_label(&mut self, label: Option<String>) {
        self.label = label;
    }

    /// The directory the pane's process was started in.
    ///
    /// For a split this is the *foreground process* cwd of the source pane,
    /// not the source pane's own directory, with a defined fallback when that
    /// is unreadable (04 §7) — `foreground_cwd` as pane API state.
    #[must_use]
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Record the directory the pane's process was started in.
    pub(crate) fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = Some(cwd);
    }
}
