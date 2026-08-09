//! The board's paint, and the half of the screen it leaves alone.
//!
//! The view is full-screen — the whole content area, the status line
//! underneath untouched. That is not the picker's overlay shape and it is
//! deliberate: the picker is a chooser and can spare the rows below it to the
//! panes, while this is a *monitor* whose whole value is seeing 25 agents at
//! once.
//!
//! # The peek region is X15's, and so is the split
//!
//! Seam 5 is settled in [`App::peek_layout`](crate::app::App::peek_layout):
//! `PeekLayout { list, peek }`, computed once out of the content area and the
//! projection, so the list and the peek cannot disagree about where one ends.
//! This module draws inside `list` and **never** inside `peek` — not even to
//! blank it — because `App::draw_peek` runs after the overlays and paints that
//! rect whole, border and all.
//!
//! Two consequences worth naming rather than rediscovering:
//!
//! - Under D14's narrow projection `list` is zero rows, so the board draws
//!   *nothing* while a peek is open: 10 §D14's "peek replaces the list rather
//!   than sharing the width", including the header. A phone that opened a peek
//!   opened it to read the pane.
//! - The header is carved off the top of `list` here rather than reserved by
//!   the layout, because it belongs to the list: a peek that took the whole
//!   screen would otherwise leave one row of board floating over it.
//!
//! # Columns are dropped, never wrapped
//!
//! A row is `name`, its band mark, its status, its `reason`, its age and its
//! last screen line. The last two columns are dropped whole below
//! [`REASON_COLS`] and [`DETAIL_COLS`], because a phone at 45 columns wants the
//! four things that fit rather than six things wrapped across three rows —
//! which is the exact failure D14's projection exists to end.

use std::io::Write;
use std::os::fd::AsFd;
use std::time::Instant;

use amx_core::Rect;

use super::rows::{Band, Line};
use super::{AgentsUi, age_of};
use crate::app::App;
use crate::app::status::push_age;
use crate::model::{Attrs, Cell};

/// Columns given to `workspace/name`.
const NAME_W: usize = 16;
/// Columns given to the status word.
const STATUS_W: usize = 8;
/// Columns given to the detector's own name for the current state.
const REASON_W: usize = 18;
/// Columns given to the age.
const AGE_W: usize = 4;

/// Below this width the `reason` column is dropped.
const REASON_COLS: u16 = 56;
/// Below this width the detail column is dropped.
const DETAIL_COLS: u16 = 68;

/// What separates the row from its live detail line.
const DETAIL_MARK: &str = " │ ";

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Draw the board, when it is open.
    ///
    /// Taken out of `self` and put back for the reason the other overlays are:
    /// the frame writer and the view are two fields of one `App`, and a paint
    /// that borrowed both through `&mut self` would not compile.
    pub(in crate::app) fn draw_agents(&mut self) {
        let mut ui = std::mem::take(&mut self.agents);
        if ui.open {
            self.paint_agents(&mut ui);
        }
        self.agents = ui;
    }

    /// The paint itself.
    fn paint_agents(&mut self, ui: &mut AgentsUi) {
        // X15's seam, asked rather than recomputed: the rows the list has, once
        // the peek region (if any) has taken its share of the content area.
        // Zero rows is the narrow-with-a-peek case and means draw nothing —
        // including the header, which belongs to the list.
        let list = self.peek_layout().list;
        let width = list.w;
        if width == 0 || list.h == 0 {
            return;
        }
        // The header is the list's first row; the rest is the board.
        let header_row = list.y;
        let body = Rect::new(list.x, list.y.saturating_add(1), width, list.h - 1);
        let at = Instant::now();
        // The row buffer, borrowed out of the view so building a line and
        // drawing it are not two borrows of one struct. Cleared and refilled per
        // row and returned at the end, so a settled board repaints without
        // allocating.
        let mut line = std::mem::take(&mut ui.scratch);

        header(ui, &mut line);
        draw_row(
            self,
            header_row,
            width,
            &line,
            Attrs {
                bold: true,
                ..Attrs::default()
            },
        );

        // Enough scroll to keep the cursor on screen, and no more: the list
        // holds still while the selection moves inside it.
        let height = usize::from(body.h);
        let top = if height > 0 && ui.at >= height {
            ui.at + 1 - height
        } else {
            0
        };
        let mut row = body.y;
        for (n, &entry) in ui.visible.iter().enumerate().skip(top).take(height) {
            line.clear();
            match entry {
                Line::Agent(index) => paint_row(ui, index, width, at, &mut line),
                Line::Collapsed { count, .. } => {
                    line.push_str("  ");
                    push_count(&mut line, count);
                    line.push_str(" idle");
                }
            }
            draw_row(
                self,
                row,
                width,
                &line,
                Attrs {
                    reverse: n == ui.at,
                    ..Attrs::default()
                },
            );
            row = row.saturating_add(1);
        }
        // The rows the list owns and has nothing to put in: blanked, because the
        // panes were painted under this board and a row left untouched would
        // show one of them through the list.
        line.clear();
        for blank in row..body.y.saturating_add(body.h) {
            draw_row(self, blank, width, &line, Attrs::default());
        }
        ui.scratch = line;
    }
}

/// `agents — 25 · ⚑5 · by workspace`, plus whatever state the board is in.
fn header(ui: &AgentsUi, out: &mut String) {
    out.clear();
    if let Some((kind, text)) = ui.entry.as_ref() {
        out.push_str(kind.verb());
        out.push(' ');
        if let Some(row) = ui.selected_row() {
            out.push_str(&row.label);
        }
        out.push_str("> ");
        out.push_str(text);
        return;
    }
    if let Some(armed) = ui.armed.and_then(|pane| ui.row_of(pane)) {
        out.push_str("kill ");
        out.push_str(&armed.label);
        out.push_str("? ctrl+x again");
        return;
    }
    out.push_str("agents — ");
    push_count(out, ui.rows.len());
    if ui.queued > 0 {
        out.push_str(" · ");
        out.push(Band::Blocked.mark());
        push_count(out, ui.queued);
    }
    out.push_str(" · ");
    out.push_str(ui.grouping.as_str());
    if ui.blocked_only {
        out.push_str(" · blocked only");
    }
    let query = ui.picker.query();
    if !query.is_empty() {
        out.push_str(" · /");
        out.push_str(query);
    }
}

/// One agent's row, at `width` columns.
fn paint_row(ui: &AgentsUi, index: usize, width: u16, at: Instant, out: &mut String) {
    let Some(row) = ui.rows.get(index) else {
        return;
    };
    push_col(out, &row.label, NAME_W);
    out.push(' ');
    out.push(row.band().mark());
    out.push(' ');
    push_col(out, row.state.as_str(), STATUS_W);
    if width >= REASON_COLS {
        push_col(out, row.reason.as_deref().unwrap_or(""), REASON_W);
    }
    let start = out.len();
    if let Some(age) = age_of(&ui.clock, row.since, at) {
        push_age(out, age);
    }
    // Ages are ASCII, so the bytes written are the columns used.
    for _ in (out.len() - start)..AGE_W {
        out.push(' ');
    }
    if width >= DETAIL_COLS && !row.last_line.is_empty() {
        out.push_str(DETAIL_MARK);
        out.push_str(&row.last_line);
    }
}

/// Write `text` in exactly `width` columns, truncating or padding.
fn push_col(out: &mut String, text: &str, width: usize) {
    let mut used = 0;
    for ch in text.chars() {
        if used == width {
            break;
        }
        out.push(ch);
        used += 1;
    }
    for _ in used..width {
        out.push(' ');
    }
}

/// Write a count without allocating one.
fn push_count(out: &mut String, count: usize) {
    use std::fmt::Write as _;
    let _ = write!(out, "{count}");
}

/// Paint one full-width row of the board.
fn draw_row<Fd: AsFd, W: Write>(
    app: &mut App<Fd, W>,
    row: u16,
    width: u16,
    text: &str,
    attrs: Attrs,
) {
    app.writer.move_to(row, 0);
    let mut chars = text.chars();
    for _ in 0..width {
        let ch = chars.next().unwrap_or(' ');
        app.writer.write_cell(&Cell { ch, attrs });
    }
}
