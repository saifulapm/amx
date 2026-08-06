//! The session state tree: workspaces → panes (D13), folded through `Effect`.
//!
//! Every handler here is a pure function over the tree that returns an
//! [`Effect`] describing what it changed — never a `bool` or a mutated flag —
//! so a handler that changes state and forgets to report it is a type error
//! at the call site (04 §2, D2).

use std::collections::BTreeMap;

use crate::effect::Effect;
use crate::id::{PaneId, WorkspaceId};
use crate::layout::{Direction, Layout, LayoutError, Rect};
use crate::state::error::StateError;
use crate::state::pane::Pane;
use crate::state::workspace::Workspace;

/// Where a pane being moved into a workspace lands.
enum Placement {
    /// The destination workspace has no panes: the moved pane becomes root.
    Root,
    /// The destination already has panes: split this one to make room.
    Split(PaneId),
}

/// The whole session: every workspace and every pane in it.
///
/// Panes are also indexed flatly by id so metadata (today: the label) survives
/// independently of which workspace's tree currently holds the pane —
/// [`move_pane`](Self::move_pane) never touches this index, which is how it
/// preserves the pane's UUID and everything else about it.
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    panes: BTreeMap<PaneId, Pane>,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    active_workspace: Option<WorkspaceId>,
}

impl SessionState {
    /// A session with no workspaces and no panes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a workspace by id.
    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    /// Look up a pane by id.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    /// Every workspace in the session, in id order.
    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.values()
    }

    fn workspace_mut(&mut self, id: WorkspaceId) -> Result<&mut Workspace, StateError> {
        self.workspaces
            .get_mut(&id)
            .ok_or(StateError::NoSuchWorkspace(id))
    }

    /// Create a new workspace containing one fresh pane, focused.
    ///
    /// A workspace is never created bare: D13's tree is workspaces → panes,
    /// and an empty workspace has no rect to show, so every workspace starts
    /// with the pane that gave rise to it.
    pub fn open_workspace(&mut self) -> (WorkspaceId, PaneId, Effect) {
        let workspace = WorkspaceId::new_v4();
        let pane = PaneId::new_v4();
        self.panes.insert(pane, Pane::new(pane));
        self.workspaces
            .insert(workspace, Workspace::with_root(workspace, pane));
        (workspace, pane, Effect::Layout)
    }

    /// Split `target`, giving the new pane the slot `dir` names.
    ///
    /// The new pane takes focus, matching the usual "split and land in it"
    /// workflow (04 §7).
    pub fn split(
        &mut self,
        workspace: WorkspaceId,
        target: PaneId,
        dir: Direction,
        ratio: f32,
    ) -> Result<(PaneId, Effect), StateError> {
        let new_pane = PaneId::new_v4();
        let ws = self.workspace_mut(workspace)?;
        ws.layout_mut().split(target, dir, new_pane, ratio)?;
        ws.set_focus(Some(new_pane));
        self.panes.insert(new_pane, Pane::new(new_pane));
        Ok((new_pane, Effect::Layout))
    }

    /// Close `pane`, collapsing its sibling into the parent slot.
    ///
    /// If the closed pane was focused, focus moves to whatever pane the
    /// collapse left in the layout, or to nothing if the workspace is now
    /// empty.
    pub fn close(&mut self, workspace: WorkspaceId, pane: PaneId) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        ws.layout_mut().close(pane)?;
        if ws.focus() == Some(pane) {
            let next = ws.layout().panes().first().copied();
            ws.set_focus(next);
        }
        self.panes.remove(&pane);
        Ok(Effect::Layout)
    }

    /// Exchange the positions of `a` and `b` in `workspace`'s layout.
    ///
    /// Neither pane's identity changes; only which rect each draws into does.
    pub fn swap(
        &mut self,
        workspace: WorkspaceId,
        a: PaneId,
        b: PaneId,
    ) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        ws.layout_mut().swap(a, b)?;
        Ok(Effect::Layout)
    }

    /// Move `pane` from `from` to `to`, splitting `target` in `to` to make
    /// room (or becoming `to`'s root, if `to` currently has no panes).
    ///
    /// `pane`'s id is never regenerated — it is removed from one tree and
    /// inserted, unchanged, into another — so the pane's identity, its
    /// process and its history all survive the move (04 §7: "rearranging
    /// panes never restarts the process in the pane").
    pub fn move_pane(
        &mut self,
        pane: PaneId,
        from: WorkspaceId,
        to: WorkspaceId,
        target: Option<PaneId>,
        dir: Direction,
    ) -> Result<Effect, StateError> {
        if from == to {
            return Ok(Effect::Nothing);
        }

        let src = self
            .workspaces
            .get(&from)
            .ok_or(StateError::NoSuchWorkspace(from))?;
        if !src.layout().contains(pane) {
            return Err(StateError::Layout(LayoutError::NoSuchPane(pane)));
        }
        let dst = self
            .workspaces
            .get(&to)
            .ok_or(StateError::NoSuchWorkspace(to))?;
        let placement = if dst.layout().is_empty() {
            Placement::Root
        } else {
            match target {
                Some(t) if dst.layout().contains(t) => Placement::Split(t),
                _ => return Err(StateError::MoveTargetRequired),
            }
        };

        let src = self
            .workspaces
            .get_mut(&from)
            .ok_or(StateError::NoSuchWorkspace(from))?;
        src.layout_mut().close(pane)?;
        if src.focus() == Some(pane) {
            let next = src.layout().panes().first().copied();
            src.set_focus(next);
        }

        let dst = self
            .workspaces
            .get_mut(&to)
            .ok_or(StateError::NoSuchWorkspace(to))?;
        match placement {
            Placement::Root => *dst.layout_mut() = Layout::with_root(pane),
            Placement::Split(target) => dst.layout_mut().split(target, dir, pane, 0.5)?,
        }
        dst.set_focus(Some(pane));
        Ok(Effect::Layout)
    }

    /// Nudge the split containing `target` by `delta`.
    ///
    /// A no-op — `target` is the workspace's sole pane, with no split to
    /// resize — reports [`Effect::Nothing`] rather than [`Effect::Layout`]:
    /// nothing about the rendered rects changed.
    pub fn resize(
        &mut self,
        workspace: WorkspaceId,
        target: PaneId,
        delta: f32,
    ) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        let changed = ws.layout_mut().resize(target, delta)?;
        Ok(if changed {
            Effect::Layout
        } else {
            Effect::Nothing
        })
    }

    /// Project `pane` to fill `workspace`, without touching the layout tree.
    pub fn zoom(&mut self, workspace: WorkspaceId, pane: PaneId) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        ws.layout_mut().zoom(pane)?;
        Ok(Effect::Layout)
    }

    /// Clear `workspace`'s zoom, restoring the tree's real geometry exactly.
    ///
    /// Reports [`Effect::Nothing`] if nothing was zoomed — the same no-op
    /// discipline as [`resize`](Self::resize).
    pub fn unzoom(&mut self, workspace: WorkspaceId) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        let was_zoomed = ws.layout().zoomed().is_some();
        ws.layout_mut().unzoom();
        Ok(if was_zoomed {
            Effect::Layout
        } else {
            Effect::Nothing
        })
    }

    /// Move `workspace`'s focus to the geometric neighbour in direction
    /// `dir` (`hjkl`), if one exists.
    ///
    /// Finding no neighbour — the focused pane is already at that edge of the
    /// workspace — is a legal no-op: focus does not change, and the returned
    /// effect says so.
    pub fn move_focus(
        &mut self,
        workspace: WorkspaceId,
        dir: Direction,
    ) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        let Some(current) = ws.focus() else {
            return Ok(Effect::Nothing);
        };
        match ws.layout().neighbour(ws.area(), current, dir) {
            Some(next) => {
                ws.set_focus(Some(next));
                Ok(Effect::Layout)
            }
            None => Ok(Effect::Nothing),
        }
    }

    /// Record `workspace`'s current viewport, re-tiling every pane's rect.
    pub fn resize_workspace(
        &mut self,
        workspace: WorkspaceId,
        area: Rect,
    ) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        if ws.area() == area {
            return Ok(Effect::Nothing);
        }
        ws.set_area(area);
        Ok(Effect::Layout)
    }

    /// Rename `pane`.
    pub fn rename_pane(
        &mut self,
        pane: PaneId,
        label: Option<String>,
    ) -> Result<Effect, StateError> {
        let p = self
            .panes
            .get_mut(&pane)
            .ok_or(StateError::NoSuchPane(pane))?;
        if p.label() == label.as_deref() {
            return Ok(Effect::Nothing);
        }
        p.set_label(label);
        Ok(Effect::Layout)
    }

    /// Rename `workspace`.
    pub fn rename_workspace(
        &mut self,
        workspace: WorkspaceId,
        label: Option<String>,
    ) -> Result<Effect, StateError> {
        let ws = self.workspace_mut(workspace)?;
        if ws.label() == label.as_deref() {
            return Ok(Effect::Nothing);
        }
        ws.set_label(label);
        Ok(Effect::Layout)
    }

    /// Record the directory `pane`'s process was started in.
    ///
    /// Pure metadata: it is never consulted by a layout operation and does not
    /// itself invalidate anything on screen.
    pub fn set_pane_cwd(
        &mut self,
        pane: PaneId,
        cwd: std::path::PathBuf,
    ) -> Result<(), StateError> {
        let p = self
            .panes
            .get_mut(&pane)
            .ok_or(StateError::NoSuchPane(pane))?;
        p.set_cwd(cwd);
        Ok(())
    }

    /// Remove `workspace` entirely, taking every pane it holds with it.
    ///
    /// Returns the panes that were in it, so the caller can report — or stop —
    /// what it took with it rather than leaving that to be inferred from
    /// separate per-pane events.
    pub fn kill_workspace(
        &mut self,
        workspace: WorkspaceId,
    ) -> Result<(Vec<PaneId>, Effect), StateError> {
        let ws = self
            .workspaces
            .remove(&workspace)
            .ok_or(StateError::NoSuchWorkspace(workspace))?;
        let panes = ws.layout().panes();
        for pane in &panes {
            self.panes.remove(pane);
        }
        if self.active_workspace == Some(workspace) {
            self.active_workspace = None;
        }
        Ok((panes, Effect::Full))
    }

    /// The workspace currently focused for the session, if one was ever
    /// switched to.
    #[must_use]
    pub const fn active_workspace(&self) -> Option<WorkspaceId> {
        self.active_workspace
    }

    /// Focus `workspace` for the session.
    ///
    /// A no-op — reported as [`Effect::Nothing`] — when it is already the
    /// active workspace.
    pub fn switch_workspace(&mut self, workspace: WorkspaceId) -> Result<Effect, StateError> {
        if !self.workspaces.contains_key(&workspace) {
            return Err(StateError::NoSuchWorkspace(workspace));
        }
        if self.active_workspace == Some(workspace) {
            return Ok(Effect::Nothing);
        }
        self.active_workspace = Some(workspace);
        Ok(Effect::Full)
    }
}
