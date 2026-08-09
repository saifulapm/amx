//! The status line and the cursor: what a repaint draws after the panes.
//!
//! 04 §3 leaves presentation to the client — "draws its own chrome (borders,
//! status line, picker)" — so everything the status line says is built here
//! out of mirrored state, never received as text. What it says is the
//! per-workspace attention breakdown D15 asks for, the focused pane's label
//! and agent status, the interaction mode, the global attention count 04 §5
//! asks for ("the status line renders `⚑3` from the same state" the attention
//! queue holds) with the head of that queue and how long it has waited, and
//! the restore-loss indicator 04 §6 asks for: "panes/workspaces that fail to
//! respawn produce a restore report shown in the status line ... never
//! log-only".
//!
//! ```text
//!  [api ⚑2] [web] [infra ⚑1] · backend · blocked (permission_dialog) ⚑3 api/backend 4m
//! ```
//!
//! Below `client.narrow_cols` it degrades to D14's compact form — the active
//! workspace, the global count and the queue head — because the breakdown
//! needs width a phone does not have.
//!
//! This file is what a *repaint* does with all that: gather the inputs, hand
//! them to [`line`], and draw the answer. [`line`] is the line itself — how it
//! is built and what keeps building it cheap — and [`clock`] is the estimate
//! every age on it is rendered against.

mod clock;
mod line;

use std::io::Write;
use std::os::fd::AsFd;
use std::time::Instant;

use amx_core::Effect;
use amx_core::agent::EpochMillis;

use super::{App, Mode};
use crate::config::NarrowCols;
use crate::render::chrome;

pub(super) use line::StatusLine;

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Refresh the cached status line and draw it across the bottom row.
    pub(super) fn draw_status(&mut self) {
        // The *recorded* focus, not [`App::focused_pane`]'s self-healing one:
        // a repaint reports what the client knows, and healing focus is an
        // input consequence, not a paint one.
        let focused = self
            .model
            .focused_workspace_id()
            .and_then(|ws| self.focus.get(&ws))
            .copied();
        let losses = self
            .model
            .restore()
            .map_or(0, |restore| restore.lost.saturating_add(restore.degraded));
        let mode = mode_tag(self.mode, self.picker.is_some());

        self.status.refresh(
            line::Inputs {
                model: &self.model,
                focused,
                mode,
                losses,
            },
            Instant::now(),
        );
        chrome::status_line(
            &mut self.writer,
            self.model.term.h.saturating_sub(1),
            self.model.term.w,
            self.status.text(),
            self.status.active(),
        );
    }

    /// Set the width below which the status line degrades to D14's compact
    /// form.
    ///
    /// The client's own configuration reaches the app the way its bindings do
    /// ([`App::input`] and `amx_client::config::Settings`): read once at attach
    /// and handed in, because nothing in this crate reads a file.
    pub fn set_narrow_cols(&mut self, cols: NarrowCols) {
        self.status.set_narrow(cols);
        self.absorb(Effect::Full);
    }

    /// Tell the status line what the server's wall clock reads, from a reply
    /// that carries it.
    ///
    /// D-M4-4's `now`. Until a surface in this client calls `agent.list` the
    /// estimate is anchored on the stamps the snapshots themselves carry, which
    /// is a lower bound on this; a real `now` supersedes it the moment one is
    /// in hand, and every later age is that much tighter.
    pub fn note_server_clock(&mut self, now: EpochMillis) {
        self.status.observe_clock(now, Instant::now());
        self.absorb(Effect::Full);
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
