//! `Core`'s half of a live upgrade: the freeze, and the fence.
//!
//! §3 step 4 is one mailbox turn. Everything the successor needs is read while
//! the session is held still — the panes are quiesced, each one is captured on
//! its own parser thread, and the session-level fields (identity, bus head, the
//! persist snapshot, the hub's statuses in queue order) come off `Core`'s own
//! state — and then the whole of it goes to the orchestrator, which does the
//! talking. `Core` is stalled for exactly that turn and no longer: the socket
//! goes on answering, `session.state` goes on being served, and the panes stay
//! frozen until the swap commits or aborts.
//!
//! The two things that do *not* happen here are deliberate:
//!
//! - **the pre-flight is already done.** It ran on the connection task
//!   ([`crate::handoff::export::pre_flight`]) and its verdict arrives with the
//!   call, so a wrong binary is refused by a `Core` that has not touched a
//!   pane. herdr pays a full quiesce and rollback for the same mistake
//!   (D-M3-6 point 2);
//! - **the resume is not `Core`'s.** The orchestrator holds a pty handle per
//!   frozen pane and resumes them itself, because an abort can land while
//!   `Core` is busy and the panes' unfreezing must not queue behind anything.

use std::sync::Arc;

use amx_core::PaneId;
use amx_proto::control::session::{HandoffParams, HandoffReply, HandoffStage};
use tokio::sync::{mpsc, oneshot};

use super::Core;
use crate::actor::{PaneCommand, PaneExport, Reply};
use crate::handoff::export::{Caps, FrozenPane, Job, Ledger};
use crate::handoff::manifest::{AgentEntry, Manifest, READ_WINDOW, VERSION};
use crate::handoff::protocol::Timeouts;

/// What `Core` keeps about handing this session over.
///
/// Two halves that are `None`/empty on every `Core` nobody wired a handoff
/// into — a test's, and the direct-`absorb` path's — which is what makes
/// `session.handoff` answer "this server has no handoff path" honestly rather
/// than panicking on a capability it was never given.
#[derive(Debug, Default)]
pub(super) struct HandoffSeat {
    /// Shared with the orchestrator and with the serve assembly's drain
    /// watchdog: the commit fence, the in-flight flag, the last attempt.
    ledger: Arc<Ledger>,
    /// Where a job goes, when the serve assembly spawned an orchestrator.
    jobs: Option<mpsc::Sender<Job>>,
}

impl HandoffSeat {
    /// The shared ledger.
    pub(super) fn ledger(&self) -> &Arc<Ledger> {
        &self.ledger
    }
}

impl Core {
    /// Tell this `Core` where the export orchestrator is listening.
    ///
    /// The serve assembly calls this; a `Core` that is never told refuses
    /// `session.handoff` by name.
    pub fn set_handoff(&mut self, jobs: mpsc::Sender<Job>) {
        self.handoff.jobs = Some(jobs);
    }

    /// The handoff ledger this `Core` reads its fence from.
    ///
    /// Taken by the serve assembly before `Core` moves into its task: the
    /// post-commit drain is bounded only when this says ownership moved, and
    /// the drain runs after `Core` is gone.
    #[must_use]
    pub fn handoff_ledger(&self) -> Arc<Ledger> {
        Arc::clone(self.handoff.ledger())
    }

    /// Whether persistence is fenced because a handoff committed (R-M3-10).
    pub(super) fn handoff_fenced(&self) -> bool {
        self.handoff.ledger().fenced()
    }

    /// What `session.report` says about the last handoff attempt.
    pub(super) fn handoff_attempt(&self) -> Option<amx_proto::control::session::HandoffAttempt> {
        self.handoff.ledger().attempt()
    }

    /// `session.handoff`: freeze the session and hand it to the orchestrator.
    ///
    /// `preflight` is what `<binary> _handoff-caps` said, run before this call
    /// was made. An `Err` here is a refusal that cost nothing, and the panes
    /// are untouched when it is answered.
    pub(super) async fn handle_handoff_live(
        &mut self,
        params: HandoffParams,
        preflight: Result<Caps, String>,
        reply: Reply<HandoffReply>,
    ) {
        let ledger = self.handoff_ledger();
        let caps = match preflight {
            Ok(caps) => caps,
            Err(reason) => {
                let reason = ledger.refuse(params.binary.clone(), reason);
                self.refuse_handoff(reason, reply);
                return;
            }
        };
        let Some(jobs) = self.handoff.jobs.clone() else {
            let reason = ledger.refuse(
                params.binary.clone(),
                "this server was assembled without a handoff path".to_owned(),
            );
            self.refuse_handoff(reason, reply);
            return;
        };
        if !ledger.claim() {
            let reason = ledger.refuse(
                params.binary.clone(),
                "this session is already handing itself over".to_owned(),
            );
            self.refuse_handoff(reason, reply);
            return;
        }

        // From here the session is claimed, and every path releases it.
        match self.freeze(&caps).await {
            Ok((manifest, masters, panes)) => {
                let job = Job {
                    ctx: self.ctx.clone(),
                    binary: params.binary.clone(),
                    timeouts: timeouts_for(&params),
                    manifest,
                    masters,
                    panes,
                    ledger: Arc::clone(&ledger),
                };
                if let Err(returned) = jobs.try_send(job) {
                    let job = returned.into_inner();
                    thaw(job.panes);
                    ledger.release();
                    let reason = ledger.refuse(
                        params.binary,
                        "the export orchestrator is not accepting work".to_owned(),
                    );
                    self.refuse_handoff(reason, reply);
                    return;
                }
                let _ = reply.send(Ok(HandoffReply {
                    accepted: true,
                    reason: None,
                    seq: self.ctx.bus.head(),
                }));
            }
            Err(FreezeFailed { reason, quiesced }) => {
                // §3's step-4 row: resume the panes that did quiesce, abort,
                // and go on being the server the caller was talking to.
                thaw(quiesced);
                ledger.release();
                ledger.record(amx_proto::control::session::HandoffAttempt {
                    outcome: amx_proto::control::session::HandoffOutcome::Aborted,
                    stage: HandoffStage::PreFlight,
                    binary: params.binary,
                    reason: Some(reason.clone()),
                });
                self.refuse_handoff(reason, reply);
            }
        }
    }

    /// Answer a `session.handoff` that will not happen.
    fn refuse_handoff(&self, reason: String, reply: Reply<HandoffReply>) {
        tracing::warn!(reason = %reason, "session.handoff refused");
        let _ = reply.send(Ok(HandoffReply {
            accepted: false,
            reason: Some(reason),
            seq: self.ctx.bus.head(),
        }));
    }

    /// §3 step 4: hold every pane still and read what it is.
    ///
    /// The order is quiesce-everything, then capture-everything, and not
    /// pane-at-a-time: a pane captured while its neighbour is still running is
    /// a manifest whose entries describe two different instants, and the whole
    /// point of the freeze is that they describe one.
    async fn freeze(
        &mut self,
        caps: &Caps,
    ) -> Result<(Manifest, Vec<std::os::fd::OwnedFd>, Vec<FrozenPane>), FreezeFailed> {
        let order = self.transfer_order();
        let frozen: Vec<FrozenPane> = order
            .iter()
            .filter_map(|pane| {
                self.panes.get(pane).map(|host| FrozenPane {
                    pane: *pane,
                    pty: host.pty().clone(),
                })
            })
            .collect();

        // Blocking, so off the runtime's threads: `quiesce` drains queued
        // writes and waits for the pty actor to answer on its own thread.
        let quiescing = frozen.clone();
        let quiesced = tokio::task::spawn_blocking(move || quiesce_all(quiescing))
            .await
            .unwrap_or_else(|join| {
                Err(QuiesceFailed {
                    reason: format!("the quiesce task did not finish: {join}"),
                    done: Vec::new(),
                })
            });
        let frozen = match quiesced {
            Ok(frozen) => frozen,
            Err(QuiesceFailed { reason, done }) => {
                return Err(FreezeFailed {
                    reason,
                    quiesced: done,
                });
            }
        };

        let mut panes = Vec::with_capacity(frozen.len());
        let mut masters = Vec::with_capacity(frozen.len());
        for pane in &frozen {
            match self.capture_pane(pane.pane).await {
                Ok(export) => {
                    let mut entry = export.manifest;
                    // The one field the parser thread cannot fill: the token
                    // the child's environment carries lives in session state,
                    // and it has to cross or the successor drops every hook
                    // report from an agent that never restarted.
                    entry.token = self
                        .state
                        .pane(pane.pane)
                        .and_then(|state| state.hook_token().cloned());
                    panes.push(entry);
                    masters.push(export.master);
                }
                Err(reason) => {
                    return Err(FreezeFailed {
                        reason,
                        quiesced: frozen,
                    });
                }
            }
        }

        let manifest = Manifest {
            version: VERSION,
            read_window: READ_WINDOW.to_vec(),
            exporter: env!("CARGO_PKG_VERSION").to_owned(),
            proto: amx_proto::version::window(),
            session: self.session_id,
            seq: self.ctx.bus.head(),
            state: Box::new(self.capture_cheap()),
            panes,
            agents: self.agent_entries(),
        };
        tracing::info!(
            panes = manifest.panes.len(),
            successor = %caps.version,
            "the session is frozen and captured",
        );
        Ok((manifest, masters, frozen))
    }

    /// Ask one pane's parser thread to freeze itself into bytes and a
    /// descriptor (D-M3-4).
    async fn capture_pane(&self, pane: PaneId) -> Result<PaneExport, String> {
        let Some(host) = self.panes.get(&pane) else {
            return Err(format!(
                "pane {pane} stopped while the session was freezing"
            ));
        };
        let (tx, rx) = oneshot::channel();
        host.handle()
            .send(PaneCommand::ExportHandoff { reply: tx })
            .await
            .map_err(|err| format!("pane {pane} could not be asked to export: {err}"))?;
        match rx.await {
            Ok(Ok(export)) => Ok(export),
            Ok(Err(err)) => Err(format!("pane {pane} could not be exported: {err}")),
            Err(_) => Err(format!("pane {pane} stopped before it answered the export")),
        }
    }

    /// Every live pane, in the order their descriptors will cross.
    ///
    /// Short-number order, the same order [`Core::capture_cheap`] writes the
    /// snapshot's panes in and the same order `session.state` answers in — so
    /// the manifest's `panes` and its `state.panes` read as one document
    /// rather than two that happen to have the same members.
    fn transfer_order(&self) -> Vec<PaneId> {
        let mut live: Vec<PaneId> = self.panes.keys().copied().collect();
        live.sort_by_key(|pane| self.short_of_pane(*pane).get());
        live
    }

    /// The hub's view of each tracked pane, attention queue first.
    ///
    /// Queue order is the part that is not re-derivable — block time is not on
    /// a screen — so the queue leads and everything else follows in transfer
    /// order behind it (R-M3-13).
    fn agent_entries(&self) -> Vec<AgentEntry> {
        let mut entries = Vec::with_capacity(self.agent_status.len());
        let mut seen = std::collections::HashSet::new();
        for pane in &self.attention {
            if let Some(agent) = self.agent_status.get(pane) {
                seen.insert(*pane);
                entries.push(AgentEntry {
                    pane: *pane,
                    agent: agent.clone(),
                });
            }
        }
        for pane in self.transfer_order() {
            if seen.contains(&pane) {
                continue;
            }
            if let Some(agent) = self.agent_status.get(&pane) {
                entries.push(AgentEntry {
                    pane,
                    agent: agent.clone(),
                });
            }
        }
        entries
    }
}

/// A freeze that could not finish, and what it left quiesced.
struct FreezeFailed {
    reason: String,
    quiesced: Vec<FrozenPane>,
}

/// A quiesce that stopped part way, and what had already frozen.
struct QuiesceFailed {
    reason: String,
    done: Vec<FrozenPane>,
}

/// Freeze every pane, or say which one refused and what had frozen by then.
fn quiesce_all(panes: Vec<FrozenPane>) -> Result<Vec<FrozenPane>, QuiesceFailed> {
    let mut done: Vec<FrozenPane> = Vec::with_capacity(panes.len());
    for pane in panes {
        if let Err(err) = pane.pty.quiesce() {
            return Err(QuiesceFailed {
                reason: format!("pane {} could not be quiesced: {err}", pane.pane),
                done,
            });
        }
        done.push(pane);
    }
    Ok(done)
}

/// Undo a freeze that never became a handoff.
///
/// Detached rather than awaited, and the one place on this path that is: this
/// runs inside `Core`'s own mailbox turn, and a `Core` that waited here for a
/// blocking resume would be a session held still by its own rollback. Every
/// resume is best effort per pane and its failure is a log line, so there is no
/// answer to wait for.
fn thaw(panes: Vec<FrozenPane>) {
    if panes.is_empty() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        for pane in panes {
            if let Err(err) = pane.pty.resume() {
                tracing::warn!(pane = %pane.pane, error = %err, "a pane could not be resumed");
            }
        }
    });
}

/// The stage bounds one attempt runs under.
///
/// Absent means §3's own constants; a caller that names one gets it on every
/// stage except the advisory `owned` ack, which is 500 ms because ownership has
/// already moved by the time anybody is waiting for it.
fn timeouts_for(params: &HandoffParams) -> Timeouts {
    match params.timeout_ms {
        Some(ms) => Timeouts {
            stage: std::time::Duration::from_millis(ms),
            ..Timeouts::DEFAULT
        },
        None => Timeouts::DEFAULT,
    }
}
