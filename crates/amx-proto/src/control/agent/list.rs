//! `agent.list`: the one projection all three attention surfaces render.
//!
//! `docs/10-attention-surfaces.md` §D15 has one data source and three
//! consumers — the status line's breakdown, the agents view, and `amx agents`
//! — so this reply is deliberately the *only* place an agent's screen contents
//! reach the wire. `session.state` does not grow a `last_line`
//! (`docs/11-m4-plan.md` D-M4-5): every attached client folds the whole of that
//! reply on every resync, and putting a line of every pane's screen on it means
//! every mirror, golden and `amx session state` dump grows a copy of what is on
//! screen — for a field the expensive reply refreshes at the wrong cadence
//! anyway.
//!
//! # One round trip, whatever the pane count
//!
//! D-M4-2: `Core` already holds every field but two — it walks workspaces with
//! their labels, holds each pane's label, mirrors the hub's per-pane status and
//! the attention queue, and owns the `PaneHost`s whose published snapshot
//! `last_line` is read off. So the whole reply is one mailbox round trip and
//! not a per-pane fan-out, which at 25 agents would have been 25.
//!
//! R-M4-7 states the cost this shape implies and the constraint that follows:
//! one call walks every tracked pane, so a surface that refreshed *per pane* at
//! 4 Hz with 25 agents would issue 100 calls a second each walking 25 panes.
//! Every consumer refreshes once per debounce window for all rows.
//!
//! # Task ownership
//!
//! X02 froze this shape. **X10** answers it from `Core`, **X14** renders the
//! agents view and **X16** `amx agents`.

use amx_core::agent::{AgentKind, AgentState, AgentWorkspace, EpochMillis};
use amx_core::{PaneId, Seq, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Parameters of `agent.list`.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ListParams {
    /// List only this workspace's agents.
    ///
    /// Narrows [`agents`](ListReply::agents) and nothing else: the
    /// [`attention`](ListReply::attention) queue stays global and in its own
    /// order, because a filtered queue would answer a different question than
    /// the one `agent.next` acts on and the two must never disagree.
    ///
    /// By id, for the reason [`NextParams::workspace`](super::NextParams) is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
}

/// One tracked agent, as `agent.list` reports it.
///
/// A pane with no tracked agent is *absent* from the list rather than present
/// as an unknown: the surfaces this feeds answer "which agents need me", and a
/// row for every shell in the session is noise that would have to be filtered
/// three times.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentEntry {
    /// Which project this agent belongs to.
    ///
    /// Present on every row, because D15's whole premise is that the queue is
    /// global and *every display surface groups by workspace*. The label inside
    /// it is what a person reads and may be absent; the id never is.
    pub workspace: AgentWorkspace,
    /// The pane it runs in.
    pub pane: PaneId,
    /// The agent's name, which is the pane's label (D-M2-9).
    ///
    /// Absent for a pane nobody has named — an agent typed into an ordinary
    /// split rather than started by `agent start`, which names the pane as part
    /// of starting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Which agent it is, by registry id, once identity has confirmed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AgentKind>,
    /// What it is doing.
    pub status: AgentState,
    /// What last moved it there, by the detector's own name (D-M4-3).
    ///
    /// The winning manifest rule for a screen-owned state, the hook event for a
    /// hook-asserted entry, and absent for a tier-3 `busy`/`quiet` that no
    /// named rule produced. The same string
    /// [`AgentSnapshot::reason`](amx_core::agent::AgentSnapshot::reason)
    /// carries, not a second spelling of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When it entered [`status`](Self::status), in epoch milliseconds.
    ///
    /// Rendered as an age against [`ListReply::now`] and never against the
    /// reader's own clock — D-M4-4, and see
    /// [`EpochMillis`](amx_core::agent::EpochMillis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<EpochMillis>,
    /// The literal last non-empty visible row of the pane, trimmed.
    ///
    /// Never scrollback, never an interpretation, and never a generated
    /// summary: D15 rejects the last of those explicitly, because a model call
    /// per working session buys a token bill and a staleness window inside a
    /// monitor, and the literal line plus [`reason`](Self::reason) answers the
    /// same question at damage latency for free.
    ///
    /// The empty string for a blank pane rather than absent — a row that
    /// dropped the field for an empty screen would make "this pane has printed
    /// nothing" and "this build does not report screen contents" the same
    /// bytes.
    pub last_line: String,
}

/// Reply to `agent.list`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ListReply {
    /// The bus sequence this reply was captured at (04 §2).
    pub seq: Seq,
    /// The server's own wall clock when it answered.
    ///
    /// The other half of D-M4-4. Every age a surface renders is `now − since`
    /// from inside *this* reply, advanced locally by monotonic elapsed time
    /// between refreshes, so two machines whose clocks differ still agree about
    /// how long an agent has been waiting. Free to carry, and carrying it is
    /// what lets [`AgentEntry::since`] stay absolute for the external consumers
    /// that want an instant rather than an age.
    pub now: EpochMillis,
    /// The attention queue, in queue order, head first.
    ///
    /// The same queue `session.state` reports and `agent.next` focuses the head
    /// of — global and block-time ordered, and **not** narrowed by
    /// [`ListParams::workspace`]. Two surfaces reading two orders is how a
    /// status line and a jump key come to disagree about who has waited
    /// longest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<PaneId>,
    /// Every tracked agent, one row each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentEntry>,
}
