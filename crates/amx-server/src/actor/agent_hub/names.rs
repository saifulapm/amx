//! The labels the hub puts on an attention event, and where it got them.
//!
//! D-M4-6's names mirror, and the block assembled from it. `AgentHub` is the
//! only publisher of `attention_enqueued` and `attention_dequeued` (04 §2's
//! one-publisher rule, per event kind), and D15 requires those events to carry
//! enough for a notifier to say `api/backend blocked (permission_dialog)`
//! without a follow-up query — but the hub holds an argv and a terminal title,
//! and nothing a person named.
//!
//! Split from [`super`] rather than left there because it is one
//! responsibility with one rule, and the rule is worth reading on its own: the
//! hub never asks a sibling for a name. Everything here is either folded off
//! the bus it already reads or handed over at assembly by an import that
//! publishes nothing.

use std::collections::HashMap;

use amx_core::agent::{AgentWorkspace, EpochMillis};
use amx_core::{PaneId, WorkspaceId};

use super::AgentHub;

/// Pane and workspace labels, as this hub last saw them.
///
/// It does not *ask* for them. Parking on a sibling is what the hub's shutdown
/// discipline forbids ([`AgentHub`]'s module docs), and it does not have to:
/// every label in a live session reaches this actor on the bus it already
/// reads. `workspace.create` publishes `WorkspaceRenamed` for a label given at
/// creation (`actor/core/workspace.rs:80`), `pane.rename` and
/// `workspace.rename` publish one each, and a **cold restore replays both** for
/// every label it brings back (`actor/core/restore.rs:284,295`) — which is what
/// R-M4-4 warns is load-bearing, checked rather than assumed.
///
/// The one path that publishes nothing at all is the live handoff, by design
/// ("the swap is invisible on the bus", `docs/09-m3-plan.md` §4), so an import
/// seeds this map explicitly through
/// [`InheritedPane`](super::inherit::InheritedPane) instead.
///
/// What it cannot promise: a rename lost inside a bus [`Gap`] leaves the old
/// label here until the next one. The identity block is optional for exactly
/// this class of reason, and a stale label on a notification is the cost of not
/// asking a draining sibling for one.
///
/// [`Gap`]: amx_core::Delivery::Gap
#[derive(Debug, Default)]
pub(super) struct Names {
    /// Pane labels, which are the agent names D-M2-9 defines.
    panes: HashMap<PaneId, String>,
    /// Workspace labels.
    workspaces: HashMap<WorkspaceId, String>,
}

impl Names {
    /// Record `pane`'s label.
    pub(super) fn name_pane(&mut self, pane: PaneId, label: String) {
        self.panes.insert(pane, label);
    }

    /// Record `workspace`'s label.
    pub(super) fn name_workspace(&mut self, workspace: WorkspaceId, label: String) {
        self.workspaces.insert(workspace, label);
    }

    /// Drop a pane that has gone.
    ///
    /// Called by `retire` *after* the dequeue that pane's exit publishes, so
    /// the last event about a pane still names it the way a person would.
    pub(super) fn forget_pane(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// Drop a workspace that has gone.
    pub(super) fn forget_workspace(&mut self, workspace: WorkspaceId) {
        self.workspaces.remove(&workspace);
    }
}

/// One pane, said the way a notifier would say it.
///
/// The four optional fields `Event::AttentionEnqueued` and
/// `AttentionDequeued` grew in X02, assembled in one place so the two arms that
/// build those events cannot disagree about what "the identity block" means.
pub(super) struct Identity {
    /// The workspace and its label.
    pub workspace: Option<AgentWorkspace>,
    /// The pane's label — the agent's name (D-M2-9).
    pub name: Option<String>,
    /// What named the pane's current state, if a detector did.
    pub reason: Option<String>,
    /// When it entered that state.
    pub since: Option<EpochMillis>,
}

impl AgentHub {
    /// The identity block D15 puts on an attention event.
    ///
    /// Every field is what this hub already holds, and every one of them is
    /// optional because a fact it has not been told is *absent* — never
    /// invented and never asked of a sibling (D-M4-6).
    ///
    /// `reason` and `since` describe the pane's state **as it is now**, which
    /// is the state the event is announcing: on an enqueue that is what blocked
    /// it, and on a dequeue it is what ended the block. Read after the
    /// directives are folded, so the tracker has already moved.
    pub(super) fn identity(&self, pane: PaneId) -> Identity {
        let workspace = self.workspaces.get(&pane).map(|id| AgentWorkspace {
            id: *id,
            name: self.names.workspaces.get(id).cloned(),
        });
        let tracked = self.panes.get(&pane);
        Identity {
            workspace,
            name: self.names.panes.get(&pane).cloned(),
            reason: tracked.and_then(|tracked| tracked.tracker.reason.clone()),
            since: tracked.and_then(|tracked| tracked.since),
        }
    }
}
