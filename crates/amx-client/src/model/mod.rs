//! Client-owned presentation state (04 §3).
//!
//! The client receives the layout tree and status as *state*, not pixels, and
//! draws its own chrome from it. [`ClientModel`] is that state: a mirror of
//! each visible workspace's [`Layout`] (the same pure BSP algebra
//! `amx-core` gives the server — split/close/swap here would be the same
//! tree rewrite there, which is what "presentation, not truth" means in
//! practice) plus one [`PaneGrid`] per pane the client is currently
//! rendering, plus what M2 added beside them: each pane's agent status and the
//! attention queue the status line counts.
//!
//! The cells live next door in [`grid`], re-exported from here so the module's
//! public path never mentions the split.

mod grid;

use std::collections::HashMap;

use amx_core::agent::{AgentKind, AgentSnapshot, AgentState, StatusCause};
use amx_core::{Layout, PaneId, Rect, Seq, WorkspaceId};
use amx_proto::control::session::RestoreSummary;

pub use grid::{Attrs, Cell, Color, PaneGrid};

/// One workspace's layout, mirrored client-side.
#[derive(Clone, Debug)]
pub struct WorkspaceModel {
    /// The workspace's user-visible label, if any.
    pub label: Option<String>,
    /// The BSP layout tree — the exact same pure algebra the server runs.
    pub layout: Layout,
}

/// Whether this client currently drives the size of the panes it can see.
///
/// Mirrors `amx_proto::control::client::SizeAuthority`; kept as a client-local
/// type because nothing on the wire currently declares or negotiates it (the
/// `client.viewport` call the real value would ride is not wired into the
/// control method table yet — see the crate-level gap note in `net`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SizeAuthority {
    /// This client's size sets pane grid sizes while it is the active one.
    Active,
    /// This client never drives pane sizes: it letterboxes/clips instead.
    Observer,
}

/// The client's whole presentation state: every workspace it knows about,
/// every pane grid it is caching, and which one has focus.
#[derive(Debug)]
pub struct ClientModel {
    workspaces: HashMap<WorkspaceId, WorkspaceModel>,
    panes: HashMap<PaneId, PaneGrid>,
    /// Each labelled pane's label, as `session.state` last reported it.
    ///
    /// Beside the grids rather than inside them: a [`PaneGrid`] is cells the
    /// server sent, and a label is state the server holds — they arrive on
    /// different paths and one can exist without the other.
    pane_labels: HashMap<PaneId, String>,
    /// Each tracked pane's agent status, from `session.state` and then from
    /// `agent_status`/`agent_identified` events.
    ///
    /// Beside the grids for the same reason the labels are: a status is state
    /// the server holds, arriving on a different path from the cells, and
    /// either can exist without the other.
    agents: HashMap<PaneId, AgentSnapshot>,
    /// The attention queue in queue order — the same order `session.state`
    /// reports and `agent.next` walks (D-M2-8).
    ///
    /// The status line's `⚑N` is this vector's length, so the count a user
    /// reads and the queue `next-attention` steps through cannot disagree.
    attention: Vec<PaneId>,
    focus_workspace: Option<WorkspaceId>,
    /// What the server's startup restore cost, if it performed one.
    ///
    /// Mirrored from `session.state` so the status line can render the
    /// indicator 04 §6 requires without a second call.
    restore: Option<RestoreSummary>,
    /// The client's own terminal size, chrome included.
    pub term: Rect,
}

impl ClientModel {
    /// An empty model at term size `rows` by `cols`.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            workspaces: HashMap::new(),
            panes: HashMap::new(),
            pane_labels: HashMap::new(),
            agents: HashMap::new(),
            attention: Vec::new(),
            focus_workspace: None,
            restore: None,
            term: Rect::new(0, 0, cols, rows),
        }
    }

    /// Insert or replace a workspace's mirrored layout.
    pub fn set_workspace(&mut self, id: WorkspaceId, model: WorkspaceModel) {
        self.workspaces.insert(id, model);
        self.focus_workspace.get_or_insert(id);
    }

    /// The workspace currently in focus, if any.
    #[must_use]
    pub fn focused_workspace(&self) -> Option<&WorkspaceModel> {
        self.focus_workspace.and_then(|id| self.workspaces.get(&id))
    }

    /// The id of the workspace currently in focus, if any.
    #[must_use]
    pub const fn focused_workspace_id(&self) -> Option<WorkspaceId> {
        self.focus_workspace
    }

    /// Every workspace id the client mirrors, in no particular order.
    pub fn workspace_ids(&self) -> impl Iterator<Item = WorkspaceId> + '_ {
        self.workspaces.keys().copied()
    }

    /// The label of a mirrored workspace, if it has one.
    #[must_use]
    pub fn workspace_label(&self, id: WorkspaceId) -> Option<String> {
        self.workspaces.get(&id).and_then(|ws| ws.label.clone())
    }

    /// Focus `id`. A no-op if it is not a known workspace.
    pub fn focus_workspace(&mut self, id: WorkspaceId) {
        if self.workspaces.contains_key(&id) {
            self.focus_workspace = Some(id);
        }
    }

    /// The pane grid cache for `pane`, creating a blank `rows`x`cols` one if
    /// this is the first the client has heard of it.
    pub fn pane_mut(&mut self, pane: PaneId, rows: u16, cols: u16) -> &mut PaneGrid {
        self.panes
            .entry(pane)
            .or_insert_with(|| PaneGrid::blank(rows, cols))
    }

    /// The cached grid for `pane`, if the client has one.
    #[must_use]
    pub fn pane(&self, pane: PaneId) -> Option<&PaneGrid> {
        self.panes.get(&pane)
    }

    /// Record (or clear) a pane's label, as `session.state` reports it.
    pub fn set_pane_label(&mut self, pane: PaneId, label: Option<String>) {
        match label {
            Some(label) => {
                self.pane_labels.insert(pane, label);
            }
            None => {
                self.pane_labels.remove(&pane);
            }
        }
    }

    /// A pane's label, if the server has given it one.
    #[must_use]
    pub fn pane_label(&self, pane: PaneId) -> Option<&str> {
        self.pane_labels.get(&pane).map(String::as_str)
    }

    /// Record (or clear) a pane's agent status, as `session.state` reports it.
    pub fn set_pane_agent(&mut self, pane: PaneId, agent: Option<AgentSnapshot>) {
        match agent {
            Some(mut agent) => {
                agent.attention = self.queue_position(pane);
                self.agents.insert(pane, agent);
            }
            None => {
                self.agents.remove(&pane);
            }
        }
    }

    /// A pane's agent status, if the server tracks one there.
    #[must_use]
    pub fn pane_agent(&self, pane: PaneId) -> Option<&AgentSnapshot> {
        self.agents.get(&pane)
    }

    /// Fold an `agent_status` event: the pane moved to `state` at `seq`.
    ///
    /// Returns whether anything changed. The sequence is the guard that makes
    /// this safe to replay: `session.state` and the event stream overlap after
    /// a resync, and a transition already reflected in the mirrored status must
    /// not be applied a second time on top of a newer one. An event's bus
    /// sequence *is* the transition's sequence — the hub publishes at the
    /// transition — so comparing the two is comparing like with like.
    pub fn apply_agent_status(
        &mut self,
        pane: PaneId,
        state: AgentState,
        cause: StatusCause,
        seq: Seq,
    ) -> bool {
        let entry = self
            .agents
            .entry(pane)
            .or_insert_with(|| AgentSnapshot::unidentified(seq));
        if entry.transition_seq > seq {
            return false;
        }
        let unchanged = entry.state == state && entry.cause == cause;
        entry.state = state;
        entry.cause = cause;
        entry.transition_seq = seq;
        !unchanged
    }

    /// Fold an `agent_identified` event: the pane is running `kind`.
    ///
    /// Separate from the status because identity and state move independently —
    /// an identified agent changes state many times, and a pane can change
    /// state without ever being identified.
    pub fn apply_agent_identified(&mut self, pane: PaneId, kind: AgentKind, seq: Seq) -> bool {
        let entry = self
            .agents
            .entry(pane)
            .or_insert_with(|| AgentSnapshot::unidentified(seq));
        if entry.kind.as_ref() == Some(&kind) {
            return false;
        }
        entry.kind = Some(kind);
        true
    }

    /// Replace the attention queue wholesale, as `session.state` reports it.
    pub fn set_attention(&mut self, queue: Vec<PaneId>) {
        self.attention = queue;
        self.reindex_attention();
    }

    /// The attention queue, in queue order.
    #[must_use]
    pub fn attention(&self) -> &[PaneId] {
        &self.attention
    }

    /// How many agents are waiting on the user — the status line's `⚑N`.
    #[must_use]
    pub fn attention_count(&self) -> u32 {
        u32::try_from(self.attention.len()).unwrap_or(u32::MAX)
    }

    /// Fold an `attention_enqueued` event, returning whether it was new.
    ///
    /// Idempotent, and it has to be: after a gap the client re-reads state and
    /// then keeps folding deliveries, so an enqueue can arrive for a pane the
    /// fresh snapshot already listed. A queue that grew a duplicate there would
    /// make `⚑N` count the same agent twice.
    pub fn enqueue_attention(&mut self, pane: PaneId) -> bool {
        if self.attention.contains(&pane) {
            return false;
        }
        self.attention.push(pane);
        self.reindex_attention();
        true
    }

    /// Fold an `attention_dequeued` event, returning whether it removed
    /// anything.
    pub fn dequeue_attention(&mut self, pane: PaneId) -> bool {
        let before = self.attention.len();
        self.attention.retain(|&id| id != pane);
        let removed = self.attention.len() != before;
        if removed {
            self.reindex_attention();
        }
        removed
    }

    /// `pane`'s place in the queue, counted from zero.
    fn queue_position(&self, pane: PaneId) -> Option<u32> {
        self.attention
            .iter()
            .position(|&id| id == pane)
            .and_then(|at| u32::try_from(at).ok())
    }

    /// Rewrite every mirrored snapshot's queue position from the queue itself.
    ///
    /// One place holds the truth — the vector — and the per-pane field is
    /// derived from it on every change, so a snapshot can never claim a
    /// position the queue disagrees with.
    fn reindex_attention(&mut self) {
        for (pane, agent) in &mut self.agents {
            agent.attention = self
                .attention
                .iter()
                .position(|id| id == pane)
                .and_then(|at| u32::try_from(at).ok());
        }
    }

    /// Record what the server's startup restore cost, or that it did none.
    pub fn set_restore(&mut self, restore: Option<RestoreSummary>) {
        self.restore = restore;
    }

    /// What the server's startup restore cost, if it performed one.
    #[must_use]
    pub const fn restore(&self) -> Option<RestoreSummary> {
        self.restore
    }

    /// Drop a pane's cached grid, e.g. after `pane.close`.
    pub fn remove_pane(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
        self.pane_labels.remove(&pane);
        self.agents.remove(&pane);
        self.dequeue_attention(pane);
    }

    /// Drop a workspace the server no longer reports, moving focus off it.
    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        self.workspaces.remove(&id);
        if self.focus_workspace == Some(id) {
            self.focus_workspace = self.workspaces.keys().copied().next();
        }
    }

    /// Keep only the workspaces `keep` admits, with the same focus rule as
    /// [`Self::remove_workspace`].
    pub fn retain_workspaces(&mut self, keep: impl Fn(WorkspaceId) -> bool) {
        self.workspaces.retain(|id, _| keep(*id));
        if let Some(focus) = self.focus_workspace
            && !self.workspaces.contains_key(&focus)
        {
            self.focus_workspace = self.workspaces.keys().copied().next();
        }
    }

    /// Keep only the pane grids `keep` admits — labels, agent statuses and
    /// attention entries with them.
    ///
    /// A pane the server no longer reports must leave the attention queue too:
    /// a queue holding a dead pane would send `next-attention` somewhere there
    /// is nothing to look at.
    pub fn retain_panes(&mut self, keep: impl Fn(PaneId) -> bool) {
        self.panes.retain(|id, _| keep(*id));
        self.pane_labels.retain(|id, _| keep(*id));
        self.agents.retain(|id, _| keep(*id));
        self.attention.retain(|id| keep(*id));
        self.reindex_attention();
    }

    /// The content area chrome leaves for panes: the whole terminal minus one
    /// status line at the bottom.
    #[must_use]
    pub fn content_area(&self) -> Rect {
        Rect::new(
            self.term.x,
            self.term.y,
            self.term.w,
            self.term.h.saturating_sub(1),
        )
    }

    /// Every visible pane's rect within `area`, for the focused workspace.
    #[must_use]
    pub fn pane_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        self.focused_workspace()
            .map(|ws| ws.layout.rects(area))
            .unwrap_or_default()
    }
}
