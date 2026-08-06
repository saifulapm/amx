//! `session.state` and `client.viewport`: the snapshot a client folds into
//! its model, and the size authority that drives pane grids.
//!
//! 04 §3: "The pane's PTY grid size follows the most-recently-active client."
//! The client declares its terminal size; the server projects each
//! workspace's layout into that size the same way the client's chrome does —
//! one status line at the bottom, a one-cell border around every pane — and
//! resizes each pane's PTY to its interior. The projection re-runs after any
//! batch that changed a layout, so a split resizes both halves without the
//! client asking again.

use amx_core::{PaneId, Rect, ShortNumber};
use amx_proto::control::{client, session};

use super::Core;
use crate::actor::PaneCommand;

/// The grid size panes report before any client has declared a viewport:
/// the traditional terminal default, matching what they spawn at.
const DEFAULT_ROWS: u16 = 24;
/// See [`DEFAULT_ROWS`].
const DEFAULT_COLS: u16 = 80;

/// The interior a one-cell border leaves inside `rect` — the same inset the
/// client's chrome applies, mirrored here because the server must know each
/// pane's *cell* rect to size its PTY.
const fn inset(rect: Rect) -> Rect {
    if rect.w < 2 || rect.h < 2 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    Rect::new(rect.x + 1, rect.y + 1, rect.w - 2, rect.h - 2)
}

impl Core {
    /// The full session snapshot, captured at the current bus head.
    pub(super) fn session_state(&self) -> session::StateReply {
        let mut workspaces = Vec::new();
        let mut panes = Vec::new();
        for ws in self.state.workspaces() {
            workspaces.push(session::WorkspaceState {
                workspace: ws.id(),
                short: self.short_of_workspace(ws.id()),
                label: ws.label().map(str::to_owned),
                layout: ws.layout().clone(),
                focus: ws.focus(),
            });
            for pane in ws.layout().panes() {
                let (rows, cols) = self
                    .pane_sizes
                    .get(&pane)
                    .copied()
                    .unwrap_or((DEFAULT_ROWS, DEFAULT_COLS));
                let (head, floor) = self
                    .history
                    .get(&pane)
                    .copied()
                    .unwrap_or((amx_core::RowId::from_raw(0), amx_core::RowId::from_raw(0)));
                panes.push(session::PaneState {
                    pane,
                    short: self.short_of_pane(pane),
                    rows,
                    cols,
                    history_head: head,
                    history_floor: floor,
                });
            }
        }
        // Stable order for clients and goldens: shorts are issued in creation
        // order, and the state tree's own map is not.
        workspaces.sort_by_key(|ws| ws.short.get());
        panes.sort_by_key(|pane| pane.short.get());
        session::StateReply {
            seq: self.ctx.bus.head(),
            focused_workspace: self.state.active_workspace(),
            workspaces,
            panes,
        }
    }

    fn short_of_workspace(&self, ws: amx_core::WorkspaceId) -> ShortNumber {
        self.workspace_shorts
            .get(&ws)
            .copied()
            .unwrap_or(ShortNumber::FIRST)
    }

    fn short_of_pane(&self, pane: PaneId) -> ShortNumber {
        self.pane_shorts
            .get(&pane)
            .copied()
            .unwrap_or(ShortNumber::FIRST)
    }

    /// Record the active client's declared size and re-project every layout.
    pub(super) fn handle_viewport(&mut self, params: &client::Viewport) {
        if params.rows == 0 || params.cols == 0 {
            // A zero size starves every projection; nothing sane to do with it.
            return;
        }
        self.viewport = Some((params.rows, params.cols));
        self.reconcile_pane_sizes();
    }

    /// The cell rect a pane would occupy under the declared viewport, if the
    /// projection gives it any cells at all.
    pub(super) fn planned_size(&self, pane: PaneId) -> Option<(u16, u16)> {
        let (rows, cols) = self.viewport?;
        let area = content_area(rows, cols);
        for ws in self.state.workspaces() {
            if !ws.layout().contains(pane) {
                continue;
            }
            for (id, rect) in ws.layout().rects(area) {
                if id == pane {
                    let inner = inset(rect);
                    if inner.h > 0 && inner.w > 0 {
                        return Some((inner.h, inner.w));
                    }
                    return None;
                }
            }
        }
        None
    }

    /// Resize every pane whose projected cell rect differs from the size it
    /// was last commanded to. Children see the change as `SIGWINCH` via the
    /// pane actor's resize path.
    pub(super) fn reconcile_pane_sizes(&mut self) {
        let Some((rows, cols)) = self.viewport else {
            return;
        };
        let area = content_area(rows, cols);
        let mut wanted: Vec<(PaneId, u16, u16)> = Vec::new();
        for ws in self.state.workspaces() {
            for (pane, rect) in ws.layout().rects(area) {
                let inner = inset(rect);
                if inner.h == 0 || inner.w == 0 {
                    // A pane squeezed out of visible space keeps its last
                    // size: a 0x0 PTY starves the process for nothing.
                    continue;
                }
                if self.pane_sizes.get(&pane) != Some(&(inner.h, inner.w)) {
                    wanted.push((pane, inner.h, inner.w));
                }
            }
        }
        for (pane, new_rows, new_cols) in wanted {
            let Some(wiring) = self.panes.get(&pane) else {
                continue;
            };
            // `try_send` because this runs inside the fold: a pane whose
            // mailbox is full right now will be re-commanded by the next
            // layout batch, and its keyframe carries whatever size it has.
            if wiring
                .handle
                .try_send(PaneCommand::Resize {
                    rows: new_rows,
                    cols: new_cols,
                })
                .is_ok()
            {
                self.pane_sizes.insert(pane, (new_rows, new_cols));
            }
        }
    }
}

/// The area panes tile under a viewport: everything above the status line.
const fn content_area(rows: u16, cols: u16) -> Rect {
    Rect::new(0, 0, cols, rows.saturating_sub(1))
}
