//! A workspace: one BSP layout tree, one focus, one label (D13).

use crate::id::{PaneId, WorkspaceId};
use crate::layout::{Layout, Rect};

/// Viewport a freshly created workspace is assumed to have, until the first
/// real terminal size arrives. 80x24 is the traditional terminal default.
pub const DEFAULT_AREA: Rect = Rect::new(0, 0, 80, 24);

/// One workspace: a BSP layout tree over a set of panes, plus the focus and
/// label that exist per workspace rather than per pane.
///
/// The tree is two levels — workspaces → panes; tabs are deliberately
/// flattened away (D13), so a workspace's own state is exactly what is here.
#[derive(Clone, Debug)]
pub struct Workspace {
    id: WorkspaceId,
    label: Option<String>,
    layout: Layout,
    focus: Option<PaneId>,
    area: Rect,
}

impl Workspace {
    /// A new workspace whose only pane is `pane`, focused by default.
    #[must_use]
    pub(crate) fn with_root(id: WorkspaceId, pane: PaneId) -> Self {
        Self {
            id,
            label: None,
            layout: Layout::with_root(pane),
            focus: Some(pane),
            area: DEFAULT_AREA,
        }
    }

    /// A workspace rebuilt from a snapshot: an existing id over an existing
    /// tree, rather than a fresh one over a fresh pane.
    ///
    /// The caller (`SessionState::adopt_workspace`) has already checked that
    /// `focus`, if set, is a leaf of `layout`; nothing else about a restored
    /// workspace differs from one that was created live, which is the point —
    /// after this it is an ordinary workspace and every handler treats it as
    /// one.
    #[must_use]
    pub(crate) const fn adopted(
        id: WorkspaceId,
        label: Option<String>,
        layout: Layout,
        focus: Option<PaneId>,
    ) -> Self {
        Self {
            id,
            label,
            layout,
            focus,
            area: DEFAULT_AREA,
        }
    }

    /// This workspace's stable identity.
    #[must_use]
    pub const fn id(&self) -> WorkspaceId {
        self.id
    }

    /// The workspace's user-visible label, if one was set.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Set the workspace's label.
    pub(crate) fn set_label(&mut self, label: Option<String>) {
        self.label = label;
    }

    /// The workspace's layout tree.
    #[must_use]
    pub const fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Mutable access to the workspace's layout tree, for the operations in
    /// `state::session` that need to rewrite it.
    pub(crate) fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }

    /// The currently focused pane, or `None` if the workspace has no panes.
    #[must_use]
    pub const fn focus(&self) -> Option<PaneId> {
        self.focus
    }

    /// Set the focused pane directly. Callers are responsible for only
    /// naming a pane actually in this workspace's layout.
    pub(crate) fn set_focus(&mut self, focus: Option<PaneId>) {
        self.focus = focus;
    }

    /// The last known viewport this workspace's panes tile.
    #[must_use]
    pub const fn area(&self) -> Rect {
        self.area
    }

    /// Update the viewport this workspace's panes tile.
    pub(crate) fn set_area(&mut self, area: Rect) {
        self.area = area;
    }
}
