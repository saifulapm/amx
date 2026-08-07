//! The status line and the cursor: what a repaint draws after the panes.
//!
//! 04 §3 leaves presentation to the client — "draws its own chrome (borders,
//! status line, picker)" — so everything the status line says is built here
//! out of mirrored state, never received as text. What it says today is the
//! focused workspace's label, the focused pane's label and agent status, the
//! interaction mode, the attention count 04 §5 asks for ("the status line
//! renders `⚑3` from the same state" the attention queue holds), and the
//! restore-loss indicator 04 §6 asks for: "panes/workspaces that fail to
//! respawn produce a restore report shown in the status line ... never
//! log-only".
//!
//! The line is cached against its inputs rather than rebuilt per frame.
//! Repainting happens on every keystroke and every damage delta; a label or a
//! mode change does not, so a repaint of an unchanged status costs six
//! comparisons instead of a `String` rebuild — which is what keeps
//! `repaint_does_not_allocate_after_the_first_frame` true.
//!
//! That cache is also the trap every new input walks into: a field added to the
//! rendered text and not to the equality guard renders once and then freezes,
//! silently, because every later refresh short-circuits. Both of M2's inputs —
//! the attention count and the focused pane's agent state — are in the guard
//! below, and `status_line_renders_attention_count_and_updates_on_dequeue` is
//! the regression test that fails if a seventh is ever added without one.

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

/// The attention indicator's glyph, followed by how many agents are waiting.
///
/// 04 §5 names it: "`next-attention` (one prefix key) focuses the head; the
/// status line renders `⚑3` from the same state".
const ATTENTION_MARK: char = '⚑';

/// The rendered status line, and the inputs it was rendered from.
#[derive(Debug, Default)]
pub(super) struct StatusLine {
    text: String,
    workspace: String,
    pane: String,
    /// The focused pane's agent state, or `""` when nothing is tracked there.
    /// A `&'static str` because [`AgentState::as_str`](amx_core::agent::AgentState::as_str)
    /// hands one out — comparing it costs a pointer and a length, not a
    /// `String`.
    agent: &'static str,
    mode: &'static str,
    attention: u32,
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
    fn refresh(&mut self, inputs: Inputs<'_>) {
        let Inputs {
            workspace,
            pane,
            agent,
            mode,
            attention,
            losses,
        } = inputs;
        let pane = pane.unwrap_or("");
        let agent = agent.unwrap_or("");
        if self.built
            && self.workspace == workspace
            && self.pane == pane
            && self.agent == agent
            && self.mode == mode
            && self.attention == attention
            && self.losses == losses
        {
            return;
        }
        self.workspace.clear();
        self.workspace.push_str(workspace);
        self.pane.clear();
        self.pane.push_str(pane);
        self.agent = agent;
        self.mode = mode;
        self.attention = attention;
        self.losses = losses;
        self.built = true;

        self.text.clear();
        self.text.push(' ');
        self.text.push_str(workspace);
        if !pane.is_empty() {
            self.text.push_str(SEPARATOR);
            self.text.push_str(pane);
        }
        // The focused pane's own status, beside the name it belongs to: 04 §5
        // wants per-pane state visible, and this is the pane the user is
        // looking at. Panes they are not looking at are counted by `⚑N`.
        if !agent.is_empty() {
            self.text.push_str(SEPARATOR);
            self.text.push_str(agent);
        }
        self.text.push_str(mode);
        if attention > 0 {
            self.text.push(' ');
            self.text.push(ATTENTION_MARK);
            // Writing into a `String` that already has its steady-state
            // capacity does not allocate, and this branch only runs on the
            // frames where the count actually changed.
            let _ = write!(self.text, "{attention}");
        }
        if losses > 0 {
            self.text.push(' ');
            self.text.push(LOSS_MARK);
            let _ = write!(self.text, "{losses}");
        }
        self.text.push(' ');
    }
}

/// Everything the status line is built from, gathered once per repaint.
///
/// A struct rather than six positional arguments: the cache above compares
/// every field, and a call site that transposed two `&str`s would render a
/// plausible-looking wrong line rather than fail to compile.
struct Inputs<'a> {
    workspace: &'a str,
    pane: Option<&'a str>,
    agent: Option<&'static str>,
    mode: &'static str,
    attention: u32,
    losses: u32,
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
        let focused = self
            .model
            .focused_workspace_id()
            .and_then(|ws| self.focus.get(&ws))
            .copied();
        let pane = focused.and_then(|pane| self.model.pane_label(pane));
        let agent = focused
            .and_then(|pane| self.model.pane_agent(pane))
            .map(|agent| agent.state.as_str());
        let losses = self
            .model
            .restore()
            .map_or(0, |restore| restore.lost.saturating_add(restore.degraded));
        let mode = mode_tag(self.mode, self.picker.is_some());

        self.status.refresh(Inputs {
            workspace,
            pane,
            agent,
            mode,
            attention: self.model.attention_count(),
            losses,
        });
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
