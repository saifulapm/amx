//! Restore: rebuilding a session from its snapshot, pruning what will not
//! come back, and reporting every prune.
//!
//! Not part of the `Persist` actor (`docs/07-m1-plan.md` §2): restore runs
//! once, on the serve path, between the gateway bind and the accept loop
//! (D-M1-9), so the earliest possible client already sees the restored
//! session. `persist::snapshot::load` reads and validates the file; everything
//! here applies it.
//!
//! Everything comes back under the identity it had. 04 §6 makes the UUIDs
//! load-bearing — snapshot, scrollback sidecar and short number all join on
//! them — so restore adopts ids rather than minting them, and short numbers
//! are restored because the same section requires them "stable across
//! restarts".
//!
//! What cannot be rebuilt is pruned and *reported*, per D-M1-9's table:
//!
//! | Failure | Action | Reported as |
//! |---|---|---|
//! | saved cwd no longer exists | respawn in the home directory | degraded (pane) |
//! | shell spawn fails | prune the pane | lost (pane) |
//! | workspace loses every pane | prune the workspace | lost (workspace) |
//! | sidecar unreadable or torn | skip the replay, keep the pane | degraded (pane) |
//! | snapshot unreadable or newer than the window | start fresh | lost (session) |
//! | a captured conversation cannot be resumed | restore a plain shell | degraded (pane) |
//! | a workspace's git worktree is gone | keep it as a plain workspace | degraded (workspace) |
//!
//! Not one of those is a log line. 04 §6 requires restore loss to reach the
//! status line and `amx session report` — "never log-only" — which is herdr's
//! W8 in one sentence, and the reason the failure paths here end in a
//! [`RestoreLoss`] rather than a `warn!`.
//!
//! The second-to-last row of that table is M2's, and it lives next door:
//! [`super::resume`] plans each pane's conversation, claims it, and types it
//! back into the shell this file respawns (V15, D-M2-7). The last row is M3's
//! and lives here, in [`Core::restore_worktree`]: `amx work` is the only thing
//! that puts a git worktree under a workspace, and a checkout somebody removed
//! between two runs of the server is a loss the user can only see if restore
//! says so.

use std::collections::HashMap;
use std::path::PathBuf;

use amx_core::{Event, PaneId, ShortNumber, WorkspaceId};
use amx_proto::control::session::{
    RestoreEntity, RestoreLoss, RestoreReport, RestoreSeverity, RestoreSummary,
};

use super::Core;
use super::persist::Restored;
use super::resume::{Restoring, load_registry};
use crate::actor::{PaneCommand, PaneHandle};
use crate::agent::resume::Reservations;
use crate::persist::sidecar::SidecarRow;
use crate::persist::{PaneSnapshot, PersistError, Snapshot, WorkspaceSnapshot, sidecar, snapshot};

/// What a restore needs that the snapshot does not carry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RestoreOptions {
    /// Where a pane whose saved directory has vanished respawns (D-M1-9).
    ///
    /// Passed rather than read from `$HOME` here for the reason 04 §2 gives
    /// for `Ctx` itself — "no env-var globals, no test mutexes" — so two
    /// restores in one process can degrade into two different directories.
    /// The server binary fills it from the environment on the serve path.
    pub home: PathBuf,
}

impl Core {
    /// Load this session's snapshot and apply it, if there is one.
    ///
    /// `None` means there was nothing to restore — no file, or a file
    /// describing an empty session — and the caller falls through to the
    /// ordinary first-attach seed. A snapshot that cannot be read at all is
    /// *not* that case: it is a whole-session loss, reported through
    /// [`Core::restore_failed`], because "your session is gone" and "you had no
    /// session" are different sentences.
    ///
    /// Reads the file synchronously. This runs once, at startup, on the same
    /// path that already blocks to bind a socket.
    pub fn restore_from_disk(&mut self, opts: &RestoreOptions) -> Option<RestoreSummary> {
        match snapshot::load(&self.ctx.state_dir) {
            Ok(None) => None,
            Ok(Some(snapshot)) if snapshot.is_empty() => None,
            Ok(Some(snapshot)) => Some(self.restore(snapshot, opts)),
            Err(err) => Some(self.restore_failed(&err)),
        }
    }

    /// Rebuild the session `snapshot` describes, pruning and reporting what
    /// cannot be rebuilt.
    ///
    /// Everything comes back under the ids it had: the workspaces, the panes,
    /// the layouts, the labels and the short numbers (04 §6 — "stable across
    /// restarts"). Shells are respawned in their saved directories through the
    /// same `spawn_pane` a split uses, and saved scrollback is replayed into
    /// the fresh terminal before the child's first byte arrives.
    ///
    /// Must be called from inside a tokio runtime — it starts pane actors —
    /// and before [`Core::run`], which is where the serve path puts it.
    pub fn restore(&mut self, snapshot: Snapshot, opts: &RestoreOptions) -> RestoreSummary {
        let mut run = Restoring {
            opts,
            // Read here rather than borrowed from `AgentHub`: restore runs on
            // an owned `Core` before any actor is serving, so there is nobody
            // to ask (D-M1-9). The two loads are the same builtin plus the same
            // override file, and collapsing them into one owner is a seam V17
            // holds — noted rather than papered over.
            registry: load_registry(&self.ctx.config_path),
            reservations: Reservations::new(),
            resumed: HashMap::new(),
            report: RestoreReport::default(),
        };
        let saved: HashMap<PaneId, PaneSnapshot> = snapshot
            .panes
            .into_iter()
            .map(|pane| (pane.id, pane))
            .collect();

        // The numbers the workspaces were saved under, kept for the pass at
        // the end: the snapshot rows themselves are consumed by the rebuild.
        let recorded: Vec<(WorkspaceId, ShortNumber)> = snapshot
            .workspaces
            .iter()
            .map(|ws| (ws.id, ws.short))
            .collect();

        let (mut workspaces, mut panes) = (0_u32, 0_u32);
        for ws in snapshot.workspaces {
            if let Some(restored) = self.restore_workspace(&ws, &saved, &mut run) {
                workspaces += 1;
                panes += restored;
            }
        }
        self.restore_shorts(&recorded, &saved);
        self.focus_restored(snapshot.focused_workspace);

        let Restoring {
            report, resumed, ..
        } = run;
        let entities = workspaces + panes;
        let summary = report.summary(entities);
        self.restore = Some(Restored {
            report,
            entities,
            resumed,
        });
        self.publish(Event::SessionRestored {
            workspaces,
            panes,
            lost: summary.lost,
            degraded: summary.degraded,
        });
        summary
    }

    /// The snapshot itself could not be read: the session starts fresh, and
    /// the user is told why by `session.report` rather than by a log line.
    pub fn restore_failed(&mut self, err: &PersistError) -> RestoreSummary {
        let report = RestoreReport {
            entries: vec![RestoreLoss {
                severity: RestoreSeverity::Lost,
                entity: RestoreEntity::Session,
                workspace: None,
                pane: None,
                label: None,
                path: Some(snapshot::path(&self.ctx.state_dir)),
                reason: err.to_string(),
            }],
        };
        let summary = report.summary(0);
        self.restore = Some(Restored {
            report,
            entities: 0,
            resumed: HashMap::new(),
        });
        self.publish(Event::SessionRestored {
            workspaces: 0,
            panes: 0,
            lost: summary.lost,
            degraded: summary.degraded,
        });
        summary
    }

    /// Put the short numbers back, once the tree is final.
    ///
    /// One pass, after the rebuild rather than during it, and in one order:
    /// **every recorded number first, then the leftovers.** 04 §6 wants the
    /// numbers "stable across restarts", and assignment is lowest-free — so a
    /// pane the snapshot has no row for, handed a number while the file still
    /// had rows to read, would take one a later workspace's pane is about to
    /// claim back. Two panes would then hold one number and the second would
    /// be unaddressable.
    ///
    /// Nothing pruned on the way in is here to ask for a number, which is why
    /// this needs no release afterwards: what did not come back never took a
    /// number to give up.
    fn restore_shorts(
        &mut self,
        recorded: &[(WorkspaceId, ShortNumber)],
        saved: &HashMap<PaneId, PaneSnapshot>,
    ) {
        for (id, short) in recorded {
            if self.state.workspace(*id).is_some() {
                self.workspace_shorts.adopt(*id, *short);
            }
        }
        for pane in saved.values() {
            if self.state.pane(pane.id).is_some() {
                self.pane_shorts.adopt(pane.id, pane.short);
            }
        }
        // The leftovers: a layout naming a pane the snapshot has no row for
        // (already reported as degraded above) and, in principle, a workspace
        // the same is true of. `assign` is idempotent, so this walk is only
        // about the objects the two loops above did not reach.
        let live: Vec<(WorkspaceId, Vec<PaneId>)> = self
            .state
            .workspaces()
            .map(|ws| (ws.id(), ws.layout().panes()))
            .collect();
        for (ws, panes) in live {
            self.workspace_shorts.assign(ws);
            for pane in panes {
                self.pane_shorts.assign(pane);
            }
        }
    }

    /// Restore one workspace; the answer is how many panes came back with it,
    /// or `None` if the workspace itself was pruned.
    fn restore_workspace(
        &mut self,
        saved_ws: &WorkspaceSnapshot,
        saved: &HashMap<PaneId, PaneSnapshot>,
        run: &mut Restoring<'_>,
    ) -> Option<u32> {
        let id = saved_ws.id;
        let members = saved_ws.layout.panes();
        match self.state.adopt_workspace(
            id,
            saved_ws.label.clone(),
            saved_ws.layout.clone(),
            saved_ws.focus,
        ) {
            Ok(effect) => self.effects.absorb(effect),
            Err(err) => {
                run.report
                    .entries
                    .push(lost_workspace(saved_ws, err.to_string()));
                return None;
            }
        }
        let mut restored = Vec::with_capacity(members.len());
        for pane in members {
            if self.restore_pane(id, pane, saved.get(&pane), run) {
                restored.push(pane);
            }
        }
        if restored.is_empty() {
            // A workspace with no panes is a rectangle nothing can be typed
            // into: prune it rather than restore a hole (D-M1-9).
            if let Ok((_, effect)) = self.state.kill_workspace(id) {
                self.effects.absorb(effect);
            }
            run.report.entries.push(lost_workspace(
                saved_ws,
                "no pane in this workspace could be restored".to_owned(),
            ));
            return None;
        }

        // Only now that the workspace is certain to come back: a membership
        // recorded on a workspace about to be pruned would be state nobody can
        // reach, and a report entry about it would name a loss inside a loss.
        self.restore_worktree(saved_ws, run);

        // The transitions, in the order they would have happened live, now
        // that the workspace is settled: publishing a create for a workspace
        // that is about to be pruned would put a lie on the bus.
        self.publish(Event::WorkspaceCreated { workspace: id });
        if let Some(label) = saved_ws.label.clone() {
            self.publish(Event::WorkspaceRenamed {
                workspace: id,
                label,
            });
        }
        for pane in &restored {
            self.publish(Event::PaneCreated {
                pane: *pane,
                workspace: id,
            });
            if let Some(label) = saved.get(pane).and_then(|p| p.label.clone()) {
                self.publish(Event::PaneRenamed { pane: *pane, label });
            }
        }
        self.publish(Event::FocusChanged {
            workspace: id,
            pane: self.state.workspace(id).and_then(|ws| ws.focus()),
        });
        // `None` is reserved for "this workspace was pruned", so a count that
        // will not fit saturates rather than reporting a loss that never
        // happened.
        Some(u32::try_from(restored.len()).unwrap_or(u32::MAX))
    }

    /// Re-establish a workspace's git worktree membership, or degrade the
    /// workspace to a plain one and say so (D-M3-10).
    ///
    /// Degraded, not lost, on D-M1-9's own distinction: every pane, the layout
    /// and the label all come back, and what is missing is the *association* —
    /// which is exactly why it may not be dropped quietly, since `amx work done`
    /// reads it to decide what to delete.
    ///
    /// A directory that is there is adopted without asking git anything. Whether
    /// it is still a *registered* worktree is the acting verb's question — `amx
    /// work done` re-derives the path and reads the repository's own list — and
    /// answering it here would put git on the server's startup path, which
    /// D-M3-10 keeps in the CLI.
    fn restore_worktree(&mut self, saved_ws: &WorkspaceSnapshot, run: &mut Restoring<'_>) {
        let Some(worktree) = saved_ws.worktree.clone() else {
            return;
        };
        if worktree.path.is_dir() {
            // The workspace was adopted a few lines above: recording on it
            // cannot fail.
            let _ = self.state.set_worktree(saved_ws.id, Some(worktree));
            return;
        }
        run.report.entries.push(RestoreLoss {
            severity: RestoreSeverity::Degraded,
            entity: RestoreEntity::Workspace,
            workspace: Some(saved_ws.id),
            pane: None,
            label: saved_ws.label.clone(),
            path: Some(worktree.path),
            reason: format!(
                "the git worktree for branch {} is gone; kept as a plain workspace",
                worktree.branch
            ),
        });
    }

    /// Respawn one pane. `false` means it was pruned from the layout.
    fn restore_pane(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        saved: Option<&PaneSnapshot>,
        run: &mut Restoring<'_>,
    ) -> bool {
        let label = saved.and_then(|saved| saved.label.clone());
        let (cwd, vanished) = match saved.and_then(|saved| saved.cwd.clone()) {
            Some(saved_cwd) if saved_cwd.is_dir() => (saved_cwd, None),
            Some(gone) => (run.opts.home.clone(), Some(gone)),
            // A pane the snapshot never recorded a directory for is not a
            // loss: there was nothing to lose.
            None => (run.opts.home.clone(), None),
        };
        // Before the spawn, not after: D-M2-7 reserves the conversation first
        // so that a pane whose shell fails to start rolls the reservation back
        // and leaves it free for a later pane in the same snapshot.
        let claimed = self.claim_resume(workspace, pane, saved, run);

        match self.spawn_pane(pane, cwd.clone(), None) {
            Ok(host) => {
                let handle = host.handle().clone();
                let frames = host.frames();
                self.panes.insert(pane, host);
                // Both were just established: neither can fail.
                let _ = self.state.set_pane_cwd(pane, cwd);
                if let Some(label) = label.clone() {
                    let _ = self.state.rename_pane(pane, Some(label));
                }
                // Reported only now that the pane is really back: a degraded
                // entry for a pane that then failed to spawn would describe a
                // respawn that never happened.
                if let Some(gone) = vanished {
                    run.report.entries.push(RestoreLoss {
                        severity: RestoreSeverity::Degraded,
                        entity: RestoreEntity::Pane,
                        workspace: Some(workspace),
                        pane: Some(pane),
                        label: label.clone(),
                        path: Some(gone),
                        reason: format!(
                            "saved directory is gone; respawned in {}",
                            run.opts.home.display()
                        ),
                    });
                }
                if saved.is_none() {
                    run.report.entries.push(RestoreLoss {
                        severity: RestoreSeverity::Degraded,
                        entity: RestoreEntity::Pane,
                        workspace: Some(workspace),
                        pane: Some(pane),
                        label: None,
                        path: None,
                        reason: "the snapshot's layout names a pane it has no record of".to_owned(),
                    });
                }
                // Short numbers are settled in one pass once the tree is
                // final: see `Core::restore_shorts`.
                self.replay_sidecar(workspace, pane, &handle, &mut run.report);
                if let Some(planned) = claimed {
                    self.launch_resume(pane, &handle, frames, &planned);
                    run.resumed.insert(pane, planned.agent);
                }
                true
            }
            Err(err) => {
                // The pane exists in the tree but has nothing behind it: take
                // it out, collapsing its sibling into the slot, exactly as a
                // close would.
                if let Ok(effect) = self.state.close(workspace, pane) {
                    self.effects.absorb(effect);
                }
                // The rollback half of D-M2-7's reservation: this pane never
                // ran, so the conversation it had claimed goes back on the
                // table for whichever pane names it next.
                if let Some(planned) = claimed {
                    run.reservations.release(&planned.key);
                }
                run.report.entries.push(RestoreLoss {
                    severity: RestoreSeverity::Lost,
                    entity: RestoreEntity::Pane,
                    workspace: Some(workspace),
                    pane: Some(pane),
                    label,
                    path: Some(cwd),
                    reason: err.to_string(),
                });
                false
            }
        }
    }

    /// Feed a restored pane its saved scrollback, if it has any.
    ///
    /// Sent the instant the pane exists, so it is queued on the parser thread
    /// ahead of anything the freshly forked child manages to print. A sidecar
    /// that cannot be read costs the pane its history and nothing else — the
    /// pane itself is fine, and D-M1-9 calls that degraded, not lost.
    fn replay_sidecar(
        &self,
        workspace: WorkspaceId,
        pane: PaneId,
        handle: &PaneHandle,
        report: &mut RestoreReport,
    ) {
        let degrade = |report: &mut RestoreReport, reason: String| {
            report.entries.push(RestoreLoss {
                severity: RestoreSeverity::Degraded,
                entity: RestoreEntity::Pane,
                workspace: Some(workspace),
                pane: Some(pane),
                label: None,
                path: Some(sidecar::path(&self.ctx.state_dir, pane)),
                reason,
            });
        };
        match sidecar::load(&self.ctx.state_dir, pane) {
            Ok(None) => {}
            Ok(Some(saved)) => {
                let bytes = replay_bytes(&saved.rows);
                if !bytes.is_empty() {
                    let _ = handle.try_send(PaneCommand::Seed(bytes));
                }
                if saved.truncated {
                    degrade(
                        report,
                        "scrollback file ended mid-row; replayed what was whole".to_owned(),
                    );
                }
            }
            Err(err) => degrade(report, err.to_string()),
        }
    }

    /// Focus the workspace the snapshot had focused, or the lowest-numbered
    /// survivor if that one was pruned — a session with workspaces and no
    /// active one would render as nothing at all.
    fn focus_restored(&mut self, saved: Option<WorkspaceId>) {
        let target = saved
            .filter(|ws| self.state.workspace(*ws).is_some())
            .or_else(|| {
                self.state
                    .workspaces()
                    .map(|ws| ws.id())
                    .min_by_key(|ws| self.short_of_workspace(*ws).get())
            });
        let Some(workspace) = target else { return };
        // Present by construction: it was just looked up in the tree.
        if let Ok(effect) = self.state.switch_workspace(workspace) {
            self.effects.absorb(effect);
            let pane = self.state.workspace(workspace).and_then(|ws| ws.focus());
            self.publish(Event::FocusChanged { workspace, pane });
        }
    }
}

/// The report entry for a workspace that did not come back.
fn lost_workspace(saved: &WorkspaceSnapshot, reason: String) -> RestoreLoss {
    RestoreLoss {
        severity: RestoreSeverity::Lost,
        entity: RestoreEntity::Workspace,
        workspace: Some(saved.id),
        pane: None,
        label: saved.label.clone(),
        path: None,
        reason,
    }
}

/// Turn saved rows back into the bytes a terminal would have printed.
///
/// Soft-wrapped rows are joined into the logical line they came from and the
/// line is terminated once, so the replay re-wraps at the pane's *current*
/// width instead of at yesterday's (D-M1-6). The last line is terminated too,
/// even if it was mid-wrap when it was saved: the shell's first prompt has to
/// land on a fresh row, not on top of restored text.
fn replay_bytes(rows: &[SidecarRow]) -> Vec<u8> {
    let mut out = Vec::new();
    for row in rows {
        out.extend_from_slice(&row.text);
        if !row.wrapped {
            out.extend_from_slice(b"\r\n");
        }
    }
    if rows.last().is_some_and(|row| row.wrapped) {
        out.extend_from_slice(b"\r\n");
    }
    out
}
