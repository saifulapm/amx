//! The two surfaces drawn over the panes: the picker, and copy mode.
//!
//! The picker is the one choose-one primitive (04 §7): its sources — the
//! model's workspaces, the focused workspace's panes, a few built-in commands
//! — flatten to labels on the way in and map the chosen index back on the way
//! out, so every domain rides one code path. Copy mode is `crate::copy`'s
//! engine given a viewport: the focused pane's rect, rows served from the
//! scrollback cache in stable-row coordinates, misses queued for the wired
//! loop to fetch over the history stream.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Effect, PaneId, RowId, WorkspaceId};
use amx_proto::control::pane::SplitDirection;
use amx_proto::control::{Call, workspace as workspace_proto};

use super::{App, Mode, Projection};
use crate::cache::RowSlot;
use crate::copy::{CopyMode, Outcome};
use crate::input::{Action, Chrome, InputEvent, Wheel};
use crate::model::{Attrs, Cell};
use crate::picker::{Picker, PickerEvent};

/// What a picker row stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickTarget {
    /// Switch to this workspace.
    Workspace(WorkspaceId),
    /// Focus this pane in the focused workspace.
    Pane(PaneId),
    /// Run this built-in command.
    Command(Action),
}

/// The open picker and what its rows map back to.
#[derive(Debug)]
pub struct PickerUi {
    /// The fuzzy list.
    pub picker: Picker,
    /// One target per item, same order.
    pub targets: Vec<PickTarget>,
}

/// The live copy-mode engine and the pane it runs over.
#[derive(Debug)]
pub struct CopyUi {
    /// The pane whose scrollback is being browsed.
    pub pane: PaneId,
    /// The engine.
    pub engine: CopyMode,
}

/// Rows the picker overlay shows at most.
const PICKER_ROWS: u16 = 8;

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Open the picker over workspaces, panes and commands.
    ///
    /// Every surface in this file reports [`Effect::Full`] and none of them
    /// reports less: an overlay is drawn over the panes and taken away again,
    /// so what it invalidates is the frame rather than any pane in it.
    #[must_use]
    pub(super) fn open_picker(&mut self) -> Effect {
        let mut items = Vec::new();
        let mut targets = Vec::new();
        let mut ids: Vec<WorkspaceId> = self.model.workspace_ids().collect();
        ids.sort();
        for id in ids {
            let label = self
                .model
                .focused_workspace_id()
                .filter(|f| *f == id)
                .map_or("", |_| "* ");
            let name = self
                .model
                .workspace_label(id)
                .unwrap_or_else(|| short_id(id.to_string()));
            items.push(format!("{label}workspace: {name}"));
            targets.push(PickTarget::Workspace(id));
        }
        if let Some(ws) = self.model.focused_workspace() {
            for (n, pane) in ws.layout.panes().into_iter().enumerate() {
                items.push(format!("pane {}: {}", n + 1, short_id(pane.to_string())));
                targets.push(PickTarget::Pane(pane));
            }
        }
        for (label, action) in [
            (
                "command: split right",
                Action::Split(SplitDirection::Vertical),
            ),
            (
                "command: split down",
                Action::Split(SplitDirection::Horizontal),
            ),
            ("command: zoom", Action::Zoom),
            ("command: detach", Action::Detach),
        ] {
            items.push(label.to_owned());
            targets.push(PickTarget::Command(action));
        }
        self.picker = Some(PickerUi {
            picker: Picker::new(items),
            targets,
        });
        Effect::Full
    }

    /// Route bytes to the open picker.
    #[must_use]
    pub(super) fn picker_input(
        &mut self,
        bytes: &[u8],
        sink: &mut impl FnMut(InputEvent<'_>),
    ) -> Effect {
        // Through the machine's chrome split rather than byte by byte: a mouse
        // report's leading `ESC` is the picker's cancel key, so a wheel turn
        // over an open picker would close it. The picker interprets no mouse
        // event at all, so every report — wheel included — is dropped here.
        let mut pieces = self.input.take_chrome();
        self.input.feed_chrome(bytes, &mut pieces);
        'pieces: for &piece in &pieces {
            let Chrome::Keys { start, end } = piece else {
                continue;
            };
            for &byte in &bytes[start..end] {
                let Some(ui) = self.picker.as_mut() else {
                    break 'pieces;
                };
                match ui.picker.key(byte) {
                    PickerEvent::Continue => {}
                    PickerEvent::Cancelled => {
                        self.picker = None;
                    }
                    PickerEvent::Chosen(index) => {
                        let target = ui.targets.get(index).copied();
                        self.picker = None;
                        match target {
                            Some(PickTarget::Workspace(id)) => {
                                self.model.focus_workspace(id);
                                sink(InputEvent::Call(Call::WorkspaceSwitch(
                                    workspace_proto::SwitchParams { workspace: id },
                                )));
                            }
                            Some(PickTarget::Pane(pane)) => {
                                if let Some(ws) = self.model.focused_workspace_id() {
                                    self.focus.insert(ws, pane);
                                }
                            }
                            Some(PickTarget::Command(action)) => {
                                // Folded into this call's own `Full` rather
                                // than returned: the picker closing already
                                // invalidates the frame, and nothing an action
                                // can report is stronger than that.
                                let _ = self.apply_action(action, &[], sink);
                            }
                            None => {}
                        }
                    }
                }
            }
        }
        self.input.put_chrome(pieces);
        Effect::Full
    }

    /// Enter copy mode over the focused pane's cache, or fall straight back
    /// to terminal mode when there is no history to browse.
    ///
    /// A no-op when the mode is already open, which is what lets the wheel
    /// exception open it *and scroll it* inside one round of input: the entry
    /// point calls this again once the machine's actions have run, and a second
    /// entry that reset the engine to the bottom would undo the scroll that
    /// asked for it.
    #[must_use]
    pub(super) fn enter_copy(&mut self) -> Effect {
        if self.copy.is_some() {
            return Effect::Nothing;
        }
        let opened = (|| {
            let pane = self.focused_pane()?;
            let height = self.copy_height(pane);
            let cache = self.caches.get(&pane)?;
            let engine = CopyMode::open(cache, height)?;
            Some(CopyUi { pane, engine })
        })();
        match opened {
            Some(ui) => {
                self.queue_copy_misses(&ui);
                self.copy = Some(ui);
            }
            None => self.mode = Mode::Terminal,
        }
        Effect::Full
    }

    /// Route bytes to the copy engine, mirroring the machine's `mode_after`.
    ///
    /// Through the chrome split for the reason the picker is: an SGR report's
    /// bytes are `ESC`, `[`, `<`, digits and `M` — which this mode's own table
    /// reads as leave, then junk, then junk, then a column move. A wheel turn
    /// reaches the engine as a wheel turn; every other report is dropped before
    /// it can be mistaken for a keystroke.
    #[must_use]
    pub(super) fn copy_input(&mut self, bytes: &[u8]) -> Effect {
        let mut pieces = self.input.take_chrome();
        self.input.feed_chrome(bytes, &mut pieces);
        for &piece in &pieces {
            match piece {
                Chrome::Keys { start, end } => {
                    for &byte in &bytes[start..end] {
                        if !self.copy_key(byte) {
                            break;
                        }
                    }
                }
                Chrome::Wheel(wheel) => {
                    self.copy_wheel(wheel);
                }
            }
        }
        self.input.put_chrome(pieces);
        Effect::Full
    }

    /// One byte through the engine; answers whether copy mode is still open.
    fn copy_key(&mut self, byte: u8) -> bool {
        let Some(mut ui) = self.copy.take() else {
            self.mode = Mode::Terminal;
            return false;
        };
        let cache = self.caches.entry(ui.pane).or_default();
        match ui.engine.key(byte, cache) {
            Outcome::Continue => {
                self.queue_copy_misses(&ui);
                self.copy = Some(ui);
                true
            }
            Outcome::Exit => {
                self.mode = Mode::Terminal;
                false
            }
            Outcome::Yank(osc) => {
                self.emit.extend_from_slice(&osc);
                self.mode = Mode::Terminal;
                false
            }
        }
    }

    /// One wheel turn through the engine (D14). A wheel-down at the live edge
    /// ends the mode, which is the exit the exception is built around.
    fn copy_wheel(&mut self, wheel: Wheel) {
        let Some(mut ui) = self.copy.take() else {
            self.mode = Mode::Terminal;
            return;
        };
        let cache = self.caches.entry(ui.pane).or_default();
        match ui.engine.wheel(wheel, cache) {
            Outcome::Continue => {
                self.queue_copy_misses(&ui);
                self.copy = Some(ui);
            }
            // The engine's wheel never yanks; `Exit` is the live edge.
            Outcome::Exit | Outcome::Yank(_) => self.mode = Mode::Terminal,
        }
    }

    /// D14's wheel exception: a wheel-up in a pane that asked for no mouse
    /// reports opens copy mode over that pane's cached scrollback and scrolls
    /// it one notch, so the turn that asked for history is the turn that shows
    /// it rather than merely opening an unmoved view.
    ///
    /// A pane with nothing fetchable does not enter at all — `enter_copy` puts
    /// the mode straight back — which is the same answer navigate's `c` gives
    /// and the honest one: there is no scrollback to look at.
    #[must_use]
    pub(super) fn wheel_into_copy(&mut self) -> Effect {
        self.mode = Mode::Copy;
        let effect = self.enter_copy();
        if self.copy.is_some() {
            self.copy_wheel(Wheel::Up);
        }
        effect
    }

    /// The live copy-mode view, when one is open.
    ///
    /// Public because where the scrollback is parked is the observable half of
    /// D14's wheel exception, and a test that could only see the *mode* would
    /// be asserting that copy mode opened rather than that the wheel scrolled.
    #[must_use]
    pub const fn copy_view(&self) -> Option<&CopyUi> {
        self.copy.as_ref()
    }

    /// Queue the viewport's cache misses for the wired loop to fetch.
    fn queue_copy_misses(&mut self, ui: &CopyUi) {
        let Some(cache) = self.caches.get(&ui.pane) else {
            return;
        };
        let mut ranges = Vec::new();
        ui.engine.wanted(cache, &mut ranges);
        for range in ranges {
            if !self.wanted_history.contains(&(ui.pane, range)) {
                self.wanted_history.push((ui.pane, range));
            }
        }
    }

    /// The viewport height copy mode browses at: the focused pane's interior.
    fn copy_height(&self, pane: PaneId) -> u16 {
        self.pane_rects
            .iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, rect)| self.pane_interior(*rect).h)
            .filter(|h| *h > 0)
            .unwrap_or(1)
    }

    /// Draw whichever overlay is active. Runs after the pane grids, before
    /// the status line, so overlays paint over content but never chrome.
    pub(super) fn draw_overlays(&mut self) {
        if let Some(ui) = self.copy.take() {
            self.draw_copy(&ui);
            self.copy = Some(ui);
        }
        if let Some(ui) = self.picker.take() {
            self.draw_picker(&ui);
            self.picker = Some(ui);
        }
    }

    /// Copy mode fills the focused pane's interior with cached rows.
    fn draw_copy(&mut self, ui: &CopyUi) {
        let Some(&(_, rect)) = self.pane_rects.iter().find(|(id, _)| id == &ui.pane) else {
            return;
        };
        // The interior the *projection* leaves, not the border's: D14's
        // single-pane projection draws no border, so insetting for one would
        // put copy mode's rows a cell in from a frame nothing drew.
        let inner = self.pane_interior(rect);
        if inner.w == 0 || inner.h == 0 {
            return;
        }
        let cache = self.caches.entry(ui.pane).or_default();
        let top = ui.engine.top().get();
        let selection = ui.engine.selection();
        let cursor = ui.engine.cursor();
        for line in 0..inner.h {
            let row = RowId::from_raw(top.saturating_add(u64::from(line)));
            self.writer.move_to(inner.y + line, inner.x);
            let text = match cache.slot(row) {
                RowSlot::Cached(text) => text,
                RowSlot::Missing => "…",
                RowSlot::Unavailable => "~",
            };
            let mut chars = text.chars();
            for col in 0..inner.w {
                let ch = chars.next().unwrap_or(' ');
                let selected =
                    in_selection(selection, row, col) || (cursor.row == row && cursor.col == col);
                self.writer.write_cell(&Cell {
                    ch: if ch == '\0' { ' ' } else { ch },
                    attrs: Attrs {
                        reverse: selected,
                        ..Attrs::default()
                    },
                });
            }
        }
    }

    /// The picker draws as a query line plus its best matches, top of screen —
    /// and, under D14's narrow projection, over the whole content area.
    ///
    /// 10 §D14: "picker and agents view render full-screen instead of as an
    /// overlay region". A terminal wide enough to tile can spare the rows below
    /// the list to the panes underneath; one that is showing a single pane
    /// because it has no width to spare cannot, and eight rows of list over
    /// twelve rows of a pane the user cannot read is the region the policy
    /// exists to replace.
    fn draw_picker(&mut self, ui: &PickerUi) {
        let width = self.model.term.w;
        let content = self.model.content_area().h;
        let height = match self.projection() {
            Projection::Single(_) => content,
            Projection::Tiled => (PICKER_ROWS + 1).min(content),
        };
        if width == 0 || height == 0 {
            return;
        }
        let query = format!("> {}", ui.picker.query());
        draw_line(self, 0, width, &query, false);
        let selected = ui.picker.selected();
        let mut row = 1_u16;
        for &item in ui
            .picker
            .matches()
            .iter()
            .take(usize::from(height.saturating_sub(1)))
        {
            let label = &ui.picker.items()[item];
            draw_line(self, row, width, label, selected == Some(item));
            row += 1;
        }
        // Only the full-screen form pads: the overlay form is *supposed* to
        // leave the panes below it showing, and blanking down to the status
        // line would turn every short match list into a full-height dialog.
        if matches!(self.projection(), Projection::Single(_)) {
            for blank in row..height {
                draw_line(self, blank, width, "", false);
            }
        }
    }
}

/// Whether `(row, col)` falls inside the ordered selection.
fn in_selection(
    selection: Option<(crate::copy::Point, crate::copy::Point)>,
    row: RowId,
    col: u16,
) -> bool {
    let Some((start, end)) = selection else {
        return false;
    };
    if row < start.row || row > end.row {
        return false;
    }
    let after_start = row > start.row || col >= start.col;
    let before_end = row < end.row || col <= end.col;
    after_start && before_end
}

/// Paint one full-width overlay line, optionally highlighted.
fn draw_line<Fd: AsFd, W: Write>(
    app: &mut App<Fd, W>,
    row: u16,
    width: u16,
    text: &str,
    selected: bool,
) {
    app.writer.move_to(row, 0);
    let mut chars = text.chars();
    for _ in 0..width {
        let ch = chars.next().unwrap_or(' ');
        app.writer.write_cell(&Cell {
            ch,
            attrs: Attrs {
                reverse: !selected,
                bold: selected,
                ..Attrs::default()
            },
        });
    }
}

/// The head of a UUID, enough to tell panes apart in a list.
fn short_id(id: String) -> String {
    id.chars().take(8).collect()
}
