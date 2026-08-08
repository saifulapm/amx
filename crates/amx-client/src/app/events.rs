//! State the server pushes at this client: the snapshot it folds, and the
//! deliveries that carry that snapshot forward (D-M2-5).
//!
//! Before M2 the client polled: `session.state` after every layout-mutating
//! call and again whenever the picker opened, because there was no other way to
//! learn that anything had moved. V11 built the server's half — one
//! `events.subscribe` row, then one JSON-RPC notification per
//! [`Delivery`] on the control channel — and this module is what listens.
//!
//! Three rules govern everything below, and all three are 04 §2's:
//!
//! - **Subscribe after the snapshot, not before.** [`App::attach`] folds
//!   `session.state`, which carries the sequence it was captured at, and
//!   subscribes with `after_seq` set to it. State as of *N*, deliveries from
//!   *N+1*: no window, and no replay of transitions the snapshot already
//!   includes.
//! - **A gap is re-read state, never a skipped line.** "Subscribers that fall
//!   behind the replay buffer get an explicit `gap{from,to}` — never a silent
//!   drop", and the recovery is fixed: re-query state, then keep consuming.
//!   One resync per drain however many gaps it held, because one `session.state`
//!   answers all of them.
//! - **Events from the future are ignored, not fatal.** [`Event`] is
//!   `#[non_exhaustive]` and the tools this client talks to ship weekly, so a
//!   delivery this build cannot name costs a skipped line. A consumer that
//!   refused one would break on the first server newer than itself.
//!
//! Folding is idempotent by construction, which is what makes the overlap after
//! a resync harmless: statuses carry the sequence of the transition they
//! describe and older ones lose, and the attention queue rejects a pane it
//! already holds.
//!
//! [`App::sync_state`] lives here rather than in [`super::wired`] for the same
//! reason: it is the *other* half of one mechanism — the snapshot a
//! subscription is anchored to, and the answer a gap owes — and reading either
//! without the other tells half the story.
//!
//! # Task ownership
//!
//! **V14** owns this file, split out of `wired.rs` (R-M2-5 — that file was at
//! 455 lines of a 500-line soft budget before M2 added a line to it).

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Delivery, Event, PaneId, RowRange, Seq, WorkspaceId};
use amx_proto::control::{Method, session, wait as wait_proto};
use amx_proto::rpc::Notification;

use super::{App, AppError};
use crate::model::WorkspaceModel;

/// Whether a state fold may change which workspace this terminal is showing.
///
/// The difference between "I moved" and "the session moved", which the two
/// public spellings of the fold exist to keep apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Presentation {
    /// Show whatever the session says is focused: an attach, or this client's
    /// own `workspace.switch` / `agent.next`.
    Adopt,
    /// Keep showing what this terminal was showing.
    Keep,
}

/// What folding one notification asks the loop to do next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Folded {
    /// Nothing this build acts on: another method, a delivery it cannot name,
    /// or a transition it has no model for.
    Nothing,
    /// The model changed; the next repaint shows it.
    Applied,
    /// The mirror can no longer be trusted to be complete: a gap, or a queue of
    /// notifications that overflowed, which is the same loss by another route.
    /// Re-read state.
    Resync,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Subscribe this connection to the session's event bus.
    ///
    /// `after_seq` is the sequence `session.state` was captured at, so the
    /// first delivery is the first transition the snapshot does not already
    /// describe. Returns the sequence the subscription was taken at, which the
    /// server reports and which is what an external consumer would resume from.
    pub async fn subscribe_events(&mut self, after_seq: Option<Seq>) -> Result<Seq, AppError> {
        // Before the call, never after: the pump is spawned inside the server's
        // handler and its first delivery can reach the socket ahead of the
        // reply this call is waiting for.
        self.session.collect_notifications();
        let params = serde_json::to_value(wait_proto::SubscribeParams { after_seq })
            .map_err(|_| AppError::BadState("unencodable subscribe"))?;
        let value = self
            .call(Method::EventsSubscribe.wire_name(), params)
            .await?;
        let reply: wait_proto::SubscribeReply = serde_json::from_value(value)
            .map_err(|_| AppError::BadState("events.subscribe reply"))?;
        Ok(reply.seq)
    }

    /// Fold a fresh `session.state` into the model, adopting the session's
    /// focused workspace as this terminal's own.
    ///
    /// Right when this client is the reason state moved — it attached, or it
    /// made a call that switched workspace — and wrong when some other
    /// connection did (see [`resync_state`](Self::resync_state)).
    ///
    /// Returns the bus sequence the snapshot was captured at: 04 §2 puts one on
    /// every state-query reply precisely so a subscription can be anchored to
    /// it, and [`App::attach`](super::App::attach) is what does the anchoring.
    pub async fn sync_state(&mut self) -> Result<Seq, AppError> {
        self.fold_state(Presentation::Adopt).await
    }

    /// Re-read state without touching what this terminal is showing.
    ///
    /// The recovery half of the same mechanism: a gap, or a structural event
    /// from another connection, means the mirror is incomplete — not that this
    /// client asked to look somewhere else. 04 §3 gives every client its own
    /// presentation, and being yanked into another workspace because someone
    /// else split a pane there is not one this terminal asked for
    /// (`tests/adversarial.rs` pins it: "its screen owes the flood pane
    /// nothing").
    pub async fn resync_state(&mut self) -> Result<Seq, AppError> {
        self.fold_state(Presentation::Keep).await
    }

    /// The one fold both spellings share.
    async fn fold_state(&mut self, presentation: Presentation) -> Result<Seq, AppError> {
        self.resyncs += 1;
        let value = self
            .call(Method::SessionState.wire_name(), serde_json::json!({}))
            .await?;
        let state: session::StateReply =
            serde_json::from_value(value).map_err(|_| AppError::BadState("session.state reply"))?;

        let workspaces: Vec<WorkspaceId> = state.workspaces.iter().map(|ws| ws.workspace).collect();
        self.model.retain_workspaces(|id| workspaces.contains(&id));
        let panes: Vec<PaneId> = state.panes.iter().map(|pane| pane.pane).collect();
        self.model.retain_panes(|id| panes.contains(&id));
        self.caches.retain(|id, _| panes.contains(id));

        for ws in &state.workspaces {
            self.model.set_workspace(
                ws.workspace,
                WorkspaceModel {
                    label: ws.label.clone(),
                    layout: ws.layout.clone(),
                },
            );
            if let Some(focus) = ws.focus {
                self.focus.insert(ws.workspace, focus);
            }
        }
        // A mirror that holds no workspace yet has nothing to preserve: the
        // first fold of a fresh attach must land somewhere, whichever spelling
        // asked for it.
        if let Some(focused) = state.focused_workspace
            && (presentation == Presentation::Adopt || self.model.focused_workspace_id().is_none())
        {
            self.model.focus_workspace(focused);
        }
        // Loss is state, not a log line (04 §6): the summary rides every
        // snapshot, so the indicator survives a resync and clears itself the
        // moment a server reports a clean start.
        self.model.set_restore(state.restore);
        // Before the per-pane statuses, because a snapshot's queue position is
        // derived from the queue, and folding them the other way round would
        // stamp each pane with a position read from the previous queue.
        self.model.set_attention(state.attention.clone());
        for pane in &state.panes {
            self.model.set_pane_label(pane.pane, pane.label.clone());
            self.model.set_pane_agent(pane.pane, pane.agent.clone());
            let cache = self.caches.entry(pane.pane).or_default();
            let known = cache.head().get();
            let head = pane.history_head.get();
            if head < known {
                cache.invalidate(pane.history_head);
            } else if head > known {
                cache.commit(RowRange::new(
                    amx_core::RowId::from_raw(known),
                    amx_core::RowId::from_raw(head - 1),
                ));
            }
            cache.evict(pane.history_floor);
        }

        self.bind_visible().await?;
        self.layout_dirty = true;
        self.dirty = true;
        Ok(state.seq)
    }

    /// Fold every notification the session has read since the last drain,
    /// re-syncing once if any of them said the mirror had fallen behind.
    ///
    /// Called after every wake of the loop and after every round of input, so
    /// a notification that arrived while a call was in flight is folded as soon
    /// as the reply is in rather than waiting for the next frame.
    pub async fn drain_events(&mut self) -> Result<(), AppError> {
        // The buffer is reused across drains: event dispatch is a hot path
        // (04's performance rule names it), and a fresh `Vec` per wake would
        // allocate on every damage batch a busy session produces.
        let mut pending = std::mem::take(&mut self.events);
        let mut resync = self.session.take_notifications(&mut pending);
        for notification in &pending {
            match self.apply_notification(notification) {
                Folded::Nothing => {}
                Folded::Applied => self.dirty = true,
                Folded::Resync => resync = true,
            }
        }
        pending.clear();
        self.events = pending;
        if resync {
            self.resync_state().await?;
        }
        Ok(())
    }

    /// Fold one notification into the model.
    ///
    /// Public because it is the whole of the client's event vocabulary and a
    /// test drives it directly: a delivery carrying a tag no build knows cannot
    /// be published through the typed bus, so the only honest way to prove the
    /// catch-all rule is to hand one in here.
    pub fn apply_notification(&mut self, notification: &Notification) -> Folded {
        if notification.method != wait_proto::EVENT_METHOD {
            // A server-initiated method this build has no reading of. Skipped,
            // exactly like an unknown event tag, and for the same reason.
            return Folded::Nothing;
        }
        let Some(params) = notification.params.clone() else {
            return Folded::Nothing;
        };
        // A delivery whose event tag this build has never heard of fails to
        // decode here, and that is the whole of the `#[non_exhaustive]`
        // contract on the wire: skip the line, keep the stream.
        let Ok(delivery) = serde_json::from_value::<Delivery>(params) else {
            return Folded::Nothing;
        };
        match delivery {
            Delivery::Event(envelope) => {
                // Before the fold, and whatever the fold makes of it: the
                // cursor is how far this client has *read* the stream, not how
                // much of it it understood. An event this build has no model
                // for is still an event a reattach must not ask to have
                // replayed (`super::reconnect`).
                self.consumed(envelope.seq);
                self.apply_event(envelope.seq, &envelope.event)
            }
            // The events are gone; the state they described is not. `to` is
            // where the stream resumes, so that is where the cursor goes — the
            // gap is loss made visible, and pretending it did not move the
            // stream on would ask a successor to replay a window nothing can
            // still produce.
            Delivery::Gap { to, .. } => {
                self.consumed(to);
                Folded::Resync
            }
        }
    }

    /// Fold one event published at `seq`.
    ///
    /// The catch-all arm is required — [`Event`] is `#[non_exhaustive]`, so no
    /// crate but its own can match it exhaustively — and it is also correct:
    /// the transitions this client has no model for are the ones a resync after
    /// a layout-mutating call already covers.
    fn apply_event(&mut self, seq: Seq, event: &Event) -> Folded {
        match *event {
            // The layout tree moved. Re-read state: the event says *that* a
            // workspace changed shape, never what it changed to, and the tree
            // is not something a mirror can reconstruct from a pane id.
            //
            // Only the calls this client makes itself resync (`mutates_layout`
            // in `super::wired`), so without this arm a pane minted by another
            // connection never appears on this screen at all — which is what
            // `amx agent start`, run from a second terminal, does every time.
            Event::PaneCreated { .. } | Event::LayoutChanged { .. } => Folded::Resync,
            // The notification rides `attention_enqueued` and not this, even
            // though a move into `blocked` implies one: the hub publishes both
            // and notifying from each would notify twice.
            Event::AgentStatus {
                pane, to, cause, ..
            } => folded(self.model.apply_agent_status(pane, to, cause, seq)),
            Event::AgentIdentified { pane, ref kind } => {
                folded(self.model.apply_agent_identified(pane, kind.clone(), seq))
            }
            Event::AttentionEnqueued { pane, .. } => {
                if !self.model.enqueue_attention(pane) {
                    return Folded::Nothing;
                }
                self.notify_attention(pane);
                Folded::Applied
            }
            Event::AttentionDequeued { pane, .. } => folded(self.model.dequeue_attention(pane)),
            // Focus is server state and every client hears every move of it:
            // a `pane.focus` from this client or another, a `workspace.switch`,
            // an `agent.next`, a restore. Which pane is focused *in* a
            // workspace is recorded for all of them, because that is what this
            // client will land on the moment it looks at one.
            //
            // Which workspace this terminal is *showing* is not taken from
            // here. 04 §3 gives the client its own presentation, and being
            // yanked to another workspace because a second client switched — or
            // because a one-shot `amx workspace create` ran in some other
            // terminal — is not a presentation this one asked for; the flood
            // test in `tests/adversarial.rs` pins exactly that ("its screen owes
            // the flood pane nothing"). The one cross-workspace move that *is*
            // asked for is `agent.next`, and the client that asked follows its
            // own reply — see `mutates_layout` in `super::wired`.
            Event::FocusChanged { workspace, pane } => {
                if let Some(pane) = pane {
                    self.focus.insert(workspace, pane);
                }
                let showing = self.model.focused_workspace_id() == Some(workspace);
                if showing {
                    self.layout_dirty = true;
                }
                folded(showing)
            }
            _ => Folded::Nothing,
        }
    }

    /// Write an OSC 9 desktop notification for `pane` into the host terminal.
    ///
    /// 03 §4 allows amx exactly one built-in notify path and bounds it: "OSC
    /// 9/99 escapes emitted by the client to the host terminal (SSH-safe,
    /// chrome-free, a few dozen lines) on blocked-agent events". Everything
    /// richer is an extension consuming the same queue — `examples/notify.sh`
    /// (V16) is the reference one.
    ///
    /// It lands in [`App::emit`], the buffer OSC 52 already flushes after each
    /// frame, so this needed no plumbing of its own.
    ///
    /// **OSC 9 only, deliberately.** Every terminal that implements kitty's
    /// OSC 99 protocol also implements the older OSC 9, and there is no way for
    /// a client to ask the host which it prefers — emitting both would notify
    /// twice on exactly the terminals that support the better one. Choosing
    /// between them needs a capability the client can observe, and this build
    /// has none, so it sends the one that works everywhere and says so here
    /// rather than guessing from `$TERM`.
    fn notify_attention(&mut self, pane: PaneId) {
        self.emit.extend_from_slice(b"\x1b]9;");
        self.emit.extend_from_slice(b"amx: ");
        match self.model.pane_label(pane) {
            Some(label) => push_printable(&mut self.emit, label),
            None => self.emit.extend_from_slice(b"an agent"),
        }
        self.emit.extend_from_slice(b" needs attention");
        // BEL, not ST: OSC 9 predates the string terminator's general use and
        // the terminals that implement it document the BEL form.
        self.emit.push(0x07);
    }

    /// The bytes owed to this client's own terminal outside the frame — OSC 52
    /// yanks and OSC 9 notifications — as the loop flushes them.
    #[must_use]
    pub fn emitted(&self) -> &[u8] {
        &self.emit
    }
}

/// Append `text` with every control character dropped.
///
/// A pane label is user text and it is about to be spliced into an escape
/// sequence: a stray BEL or ESC inside one would end the sequence early and
/// leave the rest of the label to be executed by the terminal as commands.
fn push_printable(out: &mut Vec<u8>, text: &str) {
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        let mut buf = [0_u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

/// [`Folded::Applied`] when something actually moved, [`Folded::Nothing`] when
/// the event described state the mirror already held.
const fn folded(changed: bool) -> Folded {
    if changed {
        Folded::Applied
    } else {
        Folded::Nothing
    }
}
