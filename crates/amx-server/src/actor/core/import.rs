//! Import: this session's state from a manifest, and its panes from
//! descriptors that crossed a socket.
//!
//! The mirror of [`super::restore`], and the differences are the whole point.
//! Restore reads a file, respawns a shell behind every pane, replays what
//! scrollback it kept and reports everything it could not rebuild. An import
//! reads a manifest that a live server captured seconds ago, adopts terminals
//! whose children never stopped running, and rebuilds nothing — so there is no
//! loss to report, no cwd to fall back to, and no conversation to type back in.
//!
//! Three rules hold this file together:
//!
//! - **Nothing is published.** 04 §6 and `docs/09-m3-plan.md` §4: the swap is
//!   invisible on the bus, and a client learns that the server changed from
//!   `Welcome.session` rather than from an event. A `PaneCreated` here would be
//!   an announcement of something that did not happen — the pane already
//!   existed, on the exporter, under this id.
//! - **Nothing is spawned.** A pane's process is the one the exporter started;
//!   its terminal arrives as a descriptor and its state as a seed (D-M3-4).
//! - **Nothing is touched until the commit.** Every pane is quiesced before it
//!   is handed to `Core`, and its terminal is behind a closed
//!   [`PtyGate`] until ownership transfers (§3 steps 9–14).
//!
//! What the successor keeps that a cold restore mints fresh is the identity:
//! the `SessionId` ([`Core::with_session_id`]), the bus head
//! ([`Bus::new_at`](amx_core::Bus::new_at)), every pane's row-id space, grid
//! generation and frame counter (the seed's three counters), and the hub's
//! statuses and attention queue (R-M3-13).

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

use amx_core::agent::HookToken;
use amx_core::platform::ProcessId;
use amx_core::{PaneId, WorkspaceId};
use thiserror::Error;
use uuid::Uuid;

use super::Core;
use crate::actor::{
    AgentCommand, PaneHost, PaneHostConfig, PaneHostError, PaneResume, SpawnedIdentity,
};
use crate::handoff::manifest::{Manifest, ManifestError, PaneManifest};
use crate::platform::{InheritedPtySession, PtyGate};
use crate::pty::PtyActorError;

/// A session could not be assembled from what crossed.
#[derive(Debug, Error)]
pub enum AdoptError {
    /// The manifest names as many descriptors as it has panes, and it did not.
    #[error("the manifest carries {panes} panes and {masters} descriptors")]
    Mismatched {
        /// Entries in the manifest.
        panes: usize,
        /// Descriptors that arrived.
        masters: usize,
    },
    /// A pane's carried state could not be turned back into bytes.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// A pane host could not be started over an inherited terminal.
    #[error("pane {pane}: {source}")]
    Pane {
        /// Which pane.
        pane: PaneId,
        /// What the pane host said.
        source: PaneHostError,
    },
    /// A pane could not be held still before the session was handed over.
    ///
    /// Fatal on purpose: a pane that will not quiesce is a pane whose terminal
    /// could move under the exporter while it still owns the session, and §3
    /// has exactly one answer for anything that goes wrong before the commit.
    #[error("pane {pane} would not quiesce: {source}")]
    Quiesce {
        /// Which pane.
        pane: PaneId,
        /// What the pty actor said.
        source: PtyActorError,
    },
}

/// What an adoption produced, for the assembly that has to finish it.
///
/// The panes themselves are `Core`'s from the moment they are adopted, but the
/// *commit* is not: it lands after `Core` is already running, and §3 step 14 is
/// the assembly's to perform. So the two capabilities that outlive the
/// adoption — open the terminal, resume the actor — come back here rather than
/// staying somewhere only a running actor could reach them.
#[derive(Debug, Default)]
pub struct Adopted {
    /// Every inherited pane's gate and its resume, in transfer order.
    ///
    /// Used exactly once, by [`Adopted::take_over`].
    pub resume: Vec<(PtyGate, PaneResume)>,
    /// Every pane that came back, with the workspace it belongs to.
    ///
    /// The pairing `AgentHub` would otherwise learn from the `PaneCreated`
    /// events this path deliberately does not publish.
    pub panes: Vec<(PaneId, Option<WorkspaceId>)>,
    /// Panes the manifest's layout named and no descriptor arrived for.
    ///
    /// Pruned from the tree rather than left as slots with nothing behind
    /// them, and counted here so the assembly can say so.
    pub pruned: Vec<PaneId>,
    /// The attention queue, in the block order it crossed in (R-M3-13).
    pub attention: Vec<PaneId>,
}

impl Adopted {
    /// §3 step 14: ownership transferred, so let every pane read and write.
    ///
    /// The gates open before the actors resume, in that order, because a
    /// resumed actor whose gate is still closed would poll a terminal it is
    /// still refused.
    ///
    /// A pane that will not resume is returned rather than fatal: the commit
    /// has landed, there is nothing left to abort back to, and one stuck
    /// terminal must not cost a session that is otherwise serving.
    ///
    /// Blocks per pane, on the round trip to each pty actor's thread.
    #[must_use]
    pub fn take_over(&self) -> Vec<(PaneId, PtyActorError)> {
        for (gate, _) in &self.resume {
            gate.open();
        }
        self.resume
            .iter()
            .filter_map(|(_, pane)| pane.resume().err().map(|err| (pane.pane(), err)))
            .collect()
    }
}

impl Core {
    /// Adopt a manifest and the descriptors that came with it.
    ///
    /// Must be called from inside a tokio runtime — it starts pane actors —
    /// and before [`Core::run`], like [`Core::restore`]. It blocks briefly per
    /// pane, on the quiesce: nothing is serving yet, and holding the terminal
    /// still is what the next stage of §3 promises the exporter.
    ///
    /// # Errors
    ///
    /// [`AdoptError`] for a manifest whose descriptors do not pair with its
    /// entries, a pane whose seed will not decode, a pane host that cannot
    /// start, or a pane that will not quiesce. Every one of them is a
    /// pre-commit failure, and §3 has one answer for those: abort.
    pub fn adopt_manifest(
        &mut self,
        manifest: &Manifest,
        masters: Vec<OwnedFd>,
    ) -> Result<Adopted, AdoptError> {
        if manifest.panes.len() != masters.len() {
            return Err(AdoptError::Mismatched {
                panes: manifest.panes.len(),
                masters: masters.len(),
            });
        }
        self.adopt_state(manifest);
        let mut adopted = Adopted::default();
        for (entry, master) in manifest.panes.iter().zip(masters) {
            adopted.resume.push(self.adopt_pane(entry, master)?);
            adopted
                .panes
                .push((entry.pane, self.workspace_of(entry.pane)));
        }
        adopted.pruned = self.prune_paneless();
        self.adopt_agents(manifest);
        adopted.attention.clone_from(&self.attention);
        Ok(adopted)
    }

    /// Put the workspaces, the layouts, the labels and the numbers back.
    ///
    /// The same snapshot a cold restore would have read off disk, captured in
    /// memory instead (D-M3-5) — so this is [`Core::restore`]'s bookkeeping
    /// with every rebuild taken out of it, and with no report, because nothing
    /// here can degrade: the panes are not being recreated, they are being
    /// handed over.
    fn adopt_state(&mut self, manifest: &Manifest) {
        for saved in &manifest.state.workspaces {
            if self
                .state
                .adopt_workspace(
                    saved.id,
                    saved.label.clone(),
                    saved.layout.clone(),
                    saved.focus,
                )
                .is_err()
            {
                // Two workspaces claiming one pane, or a focus outside its own
                // layout: the exporter built this from a tree that held
                // neither, so it means the manifest was rewritten in flight.
                tracing::warn!(workspace = %saved.id, "the manifest's layout does not fit the tree");
                continue;
            }
            if let Some(worktree) = saved.worktree.clone() {
                let _ = self.state.set_worktree(saved.id, Some(worktree));
            }
            self.workspace_shorts.insert(saved.id, saved.short);
            self.next_workspace_short = self
                .next_workspace_short
                .max(saved.short.get().saturating_add(1));
        }
        for saved in &manifest.state.panes {
            if self.state.pane(saved.id).is_none() {
                continue;
            }
            self.pane_shorts.insert(saved.id, saved.short);
            self.next_pane_short = self
                .next_pane_short
                .max(saved.short.get().saturating_add(1));
            if let Some(cwd) = saved.cwd.clone() {
                let _ = self.state.set_pane_cwd(saved.id, cwd);
            }
            if let Some(label) = saved.label.clone() {
                let _ = self.state.rename_pane(saved.id, Some(label));
            }
            // The argv the exporter recorded, kept so `session.state` and the
            // successor's own captures answer what they answered before. The
            // token beside it is *this* server's: the one the child's
            // environment carries did not cross the manifest, which is a
            // known gap, not a choice — see the module's hand-off note in
            // `docs/notes/m3-wave-outcomes.md`.
            if let Some(argv) = saved.argv.clone() {
                let _ = self.state.set_pane_spawn(saved.id, argv, mint_token());
            }
        }
        if let Some(workspace) = manifest.state.focused_workspace {
            let _ = self.state.switch_workspace(workspace);
        }
    }

    /// Take over one pane: its terminal, its screen, and its three counters.
    fn adopt_pane(
        &mut self,
        entry: &PaneManifest,
        master: OwnedFd,
    ) -> Result<(PtyGate, PaneResume), AdoptError> {
        let pane = entry.pane;
        let seed = entry.seed()?;
        let gate = PtyGate::closed();
        let session = InheritedPtySession::new(master, ProcessId(entry.child), gate.clone());
        let mut config = PaneHostConfig::new(pane, self.ctx.bus.clone(), entry.size());
        config.core = Some(self.handle.clone());
        config.cancel = self.ctx.cancel.child_token();
        config.seed = Some(seed);
        let host =
            PaneHost::spawn(config, session).map_err(|source| AdoptError::Pane { pane, source })?;
        // Quiesced before `Core` can be asked to do anything with it, and
        // before the gate is ever opened: the pane's terminal is the
        // exporter's until the commit, and both halves of that say so.
        host.quiesce()
            .map_err(|source| AdoptError::Quiesce { pane, source })?;
        self.pane_sizes.insert(pane, (entry.rows, entry.cols));
        // The window `session.state` answers from, which is folded from pane
        // reports in ordinary operation and has none to fold here. The floor is
        // the oldest row that crossed rather than the exporter's own — the
        // successor cannot serve what it did not receive — and the parser
        // derives its authoritative one from what actually landed in the
        // scrollback (W04), which is never older than this.
        self.history
            .insert(pane, (entry.history.head, entry.history.first));
        let resume = host.resumer();
        self.panes.insert(pane, host);
        Ok((gate, resume))
    }

    /// Take the hub's view of every pane, in the order it was in.
    ///
    /// R-M3-13: carried, never re-derived. A successor that let tier 2 rebuild
    /// these within a hundred milliseconds would show a wait standing on
    /// `blocked` a spurious nothing-then-blocked flap, and would lose the
    /// attention queue's *order*, which is block time and is not recoverable
    /// from a screen.
    fn adopt_agents(&mut self, manifest: &Manifest) {
        let mut ranked = BTreeMap::new();
        let mut unranked = Vec::new();
        for entry in &manifest.agents {
            if self.state.pane(entry.pane).is_none() {
                continue;
            }
            // The queue's order is the entry's own `attention` rank, which is
            // what the hub fills in on a status before it mirrors it (the
            // position is derived on read and stored nowhere else). A pane that
            // wants attention and carries no rank is queued behind everything
            // that does: its place is unknowable, and leaving a blocked agent
            // off the queue entirely would lose it from `agent.next`.
            match entry.agent.attention {
                Some(rank) => {
                    ranked.insert((rank, entry.pane), entry.pane);
                }
                None if entry.agent.state.wants_attention() => unranked.push(entry.pane),
                None => {}
            }
            self.agent_status.insert(entry.pane, entry.agent.clone());
        }
        self.attention = ranked.into_values().chain(unranked).collect();
    }

    /// Take out every pane the layout named and no terminal arrived for.
    ///
    /// Cannot happen between two amx builds that agree about the manifest —
    /// the exporter captures the panes it is listing — so this is the shape a
    /// disagreement would take rather than a case with a story. A slot with
    /// nothing behind it is worse than a missing slot, which is the same
    /// judgement D-M1-9 makes about a pane whose shell will not start.
    fn prune_paneless(&mut self) -> Vec<PaneId> {
        let orphans: Vec<(WorkspaceId, PaneId)> = self
            .state
            .workspaces()
            .flat_map(|ws| {
                let id = ws.id();
                ws.layout().panes().into_iter().map(move |pane| (id, pane))
            })
            .filter(|(_, pane)| !self.panes.contains_key(pane))
            .collect();
        let mut pruned = Vec::new();
        for (workspace, pane) in orphans {
            tracing::warn!(%pane, "the manifest's layout names a pane no descriptor arrived for");
            if self.state.close(workspace, pane).is_ok() {
                pruned.push(pane);
            }
        }
        let empty: Vec<WorkspaceId> = self
            .state
            .workspaces()
            .filter(|ws| ws.layout().panes().is_empty())
            .map(|ws| ws.id())
            .collect();
        for workspace in empty {
            let _ = self.state.kill_workspace(workspace);
            self.workspace_shorts.remove(&workspace);
        }
        pruned
    }

    /// Hand `AgentHub` every inherited pane, once it is listening.
    ///
    /// The same `PaneStarted` a spawn sends, with the same contents: `Core`
    /// owns the hosts and is the only place a feed can be cloned from
    /// (`docs/08-m2-plan.md` §3). It is awaited rather than `try_send`, which a
    /// live `Core` may never do — the hub reads `Core`'s mirror through this
    /// same actor and parking on it would be half a deadlock — because this
    /// runs before [`Core::run`] does: there is no mailbox to starve yet, and a
    /// session of two hundred panes must not lose its two hundred and first to
    /// a full queue.
    pub async fn announce_inherited(&self) {
        let Some(agent) = self.agent.clone() else {
            return;
        };
        for (pane, host) in &self.panes {
            let state = self.state.pane(*pane);
            let spawn = SpawnedIdentity {
                argv: state
                    .and_then(|pane| pane.argv().map(<[String]>::to_vec))
                    .unwrap_or_default(),
                token: state
                    .and_then(|pane| pane.hook_token().cloned())
                    .unwrap_or_else(mint_token),
                // Identity is not being asked for: it crossed with the status,
                // and the hub adopts it (R-M3-13). A `requested` here would
                // send the pane through the ordinary identification path and
                // publish the transition this whole file exists not to.
                requested: None,
            };
            let _ = agent
                .send(AgentCommand::PaneStarted {
                    pane: *pane,
                    frames: host.frames(),
                    spawn,
                })
                .await;
        }
    }
}

/// A fresh hook token for an inherited pane.
///
/// The same 122 random bits [`spawn`](super::spawn) mints, and — until the
/// token itself crosses the manifest — deliberately *not* the one the child's
/// environment carries. The consequence is stated where it is felt: hook
/// reports from an inherited agent are dropped by the misattribution guard
/// (D-M2-4) until it restarts, and its status rides tier 2 alone.
fn mint_token() -> HookToken {
    HookToken::new(Uuid::new_v4().simple().to_string())
}
