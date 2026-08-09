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
//!
//! # The single-pane projection (D14, D-M4-7)
//!
//! Mirroring the client's chrome only works while the client is drawing the
//! whole layout. Below `client.narrow_cols` it is not: D14 has a narrow client
//! show one pane full-screen, and a client that declared only its size would
//! then be told to letterbox a pane sized to a slot in a layout it is not
//! drawing — 21 columns inside 45, measured before this rule existed
//! (`docs/notes/m4-live-smoke.md` §1.3).
//!
//! So [`Viewport`] reads the third field of `client.viewport` as well as the
//! first two, which is how `Viewport.panes` finally gets the reader it was
//! frozen with (R-M4-14): **a declaration naming one pane of a layout that
//! holds more sizes that pane to the whole content area**, and resizes no
//! other pane at all.
//!
//! Two halves of the qualifier earn their keep:
//!
//! - *of a layout that holds more*. A client tiling a one-pane workspace also
//!   declares one pane, and it draws that pane's border: its slot's interior is
//!   the right answer and the whole content area would overflow the box it
//!   draws. The rule fires only for a declaration that left panes out, which is
//!   the client saying it is not tiling.
//! - *no other pane at all*. The panes the declaring client left out have no
//!   rect in its projection, so there is no size to give them and they keep the
//!   one the last client that could see them gave them — the rule
//!   [`reconcile_pane_sizes`](Core::reconcile_pane_sizes) already applies to a
//!   pane squeezed out of visible space, applied to a pane deliberately left
//!   out of one. A wide client attached to the same session goes on declaring
//!   the whole layout and takes them back the moment it does.
//!
//! The client's half of the same rule is `amx-client/src/app/narrow.rs`, and
//! the two must agree: the declaration is the only thing that carries it
//! across.

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

/// What the active client declared it is drawing.
///
/// The size authority of 04 §3, plus the one fact D-M4-7 added to it: which
/// pane the declaration named, when it named exactly one. Held verbatim —
/// deciding what a single-pane declaration *means* needs the layout, which
/// moves under it, so that is [`Core::projection`]'s job and not this struct's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Viewport {
    /// The client terminal's rows.
    rows: u16,
    /// The client terminal's columns.
    cols: u16,
    /// The single pane the declaration named, if it named exactly one.
    only: Option<PaneId>,
}

/// How the declared viewport lays panes out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Projection {
    /// Every layout tiled into the content area, each pane at its slot's
    /// interior — what a client drawing chrome declares.
    Tiled(Rect),
    /// One pane filling the content area, and no other pane sized at all
    /// (D14 — see the module header).
    Single(PaneId, Rect),
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
        // The declaration is recorded whole and read later: what one pane means
        // depends on the layout it names, and the layout moves between
        // declarations while this field does not.
        self.viewport = Some(Viewport {
            rows,
            cols,
            only: match params.panes.as_slice() {
                [pane] => Some(*pane),
                _ => None,
            },
        });
        self.reconcile_pane_sizes();
    }

    /// How the declared viewport lays panes out, if any client has declared
    /// one. The module header argues the single-pane rule.
    fn projection(&self) -> Option<Projection> {
        let viewport = self.viewport?;
        let area = content_area(viewport.rows, viewport.cols);
        let Some(only) = viewport.only else {
            return Some(Projection::Tiled(area));
        };
        let partial = self
            .state
            .workspaces()
            .any(|ws| ws.layout().contains(only) && ws.layout().panes().len() > 1);
        if partial {
            Some(Projection::Single(only, area))
        } else {
            // Either the declaration is the whole of a one-pane workspace — a
            // client tiling it, border and all — or it names a pane this
            // session no longer holds, and a stale name must not stop every
            // other pane being sized.
            Some(Projection::Tiled(area))
        }
    }

    /// The cell rect a pane would occupy under the declared viewport, if the
    /// projection gives it any cells at all.
    pub(super) fn planned_size(&self, pane: PaneId) -> Option<(u16, u16)> {
        let area = match self.projection()? {
            // A pane the single-pane projection did not name has no rect in
            // it: a pane split off while a narrow client is attached spawns at
            // the default grid and keeps it until some client draws it.
            Projection::Single(only, area) => {
                if only != pane {
                    return None;
                }
                area
            }
            Projection::Tiled(area) => {
                let mut slot = None;
                for ws in self.state.workspaces() {
                    if !ws.layout().contains(pane) {
                        continue;
                    }
                    slot = ws
                        .layout()
                        .rects(area)
                        .into_iter()
                        .find(|(id, _)| *id == pane)
                        .map(|(_, rect)| inset(rect));
                    break;
                }
                slot?
            }
        };
        (area.h > 0 && area.w > 0).then_some((area.h, area.w))
    }

    /// Resize every pane whose projected cell rect differs from the size it
    /// was last commanded to. Children see the change as `SIGWINCH` via the
    /// pane actor's resize path.
    pub(super) fn reconcile_pane_sizes(&mut self) {
        let Some(projection) = self.projection() else {
            return;
        };
        let mut wanted: Vec<(PaneId, u16, u16)> = Vec::new();
        match projection {
            Projection::Single(pane, area) => {
                if area.h > 0 && area.w > 0 && self.pane_sizes.get(&pane) != Some(&(area.h, area.w))
                {
                    wanted.push((pane, area.h, area.w));
                }
            }
            Projection::Tiled(area) => {
                for ws in self.state.workspaces() {
                    for (pane, rect) in ws.layout().rects(area) {
                        let inner = inset(rect);
                        if inner.h == 0 || inner.w == 0 {
                            // A pane squeezed out of visible space keeps its
                            // last size: a 0x0 PTY starves the process for
                            // nothing.
                            continue;
                        }
                        if self.pane_sizes.get(&pane) != Some(&(inner.h, inner.w)) {
                            wanted.push((pane, inner.h, inner.w));
                        }
                    }
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
