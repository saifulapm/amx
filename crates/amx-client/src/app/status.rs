//! The status line and the cursor: what a repaint draws after the panes.
//!
//! 04 §3 leaves presentation to the client — "draws its own chrome (borders,
//! status line, picker)" — so everything the status line says is built here
//! out of mirrored state, never received as text. What it says today is the
//! focused workspace's label, the focused pane's label, the interaction mode,
//! and the restore-loss indicator 04 §6 asks for: "panes/workspaces that fail
//! to respawn produce a restore report shown in the status line ... never
//! log-only".
//!
//! The line is cached against its inputs rather than rebuilt per frame.
//! Repainting happens on every keystroke and every damage delta; a label or a
//! mode change does not, so a repaint of an unchanged status costs four
//! comparisons instead of a `String` rebuild — which is what keeps
//! `repaint_does_not_allocate_after_the_first_frame` true.

use std::fmt::Write as _;
use std::io::Write;
use std::os::fd::AsFd;

use super::{App, Mode};
use crate::render::chrome;

/// What the status line says when the focused workspace has no label — and
/// when there is no focused workspace at all.
const DEFAULT_LABEL: &str = "amx";

/// What separates the workspace's label from the focused pane's.
const SEPARATOR: &str = " · ";

/// The restore-loss indicator's glyph, followed by the number of entries.
const LOSS_MARK: char = '⚠';

/// The rendered status line, and the inputs it was rendered from.
#[derive(Debug, Default)]
pub(super) struct StatusLine {
    text: String,
    workspace: String,
    pane: String,
    mode: &'static str,
    losses: u32,
    built: bool,
}

impl StatusLine {
    /// The bytes to draw.
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    /// Rebuild the line if any of its inputs changed, and do nothing at all if
    /// none did.
    fn refresh(&mut self, workspace: &str, pane: Option<&str>, mode: &'static str, losses: u32) {
        let pane = pane.unwrap_or("");
        if self.built
            && self.workspace == workspace
            && self.pane == pane
            && self.mode == mode
            && self.losses == losses
        {
            return;
        }
        self.workspace.clear();
        self.workspace.push_str(workspace);
        self.pane.clear();
        self.pane.push_str(pane);
        self.mode = mode;
        self.losses = losses;
        self.built = true;

        self.text.clear();
        self.text.push(' ');
        self.text.push_str(workspace);
        if !pane.is_empty() {
            self.text.push_str(SEPARATOR);
            self.text.push_str(pane);
        }
        self.text.push_str(mode);
        if losses > 0 {
            self.text.push(' ');
            self.text.push(LOSS_MARK);
            // Writing into a `String` that already has its steady-state
            // capacity does not allocate, and this branch only runs on the
            // frames where the count actually changed.
            let _ = write!(self.text, "{losses}");
        }
        self.text.push(' ');
    }
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Refresh the cached status line and draw it across the bottom row.
    pub(super) fn draw_status(&mut self) {
        let workspace = self
            .model
            .focused_workspace()
            .and_then(|ws| ws.label.as_deref())
            .unwrap_or(DEFAULT_LABEL);
        // The *recorded* focus, not [`App::focused_pane`]'s self-healing one:
        // a repaint reports what the client knows, and healing focus is an
        // input consequence, not a paint one.
        let pane = self
            .model
            .focused_workspace_id()
            .and_then(|ws| self.focus.get(&ws))
            .and_then(|pane| self.model.pane_label(*pane));
        let losses = self
            .model
            .restore()
            .map_or(0, |restore| restore.lost.saturating_add(restore.degraded));
        let mode = mode_tag(self.mode, self.picker.is_some());

        self.status.refresh(workspace, pane, mode, losses);
        chrome::status_line(
            &mut self.writer,
            self.model.term.h.saturating_sub(1),
            self.model.term.w,
            self.status.text(),
        );
    }

    /// Park the terminal cursor on the focused pane's cursor cell, when it is
    /// visible; hide it otherwise so chrome never shows a stray block.
    pub(super) fn place_cursor(&mut self) {
        if self.picker.is_some() || self.copy.is_some() {
            self.writer.set_cursor_visible(false);
            return;
        }
        let placed = (|| {
            let ws = self.model.focused_workspace_id()?;
            let pane = *self.focus.get(&ws)?;
            let rect = self
                .pane_rects
                .iter()
                .find(|(id, _)| *id == pane)
                .map(|(_, rect)| *rect)?;
            let inner = chrome::inset(rect);
            let grid = self.model.pane(pane)?;
            let cursor = grid.cursor();
            (cursor.visible && cursor.row < inner.h && cursor.col < inner.w)
                .then(|| (inner.y + cursor.row, inner.x + cursor.col))
        })();
        match placed {
            Some((row, col)) => {
                self.writer.move_to(row, col);
                self.writer.set_cursor_visible(true);
            }
            None => self.writer.set_cursor_visible(false),
        }
    }
}

/// The status line's mode suffix.
const fn mode_tag(mode: Mode, picker: bool) -> &'static str {
    if picker {
        " PICK"
    } else {
        match mode {
            Mode::Terminal => "",
            Mode::Prefix => " PREFIX",
            Mode::Navigate => " NAV",
            Mode::Copy => " COPY",
        }
    }
}
