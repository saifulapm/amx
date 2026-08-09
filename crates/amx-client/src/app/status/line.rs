//! The line itself: what it says, and what keeps saying it cheap.
//!
//! # One source for every count
//!
//! `⚑N` and every `⚑n` inside a segment are derived from the same vector, the
//! mirrored attention queue: a segment counts the queued panes its workspace's
//! layout holds, and the global count is the queue's length. There is no second
//! predicate that could answer differently — a per-workspace tally kept beside
//! the queue would be one more thing to keep in step, and 04 §5's whole point
//! is that the number a user reads and the queue `next-attention` walks are the
//! same number.
//!
//! # The line is compared as text
//!
//! The line is rebuilt into a scratch buffer on every repaint and swapped in
//! only when it differs from what is already drawn, so an unchanged status
//! costs one string comparison and no allocation. That guard used to be a list
//! of the six inputs the text was built from, which was the trap this module
//! documented against itself: a field added to the rendered text and not to the
//! comparison rendered once and then froze, silently, because every later
//! refresh short-circuited. Comparing the rendered line — the text, and the
//! emphasis that is not in it — is the same guard with the trap taken out:
//! every input is in it by construction, including the ones a later task adds,
//! and `status_line_renders_attention_count_and_updates_on_dequeue` still fails
//! if a refresh ever stops seeing one.
//!
//! Rebuilding rather than comparing costs a few hundred bytes of `push_str` per
//! repaint and no allocation, which is the trade `the_status_breakdown_does_not_
//! allocate_after_the_first_frame` pins: every buffer here is cleared and
//! refilled, never freed.

use std::fmt::Write as _;
use std::ops::Range;
use std::time::Instant;

use amx_core::agent::{AgentSnapshot, AgentState};
use amx_core::{PaneId, WorkspaceId};

use super::clock::{ServerClock, push_age};
use crate::config::NarrowCols;
use crate::model::ClientModel;

/// What the status line says when the focused workspace has no label — and
/// when there is no focused workspace at all.
const DEFAULT_LABEL: &str = "amx";

/// What separates the workspace breakdown from the focused pane's label, and
/// that from its agent status.
const SEPARATOR: &str = " · ";

/// The restore-loss indicator's glyph, followed by the number of entries.
const LOSS_MARK: char = '⚠';

/// The attention indicator's glyph, followed by how many agents are waiting.
///
/// 04 §5 names it: "`next-attention` (one prefix key) focuses the head; the
/// status line renders `⚑3` from the same state".
const ATTENTION_MARK: char = '⚑';

/// How much of an unlabelled workspace's or pane's id stands in for a name.
///
/// The picker's `short_id` prefix, so an object nobody has named reads the same
/// on both surfaces.
const SHORT_ID: usize = 8;

/// The rendered status line, the emphasis it carries, and the buffers it is
/// rebuilt in.
#[derive(Debug, Default)]
pub(crate) struct StatusLine {
    /// What the last refresh decided the line says.
    text: String,
    /// The byte range of the active workspace within [`Self::text`].
    active: Range<usize>,
    /// The line being built this refresh, swapped with [`Self::text`] when the
    /// two differ. Kept across refreshes so a rebuild reuses its capacity.
    scratch: String,
    /// [`Self::active`] for the line being built.
    scratch_active: Range<usize>,
    /// Every mirrored workspace, in the order the segments are drawn. A field
    /// rather than a local for the same reason [`Self::scratch`] is.
    order: Vec<WorkspaceId>,
    /// How many queued panes each workspace of [`Self::order`] holds.
    counts: Vec<u32>,
    /// Below this width the line degrades to D14's compact form.
    narrow: NarrowCols,
    /// The estimate every age is rendered against.
    clock: ServerClock,
}

impl StatusLine {
    /// The bytes to draw.
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    /// The part of [`Self::text`] naming the workspace this client is showing.
    pub(super) fn active(&self) -> Range<usize> {
        self.active.clone()
    }

    /// Set the width below which the compact form takes over.
    pub(super) fn set_narrow(&mut self, cols: NarrowCols) {
        self.narrow = cols;
    }

    /// Take a server instant as evidence of where the server's clock is.
    pub(super) fn observe_clock(&mut self, now: amx_core::agent::EpochMillis, at: Instant) {
        self.clock.observe_at(now, at);
    }

    /// Rebuild the line, and keep the old one if it says the same thing.
    pub(super) fn refresh(&mut self, inputs: Inputs<'_>, at: Instant) {
        let Inputs {
            model,
            focused,
            mode,
            losses,
        } = inputs;
        // Every stamp the mirror holds, not only the queue's: the newest one
        // anywhere is the tightest bound on the server's clock, and the agents
        // that moved most recently are exactly the ones that are *not* waiting.
        for (_, agent) in model.agents() {
            if let Some(since) = agent.since {
                self.clock.observe_at(since, at);
            }
        }
        self.count_attention(model);

        self.scratch.clear();
        self.scratch_active = 0..0;
        self.scratch.push(' ');
        if self.narrow.is_narrow(model.term.w) {
            self.push_active(model);
        } else {
            self.push_breakdown(model);
            if let Some(pane) = focused.and_then(|pane| model.pane_label(pane)) {
                self.scratch.push_str(SEPARATOR);
                self.scratch.push_str(pane);
            }
            // The focused pane's own status, beside the name it belongs to: 04
            // §5 wants per-pane state visible, and this is the pane the user is
            // looking at. Panes they are not looking at are counted by `⚑N`.
            if let Some(agent) = focused.and_then(|pane| model.pane_agent(pane)) {
                self.scratch.push_str(SEPARATOR);
                self.scratch.push_str(agent.state.as_str());
                self.push_reason(agent);
            }
        }
        self.scratch.push_str(mode);
        let waiting = model.attention_count();
        if waiting > 0 {
            self.scratch.push(' ');
            self.scratch.push(ATTENTION_MARK);
            // Writing into a `String` that already has its steady-state
            // capacity does not allocate, and neither does anything else in
            // this rebuild.
            let _ = write!(self.scratch, "{waiting}");
        }
        self.push_head(model, at);
        if losses > 0 {
            self.scratch.push(' ');
            self.scratch.push(LOSS_MARK);
            let _ = write!(self.scratch, "{losses}");
        }
        self.scratch.push(' ');

        // The emphasis is part of what is drawn and therefore part of the
        // guard: moving focus between two workspaces whose segments say the
        // same thing changes which of them is drawn out of the bar and nothing
        // else, and a guard that watched only the text would leave the mark on
        // the workspace the user just left.
        if self.scratch != self.text || self.scratch_active != self.active {
            std::mem::swap(&mut self.text, &mut self.scratch);
            self.active = self.scratch_active.clone();
        }
    }

    /// Order the workspaces and tally the queue across them.
    ///
    /// Named workspaces first and alphabetically, because the question this
    /// line answers is "which project needs me" and a name is what a user scans
    /// for; anything nobody has named trails, and the id breaks every tie, so
    /// the segments never reshuffle between two repaints of the same state.
    fn count_attention(&mut self, model: &ClientModel) {
        self.order.clear();
        self.order.extend(model.workspace_ids());
        self.order
            .sort_unstable_by(|a, b| sort_key(model, *a).cmp(&sort_key(model, *b)));
        self.counts.clear();
        self.counts.resize(self.order.len(), 0);
        for &pane in model.attention() {
            let Some(at) = self.order.iter().position(|&id| holds(model, id, pane)) else {
                continue;
            };
            if let Some(count) = self.counts.get_mut(at) {
                *count = count.saturating_add(1);
            }
        }
    }

    /// D15's per-workspace segments: `[api ⚑2] [web] [infra ⚑1]`.
    fn push_breakdown(&mut self, model: &ClientModel) {
        if self.order.is_empty() {
            self.scratch.push_str(DEFAULT_LABEL);
            return;
        }
        for (n, &id) in self.order.iter().enumerate() {
            if n > 0 {
                self.scratch.push(' ');
            }
            let active = model.focused_workspace_id() == Some(id);
            let start = self.scratch.len();
            self.scratch.push('[');
            push_name(&mut self.scratch, label_of(model, id), id);
            match self.counts.get(n).copied().unwrap_or(0) {
                0 => {}
                waiting => {
                    self.scratch.push(' ');
                    self.scratch.push(ATTENTION_MARK);
                    let _ = write!(self.scratch, "{waiting}");
                }
            }
            self.scratch.push(']');
            if active {
                self.scratch_active = start..self.scratch.len();
            }
        }
    }

    /// D14's compact left side: the workspace this client is showing, alone.
    fn push_active(&mut self, model: &ClientModel) {
        let start = self.scratch.len();
        match model.focused_workspace_id() {
            Some(id) => push_name(&mut self.scratch, label_of(model, id), id),
            None => self.scratch.push_str(DEFAULT_LABEL),
        }
        self.scratch_active = start..self.scratch.len();
    }

    /// The detector's own name for what asserted a blocked pane's state.
    ///
    /// Blocked only, which is what D15 asks for and what the field is worth:
    /// `reason` names the winning manifest rule or the hook event (D-M4-3), and
    /// the question it answers — *blocked on what?* — is only asked about a
    /// pane that is waiting. It is absent for tier-3 states, and absent after a
    /// transition this client saw as an `agent_status` and nothing else, so
    /// nothing here may assume it is present.
    fn push_reason(&mut self, agent: &AgentSnapshot) {
        if agent.state != AgentState::Blocked {
            return;
        }
        if let Some(reason) = agent.reason.as_deref() {
            self.scratch.push_str(" (");
            self.scratch.push_str(reason);
            self.scratch.push(')');
        }
    }

    /// The head of the attention queue, and how long it has waited.
    ///
    /// The head and not a summary of the queue: it is the agent that has waited
    /// longest anywhere (the queue is global and block-time ordered, D-M2-8),
    /// so `workspace/name` is where `next-attention` would take the user.
    fn push_head(&mut self, model: &ClientModel, at: Instant) {
        let Some(&head) = model.attention().first() else {
            return;
        };
        self.scratch.push(' ');
        match self
            .order
            .iter()
            .copied()
            .find(|&id| holds(model, id, head))
        {
            Some(id) => push_name(&mut self.scratch, label_of(model, id), id),
            None => self.scratch.push_str(DEFAULT_LABEL),
        }
        self.scratch.push('/');
        push_name(&mut self.scratch, model.pane_label(head), head);
        // No stamp, no age: a pane whose status was re-derived rather than
        // observed entering has no entry edge to report, and an age invented
        // for it would read as measured.
        if let Some(age) = model
            .pane_agent(head)
            .and_then(|agent| agent.since)
            .and_then(|since| self.clock.age_at(since, at))
        {
            self.scratch.push(' ');
            push_age(&mut self.scratch, age);
        }
    }
}

/// A workspace's label, borrowed rather than cloned.
fn label_of(model: &ClientModel, id: WorkspaceId) -> Option<&str> {
    model.workspace(id).and_then(|ws| ws.label.as_deref())
}

/// What the segments are ordered by: named before unnamed, then the name, then
/// the id.
fn sort_key(model: &ClientModel, id: WorkspaceId) -> (bool, &str, WorkspaceId) {
    let label = label_of(model, id);
    (label.is_none(), label.unwrap_or(""), id)
}

/// Whether `workspace`'s mirrored layout holds `pane`.
fn holds(model: &ClientModel, workspace: WorkspaceId, pane: PaneId) -> bool {
    model
        .workspace(workspace)
        .is_some_and(|ws| ws.layout.contains(pane))
}

/// Write `label`, or a prefix of the id of whatever nobody has named.
fn push_name(out: &mut String, label: Option<&str>, id: impl std::fmt::Display) {
    if let Some(label) = label
        && !label.is_empty()
    {
        out.push_str(label);
        return;
    }
    let start = out.len();
    let _ = write!(out, "{id}");
    // A hyphenated UUID is ASCII, so a byte truncation is a char truncation.
    out.truncate(start + SHORT_ID);
}

/// Everything the status line is built from, gathered once per repaint.
///
/// A struct rather than four positional arguments: two of the fields are the
/// same shape, and a call site that transposed them would render a
/// plausible-looking wrong line rather than fail to compile.
pub(super) struct Inputs<'a> {
    pub(super) model: &'a ClientModel,
    /// The pane focus is recorded on in the workspace being drawn, if any.
    pub(super) focused: Option<PaneId>,
    pub(super) mode: &'static str,
    pub(super) losses: u32,
}
