//! What one decoded key *does*: input consequences (04 §3, §7).
//!
//! [`crate::input`] decides what a byte run means and stops there — the decode
//! is pure, testable without a terminal or a server. This file is the other
//! half: it turns each [`Action`] into the thing that leaves the client, which
//! is bytes for the focused pane's raw stream, a control call from the M0
//! method table, or a purely local focus move.
//!
//! The division is 04 §3's, not a convenience: "the client's layout mirror is
//! server truth", so a navigate verb *calls* the server and repaints when new
//! state arrives. Nothing here rewrites the mirrored layout, and the two places
//! that do move focus locally (`hjkl` and a numeric jump) still echo
//! `pane.focus` so the server's own focus, which is canonical, cannot silently
//! diverge from what this terminal is showing.
//!
//! Split out of [`super`] by V14 (R-M2-5): `app/mod.rs` was at 482 lines of a
//! 500-line soft budget with the `next-attention` verb still to add, and this
//! is the responsibility that comes out whole.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Direction, PaneId, WorkspaceId};
use amx_proto::control::{Call, pane as pane_proto};

use super::App;
use crate::input::{self, Action, InputEvent};

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Turn one decoded [`Action`] into its consequence: bytes to the pane,
    /// a control call, or a local focus change.
    pub(super) fn apply_action(
        &mut self,
        action: Action,
        bytes: &[u8],
        sink: &mut impl FnMut(InputEvent<'_>),
    ) {
        // The three verbs that need no focused pane come first: a client must
        // be able to leave, reach the picker, or jump to whichever agent is
        // waiting, even from an empty session.
        match action {
            Action::Detach => {
                sink(InputEvent::Detach);
                return;
            }
            Action::Picker => {
                self.open_picker();
                return;
            }
            // Focus moves server-side and comes back as `FocusChanged`, which
            // `app::events` folds — the same path every other client's focus
            // change reaches this one by. Nothing is moved locally here.
            Action::NextAttention => {
                sink(InputEvent::Call(Call::AgentNext(
                    // Unscoped: the prefix key cycles the whole queue, and the
                    // workspace-scoped variant D15 asks for is a neighbouring
                    // key X14 binds (X17 reads the scope server-side).
                    amx_proto::control::agent::NextParams { workspace: None },
                )));
                return;
            }
            _ => {}
        }
        let Some(ws) = self.model.focused_workspace_id() else {
            return;
        };
        let Some(pane) = self.focused_pane() else {
            return;
        };
        match action {
            Action::Forward { start, end } => sink(InputEvent::Forward {
                pane,
                bytes: &bytes[start..end],
            }),
            Action::Mouse { start, end } => {
                if self.input.mouse_enabled(pane) {
                    sink(InputEvent::Forward {
                        pane,
                        bytes: &bytes[start..end],
                    });
                }
            }
            Action::CarriedMouse => {
                if self.input.mouse_enabled(pane) {
                    sink(InputEvent::Forward {
                        pane,
                        bytes: self.input.carried(),
                    });
                }
            }
            Action::CarriedBytes => sink(InputEvent::Forward {
                pane,
                bytes: self.input.carried(),
            }),
            Action::Focus(dir) => {
                if let Some(next) = self.neighbour_of(pane, dir) {
                    self.focus.insert(ws, next);
                    self.repaint();
                }
                sink(InputEvent::Call(Call::PaneFocus(pane_proto::FocusParams {
                    workspace: ws,
                    direction: input::wire_direction(dir),
                })));
            }
            // Deliberately no local layout change: the mirror is server truth
            // with no mutable accessor, so the resize round-trips and the
            // repaint follows the updated layout state (04 §3).
            Action::Resize(dir) => sink(InputEvent::Call(Call::PaneResize(
                pane_proto::ResizeParams {
                    pane,
                    direction: input::wire_direction(dir),
                    delta: input::RESIZE_STEP,
                },
            ))),
            Action::Split(direction) => {
                sink(InputEvent::Call(Call::PaneSplit(pane_proto::SplitParams {
                    pane,
                    direction,
                    command: None,
                    cwd: None,
                })))
            }
            Action::Swap(dir) => {
                if let Some(with) = self.neighbour_of(pane, dir) {
                    sink(InputEvent::Call(Call::PaneSwap(pane_proto::SwapParams {
                        pane,
                        with,
                    })));
                }
            }
            // Interim target until T15's picker chooses one (04 §7): the
            // next workspace in id order, wrapping. No other workspace means
            // nowhere to move to.
            Action::MovePane => {
                if let Some(to) = self.next_workspace(ws) {
                    sink(InputEvent::Call(Call::PaneMove(pane_proto::MoveParams {
                        pane,
                        to,
                    })));
                }
            }
            Action::Close => sink(InputEvent::Call(Call::PaneClose(pane_proto::CloseParams {
                pane,
            }))),
            Action::Zoom => sink(InputEvent::Call(Call::PaneZoom(pane_proto::ZoomParams {
                pane,
            }))),
            // Handled above, before the focused-pane requirement.
            Action::Detach | Action::Picker | Action::NextAttention => {}
            // Interim numbering until short numbers reach the client: n-th
            // pane in layout order. Local-only — `pane.focus` speaks
            // directions, and a direct set-focus core op does not exist yet.
            Action::Jump(n) => {
                let target = self
                    .model
                    .focused_workspace()
                    .and_then(|w| w.layout.panes().get(usize::from(n) - 1).copied());
                if let Some(target) = target
                    && target != pane
                {
                    self.focus.insert(ws, target);
                    self.repaint();
                }
            }
        }
    }

    /// The focused workspace's geometric neighbour of `pane`, over the same
    /// content area chrome tiles.
    fn neighbour_of(&self, pane: PaneId, dir: Direction) -> Option<PaneId> {
        let area = self.model.content_area();
        self.model
            .focused_workspace()?
            .layout
            .neighbour(area, pane, dir)
    }

    /// The workspace after `current` in id order, wrapping; `None` if it is
    /// the only one.
    fn next_workspace(&self, current: WorkspaceId) -> Option<WorkspaceId> {
        self.model
            .workspace_ids()
            .filter(|&id| id > current)
            .min()
            .or_else(|| self.model.workspace_ids().filter(|&id| id != current).min())
    }
}
