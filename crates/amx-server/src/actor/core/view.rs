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

/// The smallest declared viewport the projection takes at its word.
///
/// Below this nothing tiles — a status line, a border and at least a cell of
/// interior need the room — and such declarations are real: a client on a
/// pty that reports 0×0 (python's `pty.fork` default) attaches and declares
/// exactly that. A dimension under the minimum falls back to the default
/// grid, so panes keep a live projection and render the moment the client
/// learns its real size, instead of the session sitting dark with no
/// indication of why.
const MIN_VIEWPORT_ROWS: u16 = 4;
/// See [`MIN_VIEWPORT_ROWS`].
const MIN_VIEWPORT_COLS: u16 = 4;

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
                // Pass-through, verbatim: see `core/persist.rs`. State and
                // snapshot report the same block, which is what makes the two
                // surfaces one fact rather than two.
                worktree: ws.worktree().cloned(),
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
                    label: self
                        .state
                        .pane(pane)
                        .and_then(|pane| pane.label().map(str::to_owned)),
                    // The same cwd the snapshot writes, from the same place:
                    // stored state, refreshed by the persist capture's
                    // foreground probe. `amx layout export` is what reads it.
                    cwd: self
                        .state
                        .pane(pane)
                        .and_then(|pane| pane.cwd().map(std::path::Path::to_path_buf)),
                    rows,
                    cols,
                    history_head: head,
                    history_floor: floor,
                    // What the pane's parser thread read off its own terminal,
                    // folded here by `Core::handle_pane_report` so this reply
                    // stays synchronous (`docs/notes/m4-mouse-path.md` §3).
                    // Absent means the application asked for nothing, which a
                    // client turns into "do not relay".
                    mouse: self.mouse_of(pane),
                    // From the status summaries `AgentHub` mirrors into `Core`
                    // with `try_send` during normal operation
                    // (`docs/08-m2-plan.md` §3's second read model — the slower
                    // path, whose mailbox lag is harmless because nothing
                    // awaits on it). `None` for a pane with no tracked agent,
                    // which is the honest answer for every pane running a plain
                    // shell.
                    agent: self.agent_status.get(&pane).cloned(),
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
            // The attention queue, in queue order, from the same mirror as the
            // per-pane statuses above. `session.state` *is* the query for the
            // queue (D-M2-8): there is no second method that could answer
            // differently from the status line.
            attention: self.attention.clone(),
            // Counts only; the entries are `session.report`'s. They ride here
            // so an attaching client can render the loss indicator without a
            // second call (04 §6).
            restore: self.restore_summary(),
        }
    }

    pub(super) fn short_of_workspace(&self, ws: amx_core::WorkspaceId) -> ShortNumber {
        self.workspace_shorts.get(&ws).unwrap_or(ShortNumber::FIRST)
    }

    pub(super) fn short_of_pane(&self, pane: PaneId) -> ShortNumber {
        self.pane_shorts.get(&pane).unwrap_or(ShortNumber::FIRST)
    }

    /// Record the active client's declared size and re-project every layout.
    ///
    /// Degenerate dimensions clamp to the default grid rather than being
    /// dropped: dropping would starve every projection for as long as the
    /// client's terminal misreports itself (see [`MIN_VIEWPORT_ROWS`]).
    pub(super) fn handle_viewport(&mut self, params: &client::Viewport) {
        let rows = if params.rows < MIN_VIEWPORT_ROWS {
            DEFAULT_ROWS
        } else {
            params.rows
        };
        let cols = if params.cols < MIN_VIEWPORT_COLS {
            DEFAULT_COLS
        } else {
            params.cols
        };
        self.viewport = Some((rows, cols));
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
            let Some(host) = self.panes.get(&pane) else {
                continue;
            };
            // `try_send` because this runs inside the fold: a pane whose
            // mailbox is full right now will be re-commanded by the next
            // layout batch, and its keyframe carries whatever size it has.
            if host
                .handle()
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
