//! What the board shows, in what order, and what it hides.
//!
//! Three rules, all of them D15's, and all of them display-only — the attention
//! queue itself stays global and block-time ordered on the server, because
//! fairness is the server's job and orientation is the eye's:
//!
//! - **Ordering within a group** is blocked (oldest first) → working → idle, so
//!   the top row of a group is whoever needs the user most. One comparison does
//!   all of it: band, then entry edge, then name. Applying "oldest first" to
//!   every band rather than only to `blocked` costs nothing and answers the same
//!   question one row down — who has been at this longest.
//! - **Groups are ordered by their most urgent member**, which is what keeps
//!   *"the top row is always who needs me most"* true under workspace grouping
//!   as well as state grouping. A project with a blocked agent sorts above one
//!   with only working agents; ties break on the name so the board does not
//!   reshuffle between two repaints of the same session.
//! - **More than [`IDLE_COLLAPSE`] idle agents in a group become one row**,
//!   which `Enter` expands. Blocked and working rows are never collapsed — they
//!   are the rows the surface exists for.
//!
//! # Collapse yields to a query
//!
//! An idle run is only collapsed while the filter is empty. Typing is already a
//! way of asking for fewer rows, and a board that answered a search by hiding
//! the match behind `12 idle` would be answering a different question than the
//! one that was asked. `ctrl+b` needs no such rule: a blocked-only list has no
//! idle rows to collapse.

use amx_core::agent::{AgentState, EpochMillis};
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::agent::ListReply;

use super::{AgentsUi, Cursor, band};
use crate::app::status::push_name;
use crate::picker::Picker;

/// How many idle agents a group may show before they become one row.
///
/// D15: "more than 3 idle agents in a group collapse to an `N idle` row".
const IDLE_COLLAPSE: usize = 3;

/// Which of D15's two groupings the board is laid out by (`ctrl+s`).
///
/// Two and only two: the chord is a toggle, adopted from the prior art D15
/// audited for the muscle memory, and a third grouping would need a name on
/// screen that a toggle cannot give it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Grouping {
    /// By project — the default, and D15's premise: the queue is global, every
    /// display surface groups by workspace.
    #[default]
    Workspace,
    /// By what the agents are doing.
    State,
}

impl Grouping {
    /// The other one.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Workspace => Self::State,
            Self::State => Self::Workspace,
        }
    }

    /// How the header names it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "by workspace",
            Self::State => "by state",
        }
    }
}

/// One agent's status, as the three bands the ordering and the collapse are
/// written in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Band {
    /// Waiting on the user. The whole reason the board exists.
    Blocked,
    /// Doing something.
    Working,
    /// Doing nothing.
    Idle,
}

impl Band {
    /// The glyph a row is marked with.
    pub(super) const fn mark(self) -> char {
        match self {
            Self::Blocked => '⚑',
            Self::Working => '●',
            Self::Idle => '·',
        }
    }
}

/// A group's identity, stable across a rebuild so an expansion survives one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum GroupKey {
    /// One project.
    Workspace(WorkspaceId),
    /// One band, under [`Grouping::State`].
    Band(Band),
}

/// One agent, as the view holds it between refreshes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Row {
    /// The pane it runs in — the identity everything here is keyed by.
    pub(super) pane: PaneId,
    /// The project it belongs to.
    pub(super) workspace: WorkspaceId,
    /// The agent's own name, which is the pane's label (D-M2-9), or a prefix of
    /// the pane id when nobody has named it.
    pub(super) name: String,
    /// `workspace/name` — what is drawn, and the only thing the filter matches.
    pub(super) label: String,
    /// What it is doing.
    pub(super) state: AgentState,
    /// What last moved it there, by the detector's own name (D-M4-3).
    ///
    /// Rendered verbatim and never translated: the string is `permission_dialog`
    /// from a manifest rule or `PermissionRequest` from a hook event, both are
    /// shipped detector identifiers, and X00's wave-2 note records that the hook
    /// name is what the wire carries in the common case. A renderer that
    /// switched on a known set would print nothing for the next rule anybody
    /// writes.
    pub(super) reason: Option<String>,
    /// When it entered [`Self::state`], on the server's wall clock.
    pub(super) since: Option<EpochMillis>,
    /// The literal last non-empty row of its screen.
    pub(super) last_line: String,
}

impl Row {
    /// Which band this row sorts into.
    pub(super) const fn band(&self) -> Band {
        band(self.state)
    }

    /// What the rows of one group are ordered by.
    fn order(&self) -> (Band, EpochMillis, &str) {
        // A row with no entry edge sorts last within its band rather than first:
        // an absent `since` means the status was re-derived rather than observed
        // entering (D-M4-4), and treating "unknown" as "longest waiting" would
        // put a restored pane above an agent that really has been waiting.
        (
            self.band(),
            self.since.unwrap_or(EpochMillis::MAX),
            &self.label,
        )
    }
}

/// One line of the board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Line {
    /// One agent, by its index into [`AgentsUi::rows`].
    Agent(usize),
    /// A group's collapsed idle run.
    Collapsed {
        /// Which group it belongs to.
        group: GroupKey,
        /// How many rows it stands for.
        count: usize,
    },
}

impl Line {
    /// What selecting this line records.
    pub(super) fn cursor(&self, rows: &[Row]) -> Option<Cursor> {
        match *self {
            Self::Agent(at) => rows.get(at).map(|row| Cursor::Agent(row.pane)),
            Self::Collapsed { group, .. } => Some(Cursor::Collapsed(group)),
        }
    }
}

impl AgentsUi {
    /// Take one `agent.list` reply as the whole of what the board shows.
    ///
    /// The picker is rebuilt only when the *labels* changed, which is what makes
    /// a 4 Hz refresh free: an agent whose screen moved replaces one string in
    /// the second column and disturbs neither the query nor the match set.
    pub(super) fn absorb_reply(&mut self, reply: ListReply) {
        let rows: Vec<Row> = reply.agents.into_iter().map(row_of).collect();
        let labels: Vec<String> = rows.iter().map(|row| row.label.clone()).collect();
        if labels.len() != self.rows.len()
            || labels
                .iter()
                .zip(self.rows.iter())
                .any(|(label, was)| label != &was.label)
        {
            let query = self.picker.query().to_owned();
            self.picker = Picker::new(labels);
            for byte in query.bytes() {
                self.picker.key(byte);
            }
        }
        self.details.clear();
        self.details
            .extend(rows.iter().map(|row| row.last_line.clone()));
        self.picker.set_details(std::mem::take(&mut self.details));
        self.rows = rows;
        self.rebuild();
    }

    /// Recompute what is on screen from the rows, the query and the two toggles.
    pub(super) fn rebuild(&mut self) {
        let mut groups: Vec<(GroupKey, Vec<usize>)> = Vec::new();
        for &at in self.picker.matches() {
            let Some(row) = self.rows.get(at) else {
                continue;
            };
            if self.blocked_only && row.band() != Band::Blocked {
                continue;
            }
            let key = match self.grouping {
                Grouping::Workspace => GroupKey::Workspace(row.workspace),
                Grouping::State => GroupKey::Band(row.band()),
            };
            match groups.iter_mut().find(|(held, _)| *held == key) {
                Some((_, members)) => members.push(at),
                None => groups.push((key, vec![at])),
            }
        }
        for (_, members) in &mut groups {
            members.sort_by(|&a, &b| self.rows[a].order().cmp(&self.rows[b].order()));
        }
        // The group's own key is its most urgent member's, which is the first
        // one now that the members are sorted.
        groups.sort_by(|(_, a), (_, b)| {
            let (first, second) = (self.rows[a[0]].order(), self.rows[b[0]].order());
            (first.0, first.1, group_name(&self.rows[a[0]].label)).cmp(&(
                second.0,
                second.1,
                group_name(&self.rows[b[0]].label),
            ))
        });

        self.visible.clear();
        for (group, members) in &groups {
            let idle_from = members
                .iter()
                .position(|&at| self.rows[at].band() == Band::Idle)
                .unwrap_or(members.len());
            let (shown, idle) = members.split_at(idle_from);
            self.visible.extend(shown.iter().map(|&at| Line::Agent(at)));
            if idle.len() > IDLE_COLLAPSE
                && self.picker.query().is_empty()
                && !self.expanded.contains(group)
            {
                self.visible.push(Line::Collapsed {
                    group: *group,
                    count: idle.len(),
                });
                continue;
            }
            self.visible.extend(idle.iter().map(|&at| Line::Agent(at)));
        }
        self.settle_cursor();
    }

    /// Put the cursor back on what it was pointing at.
    ///
    /// By identity first — a pane that moved from row 3 to row 1 because it
    /// blocked is still the row the user was on. A selection whose agent has
    /// gone keeps its *place* rather than jumping to the top, because a list
    /// that reset itself every time an agent finished would be unusable at the
    /// cadence this one refreshes at.
    fn settle_cursor(&mut self) {
        if self.visible.is_empty() {
            self.at = 0;
            self.cursor = None;
            return;
        }
        let found = self.cursor.and_then(|cursor| {
            self.visible
                .iter()
                .position(|line| line.cursor(&self.rows) == Some(cursor))
        });
        self.at = found.unwrap_or_else(|| self.at.min(self.visible.len() - 1));
        self.cursor = self.visible[self.at].cursor(&self.rows);
    }
}

/// One reply row, as the board holds it.
fn row_of(entry: amx_proto::control::agent::AgentEntry) -> Row {
    let mut name = String::new();
    push_name(&mut name, entry.name.as_deref(), entry.pane);
    let mut label = String::new();
    push_name(
        &mut label,
        entry.workspace.name.as_deref(),
        entry.workspace.id,
    );
    label.push('/');
    label.push_str(&name);
    Row {
        pane: entry.pane,
        workspace: entry.workspace.id,
        name,
        label,
        state: entry.status,
        reason: entry.reason,
        since: entry.since,
        last_line: entry.last_line,
    }
}

/// The workspace half of a `workspace/name` label, for ordering groups whose
/// most urgent members are equally urgent.
fn group_name(label: &str) -> &str {
    label.split('/').next().unwrap_or(label)
}
