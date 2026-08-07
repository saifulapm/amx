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

use amx_core::{Effect, Event, PaneId, RowId};

use super::Core;
use crate::actor::{PaneHost, PaneReport};

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
