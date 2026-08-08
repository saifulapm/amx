//! The loop: one mailbox, one bus subscription, one wheel — and the drain.
//!
//! Built on `Persist`'s template (`docs/07-m1-plan.md` §2), because the two
//! actors have the same shape and the same hazard: both observe the bus, both
//! own a timer, and both have to stop without touching a sibling that is
//! stopping too.
//!
//! Three rules carry it:
//!
//! - **The wheel is armed only while a deadline exists.** [`AgentHub::due`]
//!   returns `None` for a session whose panes owe nothing, which disables the
//!   timer branch of the select entirely. A session of idle agents costs zero
//!   wakeups (03 §5) — not one every 300 ms, which is what herdr's scan loop
//!   costs a laptop's battery.
//! - **The select is not `biased`.** With a fixed order, a pane flooding the
//!   bus with damage would keep that branch ready forever and the wheel would
//!   never be reached. Random selection is what makes the confirmation cap a
//!   ceiling rather than a hope. (`Persist` learned this the same way.)
//! - **Shutdown is receive-only.** After cancellation nothing is published,
//!   nothing is evaluated, no timer is armed and no sibling is sent anything;
//!   the mailbox is drained to closure, each command answered from inside this
//!   module, and the actor returns. There is a rare, undiagnosed wedge in the
//!   runtime's `JoinSet` drain (R-M1-2), and a draining task that awaits
//!   another draining task is exactly the shape that could feed it.

use amx_core::{Delivery, Event, Subscription};
use amx_proto::control::agent as proto;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::AgentHub;
use crate::actor::AgentCommand;
use crate::agent::fusion::{Deadline, HookEdge, Input};

/// What one turn of the loop decided.
///
/// The select assigns one of these and the handler runs after it, rather than
/// inside a branch: a branch body cannot borrow the actor mutably while another
/// branch's future still borrows it.
enum Step {
    /// A mailbox message arrived.
    Command(AgentCommand),
    /// The bus delivered something.
    Delivery(Delivery),
    /// The bus is gone; stop watching it.
    BusClosed,
    /// Something on the wheel came due.
    Due,
    /// Cancelled, or the mailbox closed.
    Stop,
}

impl AgentHub {
    /// Run the actor until cancellation or a closed mailbox, then drain.
    ///
    /// `events` is a live [`Subscription`]; the caller subscribes so that the
    /// subscription's position is the caller's to choose — the serve path takes
    /// one *before* the startup restore, so the panes it brings back are
    /// tracked from their first frame.
    pub async fn run(
        mut self,
        mut mailbox: mpsc::Receiver<AgentCommand>,
        mut events: Subscription,
    ) -> super::AgentReport {
        let cancel = self.ctx.cancel.clone();
        let mut watching = true;
        loop {
            let due = self.due();
            let step = tokio::select! {
                () = cancel.cancelled() => Step::Stop,
                command = mailbox.recv() => match command {
                    Some(command) => Step::Command(command),
                    None => Step::Stop,
                },
                delivery = events.recv(), if watching => match delivery {
                    Some(delivery) => Step::Delivery(delivery),
                    None => Step::BusClosed,
                },
                () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)),
                    if due.is_some() => Step::Due,
            };
            match step {
                Step::Command(command) => self.command(command),
                Step::Delivery(delivery) => self.observe(delivery),
                Step::BusClosed => watching = false,
                Step::Due => self.fire(),
                Step::Stop => break,
            }
        }
        self.drain(mailbox).await;
        self.report()
    }

    /// The soonest any pane owes the wheel anything, or `None` — which is the
    /// answer for an idle session and the reason it costs nothing.
    fn due(&self) -> Option<Instant> {
        self.panes.values().filter_map(super::Tracked::due).min()
    }

    /// Handle one mailbox message while the session is live.
    fn command(&mut self, command: AgentCommand) {
        match command {
            AgentCommand::HookReport {
                pane,
                token,
                report,
                reply,
            } => {
                let accepted = self.hook_report(pane, &token, &report);
                let _ = reply.send(Ok(proto::ReportReply {
                    accepted,
                    seq: self.ctx.bus.head(),
                }));
            }
            AgentCommand::PaneStarted {
                pane,
                frames,
                spawn,
            } => self.track(pane, frames, spawn),
            AgentCommand::PaneClosed { pane } => self.retire(pane),
            AgentCommand::Explain { pane, reply } => {
                let _ = reply.send(self.explain(pane));
            }
            AgentCommand::NextAttention { workspace, reply } => {
                let _ = reply.send(Ok(self.next_attention(workspace)));
            }
            AgentCommand::Stanza { kind, reply } => {
                let _ = reply.send(self.stanza(&kind));
            }
        }
    }

    /// Ingest one hook report, reporting whether it was applied.
    ///
    /// Two gates, both from D-M2-4, and neither of them an error reply: a hook
    /// must never break or slow a turn, so a report that fails one is dropped
    /// and counted rather than answered with a failure the agent would surface
    /// to its user.
    fn hook_report(
        &mut self,
        pane: amx_core::PaneId,
        token: &amx_core::agent::HookToken,
        report: &proto::ReportParams,
    ) -> bool {
        let Some(stanza) = self.claimed(report) else {
            self.probe.counted_drop();
            return false;
        };
        let (kind, ref_kind) = (stanza.id.clone(), stanza.ref_kind);
        if !self.token_matches(pane, token) {
            self.probe.counted_drop();
            return false;
        }
        // A `claude` typed into a shell by hand is identified by exactly this:
        // its hooks were installed for `claude`, they inherit the pane's
        // environment (V01 §3 M6), and the report says which agent they belong
        // to. Tier 3 would say the same thing later; the hook says it now.
        if self
            .panes
            .get(&pane)
            .is_some_and(|tracked| tracked.tracker.kind.is_none())
        {
            self.identify(pane, &kind);
        }
        let edge = HookEdge::from_report(report, ref_kind);
        let directives = self.apply(pane, Input::Hook(edge));
        self.absorb(pane, &directives);
        self.probe.counted_report();
        true
    }

    /// Fold one bus delivery.
    fn observe(&mut self, delivery: Delivery) {
        let now = Instant::now();
        match delivery {
            Delivery::Event(envelope) => self.event(envelope.event, now),
            Delivery::Gap { .. } => self.schedule_all(now),
        }
    }

    /// Fold one event: damage schedules, exits retire, the rest is context.
    fn event(&mut self, event: Event, now: Instant) {
        match event {
            Event::PaneDamage { pane, .. } => self.schedule_detection(pane, now),
            Event::PaneExited { pane, .. } => self.retire(pane),
            // The title is a region the rules read, and on both shipped agents
            // it is the region that moves *first*: V06's manifests carry a
            // title rule because both paint their turn into the title before
            // the grid says anything about it.
            Event::PaneTitle { pane, title } => {
                if let Some(tracked) = self.panes.get_mut(&pane) {
                    tracked.title = title;
                }
                self.schedule_detection(pane, now);
            }
            Event::PaneCreated { pane, workspace } => {
                self.workspaces.insert(pane, workspace);
            }
            // The names mirror (D-M4-6), folded off the bus beside the pairing
            // above and never asked of `Core`. Three arms and one map is the
            // whole of what an identity-bearing `attention_enqueued` costs,
            // and it is the same shape `PaneCreated` already had: the hub
            // learns a fact by watching it be published, not by requesting it.
            //
            // A rename is the only way a label reaches this actor, which is
            // also why it is enough — a cold restore replays one for every
            // label it brings back (`actor/core/restore.rs:284,295`), and
            // `workspace.create` publishes one for a label given at creation.
            // The live handoff, which publishes nothing at all, seeds the map
            // through `inherit` instead.
            Event::PaneRenamed { pane, label } => {
                self.names.name_pane(pane, label);
            }
            Event::WorkspaceRenamed { workspace, label } => {
                self.names.name_workspace(workspace, label);
            }
            // A workspace that is gone cannot be named again, and its entry
            // would outlive every pane that could have quoted it. Panes are
            // dropped by `retire`, which runs after the dequeue it publishes.
            Event::WorkspaceClosed { workspace } => {
                self.names.forget_workspace(workspace);
            }
            // A user editing a rule to fix a wrong detection sees it take
            // effect without restarting the session (R-M2-13). The registry is
            // deliberately *not* re-read: a stanza's coverage class and grace
            // are already inside live trackers, and swapping them underneath a
            // pane mid-turn would move its status for a reason nothing on the
            // screen explains.
            Event::ConfigReloaded { .. } => self.reload_manifests(),
            _ => {}
        }
    }

    /// Fire everything the wheel owes at this instant.
    fn fire(&mut self) {
        let now = Instant::now();
        self.probe.counted_wakeup();
        // Collected before anything is applied: a deadline's directives can arm
        // another deadline, and a wheel that walked its own mutations would
        // fire the new one in the same turn it was asked for.
        let fired: Vec<(amx_core::PaneId, Deadline)> = self
            .panes
            .iter()
            .flat_map(|(pane, tracked)| {
                tracked
                    .deadlines
                    .iter()
                    .filter(|(_, at)| **at <= now)
                    .map(|(deadline, _)| (*pane, *deadline))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (pane, deadline) in fired {
            if let Some(tracked) = self.panes.get_mut(&pane) {
                tracked.deadlines.remove(&deadline);
            }
            let directives = self.apply(pane, Input::Deadline(deadline));
            self.absorb(pane, &directives);
        }
        self.evaluate_due(now);
    }

    /// Shutdown: receive until the mailbox closes, answering nothing that
    /// would leave this module, and return.
    ///
    /// Nothing here publishes, evaluates, arms a timer or asks anything of
    /// another actor. There is nothing to flush either: refs and statuses
    /// already live in `Core`'s mirror from the fire-and-forget sends made
    /// during normal operation, so `Core`'s final capture carries them without
    /// asking the hub anything (`docs/08-m2-plan.md` §3). Detection state is
    /// derived and dies with the process.
    async fn drain(&mut self, mut mailbox: mpsc::Receiver<AgentCommand>) {
        while let Some(command) = mailbox.recv().await {
            self.refuse(command);
        }
    }
}
