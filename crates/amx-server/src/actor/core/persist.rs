//! `Core`'s half of persistence: the capture the `Persist` actor asks for,
//! and the restore report it leaves behind.
//!
//! The capture split of `docs/07-m1-plan.md` §2: capture happens here because
//! `Core` owns the state, writing happens on the `Persist` actor, and fsync
//! happens on the blocking pool. `Persist` asks through the ordinary
//! [`CoreHandle`](crate::actor::CoreHandle) mailbox — no back door — so a
//! capture that arrives while `Core` is folding a batch simply queues, exactly
//! like every other command, and a capture can never fail.
//!
//! The other direction, applying a snapshot at startup, is [`super::restore`];
//! what it leaves on `Core` is served from here, because a restore report is
//! state the session answers questions about (04 §6: queryable, never
//! log-only) rather than anything the restore path keeps.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use amx_core::PaneId;
use amx_core::agent::{AgentKind, RefSource, StartSource};
use amx_proto::control::session::{ReportReply, RestoreReport, RestoreSummary};
use tokio::sync::oneshot;

use super::Core;
use crate::actor::{Capture, PaneCommand, Reply};
use crate::persist::{AgentSnapshot, PaneSnapshot, Snapshot, WorkspaceSnapshot};

/// How long a whole capture waits for the foreground-cwd probes it started.
///
/// One deadline for the batch rather than one per pane: the probes are all in
/// flight at once (each is a `try_send` and a `oneshot`), so a session with
/// twenty panes costs the same wait as one with two — and `Core` is
/// single-threaded, so every millisecond spent here is a millisecond no
/// keystroke is being routed. A pane that misses the deadline contributes its
/// stored cwd, which is a stale directory rather than a wrong one.
pub const CAPTURE_PROBE_BUDGET: Duration = Duration::from_millis(100);

/// What this server's startup restore did, kept for the process's lifetime.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Restored {
    /// Every entry, as `session.report` serves it.
    pub(super) report: RestoreReport,
    /// Workspaces plus panes that came back, which the entries cannot say.
    pub(super) entities: u32,
    /// The panes this restore typed a conversation back into, and which agent
    /// it was.
    ///
    /// The one fact about how an agent got into a pane that only the restore
    /// path knows: a resumed agent arrives in a respawned *shell* (D-M2-7's
    /// type-in launch), so its pane's argv looks exactly like any other shell's
    /// and [`Core::start_source`] would otherwise call it `detected`. Kept
    /// beside the report because it has the same lifetime — for as long as this
    /// server can be asked what its restore did.
    pub(super) resumed: HashMap<PaneId, AgentKind>,
}

impl Core {
    // ------------------------------------------------------------- capture

    /// Assemble a snapshot of the session for the `Persist` actor.
    ///
    /// `sidecars` asks for a clone of each live pane's command handle as well,
    /// so a scrollback dump can read history through the parser thread; it is
    /// false whenever `[persist] history` is off, and then the dump path costs
    /// nothing.
    ///
    /// This is the synchronous shape, reached through
    /// [`Core::absorb`](super::Core::absorb): stored state only, no probes.
    /// [`Core::handle_capture_live`] is what the running actor uses.
    pub(super) fn handle_capture(&mut self, sidecars: bool, reply: oneshot::Sender<Capture>) {
        let capture = Capture {
            snapshot: Box::new(self.capture_cheap()),
            panes: if sidecars {
                self.panes
                    .iter()
                    .map(|(pane, host)| (*pane, host.handle().clone()))
                    .collect()
            } else {
                Vec::new()
            },
        };
        let _ = reply.send(capture);
    }

    /// The capture the running actor serves: stored state, with every pane's
    /// cwd refreshed from its foreground process first.
    ///
    /// amx has no OSC 7 plumbing, so a pane's recorded cwd is where its shell
    /// *started*; a user who has spent the day in another directory would
    /// otherwise restore into yesterday's. The refresh is bounded
    /// ([`CAPTURE_PROBE_BUDGET`]) and falls back to the stored value, because a
    /// stale cwd is a degraded restore and a wedged `Core` is a dead session.
    pub(super) async fn handle_capture_live(
        &mut self,
        sidecars: bool,
        reply: oneshot::Sender<Capture>,
    ) {
        self.refresh_cwds().await;
        self.handle_capture(sidecars, reply);
    }

    /// Ask every live pane where its foreground process is, and record what
    /// answers in time.
    async fn refresh_cwds(&mut self) {
        if self.capture_budget.is_zero() {
            // No time to wait for an answer is no reason to ask the question:
            // every pane contributes its stored cwd, and no pane's mailbox is
            // disturbed for a reply that would be thrown away.
            return;
        }
        let mut pending = Vec::with_capacity(self.panes.len());
        for (pane, host) in &self.panes {
            let (tx, rx) = oneshot::channel();
            // `try_send`, never a blocking send: `Core` waiting on a pane's
            // mailbox while that pane waits on `Core`'s is a deadlock, and a
            // refreshed cwd is not worth the session.
            if host
                .handle()
                .try_send(PaneCommand::ForegroundCwd(tx))
                .is_ok()
            {
                pending.push((*pane, rx));
            }
        }
        let deadline = tokio::time::Instant::now() + self.capture_budget;
        let mut found = Vec::with_capacity(pending.len());
        for (pane, rx) in pending {
            if let Ok(Ok(Some(cwd))) = tokio::time::timeout_at(deadline, rx).await {
                found.push((pane, cwd));
            }
        }
        for (pane, cwd) in found {
            let _ = self.state.set_pane_cwd(pane, cwd);
        }
    }

    /// A snapshot built from stored state alone — no pane is asked anything.
    ///
    /// The shutdown push (`Core::run`'s break path) uses this: the panes are
    /// about to be killed, and nothing on the drain path may wait on a sibling
    /// actor (R-M1-2).
    #[must_use]
    pub fn capture_cheap(&self) -> Snapshot {
        let mut workspaces = Vec::new();
        let mut panes = Vec::new();
        for ws in self.state.workspaces() {
            workspaces.push(WorkspaceSnapshot {
                id: ws.id(),
                short: self.short_of_workspace(ws.id()),
                label: ws.label().map(str::to_owned),
                layout: ws.layout().clone(),
                focus: ws.focus(),
            });
            for pane in ws.layout().panes() {
                let state = self.state.pane(pane);
                panes.push(PaneSnapshot {
                    id: pane,
                    short: self.short_of_pane(pane),
                    label: state.and_then(|p| p.label().map(str::to_owned)),
                    cwd: state.and_then(|p| p.cwd().map(Path::to_path_buf)),
                    // V07 records the spawn argv into pane state and **V15**
                    // captures both of these from `Core`'s agent mirror, which
                    // `AgentHub` fills with `try_send` during normal operation
                    // — which is exactly why the hub has nothing to flush on
                    // the way down (`docs/08-m2-plan.md` §3).
                    argv: state.and_then(|p| p.argv().map(<[String]>::to_vec)),
                    agent: self.captured_agent(pane),
                });
            }
        }
        // Short-number order, like `session.state` and for the same reason: the
        // state tree's own map is keyed by UUID, and a snapshot whose order
        // wandered between saves would make every diff of the file unreadable.
        workspaces.sort_by_key(|ws| ws.short.get());
        panes.sort_by_key(|pane| pane.short.get());
        Snapshot {
            version: crate::persist::VERSION,
            focused_workspace: self.state.active_workspace(),
            workspaces,
            panes,
        }
    }

    /// What the snapshot remembers about `pane`'s agent, if it had one.
    ///
    /// Read out of `Core`'s mirror — the slow read model of
    /// `docs/08-m2-plan.md` §3, which `AgentHub` fills with `try_send` during
    /// normal operation. That is precisely why the hub's shutdown flushes
    /// nothing: whatever it knew is already here by the time the final capture
    /// runs.
    ///
    /// Only *durable* facts cross into the file. The mirror carries a live
    /// status — `working`, its cause, its transition sequence, its queue
    /// position — and none of it is written: restoring "this agent was working
    /// yesterday" would be restoring a lie, and `crate::persist::AgentSnapshot`
    /// has nowhere to put it (D-M2-7).
    ///
    /// The source is *derived* rather than mirrored, and it is not a guess:
    /// `AgentHub` accepts a ref only from the hook path of the agent it claims
    /// (D-M2-7's first gate), so a ref that reached the mirror under `kind`
    /// came from `amx:<kind>` and from nowhere else. Writing it out is what
    /// gives the second and third gates something to check on the way back in.
    fn captured_agent(&self, pane: PaneId) -> Option<AgentSnapshot> {
        let status = self.agent_status.get(&pane)?;
        let kind = status.kind.clone()?;
        Some(AgentSnapshot {
            name: self
                .state
                .pane(pane)
                .and_then(|p| p.label().map(str::to_owned)),
            session_ref: status.session_ref.clone(),
            source: status
                .session_ref
                .as_ref()
                .map(|_| RefSource::for_agent(&kind)),
            start_source: self.start_source(pane, &kind),
            kind,
        })
    }

    /// How the agent in `pane` got there, as far as `Core` can tell.
    ///
    /// Three answers and two sources for them. A pane this server's own restore
    /// typed a conversation into is [`StartSource::Restored`], which is a fact
    /// only [`Restored::resumed`] holds — the agent arrived in a respawned
    /// shell, so nothing about the pane's process says so. Otherwise the pane's
    /// recorded argv decides: a pane whose spawned program *is* the agent is
    /// one amx launched, and a pane running something else with an agent
    /// identified in it is one the agent was typed into.
    ///
    /// The argv comparison is against the stanza id rather than against the
    /// stanza's `executables`, because `Core` holds no registry — the registry
    /// is `AgentHub`'s, parsed once (D-M2-2), and asking it here would be the
    /// sibling round trip §3 spends a whole section avoiding. The two agree for
    /// every stanza whose id names its program, which is both shipped agents;
    /// where they would not, this field is the looser answer, and it is a
    /// record of how a pane came to be rather than anything restore branches
    /// on — D-M2-7 types every agent back into a shell regardless.
    fn start_source(&self, pane: PaneId, kind: &AgentKind) -> StartSource {
        let resumed = self
            .restore
            .as_ref()
            .and_then(|restored| restored.resumed.get(&pane));
        if resumed == Some(kind) {
            return StartSource::Restored;
        }
        let spawned_as_agent = self
            .state
            .pane(pane)
            .and_then(|p| p.argv())
            .and_then(<[String]>::first)
            .map(Path::new)
            .and_then(Path::file_name)
            .is_some_and(|program| program == kind.as_str());
        if spawned_as_agent {
            StartSource::Started
        } else {
            StartSource::Detected
        }
    }

    // -------------------------------------------------------------- report

    /// Serve `session.report`: the entries, whole.
    pub(super) fn handle_report(&self, reply: Reply<ReportReply>) {
        let _ = reply.send(Ok(ReportReply {
            seq: self.ctx.bus.head(),
            report: self
                .restore
                .as_ref()
                .map(|restored| restored.report.clone())
                .unwrap_or_default(),
        }));
    }

    /// The counts `session.state` carries, or `None` on a server that started
    /// fresh.
    pub(super) fn restore_summary(&self) -> Option<RestoreSummary> {
        self.restore
            .as_ref()
            .map(|restored| restored.report.summary(restored.entities))
    }
}
