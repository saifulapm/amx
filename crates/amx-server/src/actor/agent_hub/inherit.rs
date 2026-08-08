//! Taking a predecessor's view of this session, across a live upgrade.
//!
//! `docs/09-m3-plan.md` §3 step 9 says "hub seeded" and R-M3-13 says why:
//! agent status is **carried, not re-derived**. A successor that let tier 2
//! rebuild it would show every blocked agent a spurious nothing-then-blocked
//! flap — a standing `wait --until blocked` would see it — and would lose the
//! attention queue's *order*, which is block time and is not recoverable from
//! a screen.
//!
//! Seeding happens in two halves because a tracked pane is two things. The
//! status arrives with the manifest, at assembly time
//! ([`AgentHub::inherit`]); the frame feed arrives later, with the
//! `PaneStarted` `Core` sends once it has the pane host. [`AgentHub::adopt`]
//! marries them, and until it runs the status lives in the view alone — which
//! is the read model a wait uses, so nothing is missing in the meantime.
//!
//! Nothing here publishes. These are not transitions: they are the same
//! statuses the exporter last announced, in a different process, and the swap
//! is invisible on the bus by design (§4).

use amx_core::agent::{AgentSnapshot, CoverageClass};
use amx_core::{PaneId, WorkspaceId};

use super::{AgentHub, IDENTITY_GRACE, Tracked, startup_grace};
use crate::actor::{SnapshotFeed, SpawnedIdentity, StatusUpdate};

/// One pane as a live upgrade hands it over (`docs/09-m3-plan.md` D-M3-5).
///
/// The workspace rides along because the hub learns that pairing from
/// `PaneCreated` events, and an import publishes none — the panes were created
/// on the exporter, and announcing them again would be announcing something
/// that did not happen.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InheritedPane {
    /// The pane.
    pub pane: PaneId,
    /// The workspace holding it, if the manifest's layout still names one.
    pub workspace: Option<WorkspaceId>,
    /// Its label, which is the agent's name (D-M2-9).
    ///
    /// Here for the same reason [`workspace`](Self::workspace) is: the hub
    /// learns labels from `PaneRenamed`, an import publishes none, and a label
    /// that only arrived by rename event would be absent for every pane that
    /// crossed an upgrade — R-M4-4's warning, which is why this is load-bearing
    /// rather than a convenience. The exporter's own layout knows it.
    pub label: Option<String>,
    /// Its workspace's label, on the same terms.
    pub workspace_label: Option<String>,
    /// Its agent status, or `None` for a pane the exporter tracked no agent in.
    pub status: Option<AgentSnapshot>,
}

impl AgentHub {
    /// Take a predecessor's view of this session, across a live upgrade.
    ///
    /// Called on the import assembly's path (`docs/09-m3-plan.md` §3 step 9,
    /// "hub seeded"), before [`AgentHub::run`] and before the session socket is
    /// bound, so the first reader of either read model sees what the exporter's
    /// last reader saw. Nothing is published: the swap is invisible on the bus
    /// (§4), and these are not transitions — they are the same statuses, in a
    /// different process.
    ///
    /// The statuses wait here rather than becoming trackers immediately,
    /// because a tracker needs the pane's frame feed and only `Core` can clone
    /// one; [`AgentHub::track`] marries the two when the `PaneStarted` arrives.
    #[must_use]
    pub fn inherit(mut self, panes: Vec<InheritedPane>, attention: Vec<PaneId>) -> Self {
        self.attention = attention;
        for pane in panes {
            if let Some(workspace) = pane.workspace {
                self.workspaces.insert(pane.pane, workspace);
                if let Some(label) = pane.workspace_label {
                    self.names.workspaces.insert(workspace, label);
                }
            }
            if let Some(label) = pane.label {
                self.names.panes.insert(pane.pane, label);
            }
            let Some(status) = pane.status else { continue };
            // The fast read model, written now: a `wait --until blocked` that
            // reconnects the instant the socket comes back must find the block
            // the exporter told it about, not an empty view it will sit on
            // until the next transition. No events ride with it, so nothing is
            // published — this is the same status, in a different process.
            self.write(StatusUpdate {
                pane: pane.pane,
                status: Some(status.clone()),
                attention: self.attention.clone(),
                events: Vec::new(),
            });
            self.inherited.insert(pane.pane, status);
        }
        self
    }

    /// Start tracking an *inherited* pane, continuing the status that crossed.
    ///
    /// The identity, the state, the cause, the conversation and the transition
    /// sequence are the exporter's; what is new is the feed and the token,
    /// because both belong to the process now holding the pane. Nothing is
    /// published and nothing is written to either read model — [`inherit`] put
    /// the status in the view already and `Core` was seeded with the mirror —
    /// so the only directives that reach [`absorb`](Self::absorb) are the
    /// tracker's deadlines, which it applies without announcing anything.
    ///
    /// [`inherit`]: Self::inherit
    pub(super) fn adopt(
        &mut self,
        pane: PaneId,
        frames: SnapshotFeed,
        spawn: SpawnedIdentity,
        carried: &AgentSnapshot,
    ) {
        let stanza = carried
            .kind
            .as_ref()
            .and_then(|kind| self.registry.resolve(kind));
        let (coverage, grace) = stanza.map_or((CoverageClass::None, IDENTITY_GRACE), |stanza| {
            (stanza.coverage, startup_grace(stanza))
        });
        let mut tracked = Tracked::new(frames, spawn, carried.transition_seq);
        tracked.session_ref = carried.session_ref.clone();
        // The exporter's instant, not this process's. An agent blocked all
        // night reads as blocked all night after an upgrade; re-stamping here
        // would tell the user it had been waiting four seconds, which is the
        // half of R-M4-4 the handoff can answer exactly because the manifest
        // carries `AgentSnapshot`s whole.
        tracked.since = carried.since;
        let directives = tracked.tracker.adopt(carried, coverage, grace);
        self.panes.insert(pane, tracked);
        // Normally a consequence of `Directive::Identified`, which an adoption
        // does not emit — the agent was identified on the exporter, one server
        // ago — and without it the pane would have no screen rules and tier 2
        // would go quiet for the rest of the session.
        if let Some(kind) = carried.kind.clone() {
            self.bind_manifest(pane, &kind);
        }
        self.absorb(pane, &directives);
    }
}
