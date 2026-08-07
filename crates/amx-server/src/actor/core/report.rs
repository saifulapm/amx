//! Pane reports, folded: what a `PaneHost` tells the `Core` and what becomes
//! of it.
//!
//! A [`PaneReport`] is a fact about something that already happened on a pane's
//! own threads. `Core` is the only publisher of bus events (04 §2 — one
//! publisher per transition is what keeps sequence numbers meaningful), so
//! every report arrives here, updates whatever `Core` folds on the pane's
//! behalf, and leaves as at most one [`Event`] plus at most one [`Effect`].
//!
//! The history window is the fold worth naming: `session.state` answers
//! synchronously, so it cannot ask a pane where its scrollback starts and ends.
//! `Core` keeps the pair per pane instead, moving it as commits, invalidations
//! and evictions come in.
//!
//! `AgentHub`'s [`AgentCall`]s fold the same way and for the same reason: agent
//! status has to be answerable synchronously by `session.state` and by the
//! final capture, so the hub mirrors it here during normal operation rather
//! than being asked for it on the way down (`docs/08-m2-plan.md` §3, R-M1-2).

use amx_core::{Effect, Event, PaneId, RowId};

use super::Core;
use crate::actor::{AgentCall, AgentCommand, PaneHost, PaneReport};

impl Core {
    /// The pane's folded history window (`head`, `floor`), created at zero the
    /// first time a report mentions the pane.
    pub(super) fn history_window_mut(&mut self, pane: PaneId) -> (&mut RowId, &mut RowId) {
        let entry = self
            .history
            .entry(pane)
            .or_insert((RowId::from_raw(0), RowId::from_raw(0)));
        (&mut entry.0, &mut entry.1)
    }

    /// Fold one report from `pane`.
    pub(super) fn handle_pane_report(&mut self, pane: PaneId, report: PaneReport) {
        match report {
            PaneReport::Damage { generation } => {
                self.effects.absorb(Effect::PaneDamage(pane));
                self.publish(Event::PaneDamage { pane, generation });
            }
            // The hashes ride the pane's delta stream, not the bus: 04 §3 puts
            // them next to the rows they describe, and the bus event is the
            // session-state fact that ids `range` now exist.
            PaneReport::Committed { range, .. } => {
                let (head, _) = self.history_window_mut(pane);
                *head = (*head).max(RowId::from_raw(range.last.get().saturating_add(1)));
                self.publish(Event::HistoryCommitted { pane, range });
            }
            PaneReport::Invalidated { from_row, cause } => {
                let (head, _) = self.history_window_mut(pane);
                *head = from_row;
                self.publish(Event::HistoryInvalidated {
                    pane,
                    from_row,
                    cause,
                });
            }
            PaneReport::Evicted { oldest_row } => {
                let (head, floor) = self.history_window_mut(pane);
                *floor = (*floor).max(oldest_row);
                *head = (*head).max(*floor);
                self.publish(Event::HistoryEvicted { pane, oldest_row });
            }
            PaneReport::Title(title) => {
                self.publish(Event::PaneTitle { pane, title });
            }
            // `Event` has no bell variant (T01's frozen enum): a bell is not
            // session state, so there is nothing to publish. Flagged in T09's
            // report rather than added there — extending `Event` is T01's file.
            PaneReport::Bell => {}
            PaneReport::Exited { status } => {
                // The actor that reported this is on its way down: nothing
                // left to send it, but its task is still ours to join, so the
                // host moves to the draining list instead of being dropped.
                if let Some(host) = self.panes.remove(&pane) {
                    self.draining.push(host);
                }
                self.effects.absorb(Effect::PaneDamage(pane));
                self.publish(Event::PaneExited { pane, status });
            }
        }
    }

    /// Fold one message from `AgentHub`.
    pub(super) fn handle_agent_call(&mut self, call: AgentCall) {
        match call {
            AgentCall::Status {
                pane,
                status,
                attention,
            } => {
                match status {
                    Some(status) => {
                        self.agent_status.insert(pane, *status);
                    }
                    None => {
                        self.agent_status.remove(&pane);
                    }
                }
                self.attention = attention;
            }
            AgentCall::Focus { pane } => self.focus_agent_pane(pane),
        }
    }

    /// Put the user in front of `pane`: switch to its workspace, focus it.
    ///
    /// `agent.next`'s second half (D-M2-8). A pane that has since gone — the
    /// queue is mirrored state and the hub's copy can be one message stale — is
    /// simply not focused: this arrives with no reply channel, so there is
    /// nobody to fail, and the caller already has the honest answer that the
    /// head *was* this pane.
    fn focus_agent_pane(&mut self, pane: PaneId) {
        let Some(workspace) = self.workspace_of(pane) else {
            return;
        };
        let mut moved = false;
        for effect in [
            self.state.switch_workspace(workspace),
            self.state.focus_pane(workspace, pane),
        ]
        .into_iter()
        .flatten()
        {
            moved |= !matches!(effect, Effect::Nothing);
            self.effects.absorb(effect);
        }
        // One event for the pair, and only if something actually moved: a
        // `next-attention` onto the pane the user is already looking at is a
        // no-op, and 04 §2 gives sequence numbers to transitions, not to calls.
        if moved {
            self.publish(Event::FocusChanged {
                workspace,
                pane: Some(pane),
            });
        }
    }

    /// Hang up a closed or killed pane's terminal and park its host until the
    /// exit is reported.
    ///
    /// [`PaneHost::kill`] bypasses the command mailbox, so a pane whose
    /// mailbox is full cannot silently outlive its close — the old
    /// `try_send(Kill)` could be dropped exactly then, orphaning the child
    /// behind a success reply.
    pub(in crate::actor) fn hang_up_pane(&mut self, pane: PaneId) {
        if let Some(host) = self.panes.remove(&pane) {
            host.kill();
            self.draining.push(host);
        }
        // The hub also sees `PaneExited` on the bus, but not every hang-up
        // produces one promptly — a child ignoring its signal keeps its tracker
        // and its place in the attention queue alive, and `next-attention`
        // would send the user to a pane that is no longer in the layout.
        self.tell_agent(AgentCommand::PaneClosed { pane });
    }

    /// Send `command` to `AgentHub` if the session has one, never waiting.
    ///
    /// `try_send` and a dropped error, deliberately: a lost `PaneStarted` costs
    /// that pane its tracking until the next spawn, which is a far better
    /// outcome than a `Core` parked on a busy hub (`docs/08-m2-plan.md` §3).
    pub(super) fn tell_agent(&self, command: AgentCommand) {
        if let Some(agent) = &self.agent {
            let _ = agent.try_send(command);
        }
    }

    /// Take every pane down and join its task: nothing detached, everything
    /// joined (04 §2). Hang-up goes first because it bypasses the mailbox and
    /// unblocks an actor stuck on a stuffed pty, so the shutdown send that
    /// follows always lands.
    pub(super) async fn join_panes(&mut self) {
        let mut hosts: Vec<PaneHost> = self.panes.drain().map(|(_, host)| host).collect();
        hosts.append(&mut self.draining);
        for host in hosts {
            host.kill();
            let _ = host.shutdown().await;
        }
    }
}
