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

use amx_core::{PaneId, RowId, WorkspaceId};
use amx_proto::control::pane::SplitDirection;
use amx_proto::control::{Call, workspace as workspace_proto};

use super::{App, Mode};
use crate::cache::RowSlot;
use crate::copy::{CopyMode, Outcome};
use crate::input::{Action, InputEvent};
use crate::model::{Attrs, Cell};
use crate::picker::{Picker, PickerEvent};
use crate::render::chrome;

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
    pub(super) fn open_picker(&mut self) {
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
        self.dirty = true;
    }

    /// Route bytes to the open picker.
    pub(super) fn picker_input(&mut self, bytes: &[u8], sink: &mut impl FnMut(InputEvent<'_>)) {
        for &byte in bytes {
            let Some(ui) = self.picker.as_mut() else {
                return;
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
                            self.layout_dirty = true;
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
                            self.apply_action(action, &[], sink);
                        }
                        None => {}
                    }
                }
            }
        }
        self.dirty = true;
    }

    /// Enter copy mode over the focused pane's cache, or fall straight back
    /// to terminal mode when there is no history to browse.
    pub(super) fn enter_copy(&mut self) {
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
        self.dirty = true;
    }

    /// Route bytes to the copy engine, mirroring the machine's `mode_after`.
    pub(super) fn copy_input(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let Some(mut ui) = self.copy.take() else {
                self.mode = Mode::Terminal;
                return;
            };
            let cache = self.caches.entry(ui.pane).or_default();
            match ui.engine.key(byte, cache) {
                Outcome::Continue => {
                    self.queue_copy_misses(&ui);
                    self.copy = Some(ui);
                }
                Outcome::Exit => self.mode = Mode::Terminal,
                Outcome::Yank(osc) => {
                    self.emit.extend_from_slice(&osc);
                    self.mode = Mode::Terminal;
                }
            }
        }
        self.dirty = true;
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
            .map(|(_, rect)| chrome::inset(*rect).h)
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
        let inner = chrome::inset(rect);
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

    /// The picker draws as a query line plus its best matches, top of screen.
    fn draw_picker(&mut self, ui: &PickerUi) {
        let width = self.model.term.w;
        if width == 0 {
            return;
        }
        let query = format!("> {}", ui.picker.query());
        draw_line(self, 0, width, &query, false);
        let selected = ui.picker.selected();
        for (line, &item) in ui
            .picker
            .matches()
            .iter()
            .take(usize::from(PICKER_ROWS))
            .enumerate()
        {
            let label = &ui.picker.items()[item];
            // Rows start below the query line; the cast is bounded by
            // PICKER_ROWS.
            #[allow(clippy::cast_possible_truncation, reason = "line < PICKER_ROWS")]
            let row = 1 + line as u16;
            if row >= self.model.term.h.saturating_sub(1) {
                break;
            }
            draw_line(self, row, width, label, selected == Some(item));
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
