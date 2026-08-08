//! One transition, folded: the queue, the wheel, and both read models.
//!
//! [`AgentHub::absorb`] is the only place in the tree where a `StatusView` is
//! written or an agent event is published, which is what makes
//! `docs/08-m2-plan.md` §3's ordering a property of the module rather than of
//! each call site:
//!
//! > the write protocol is fixed: **update `StatusView`, then publish the
//! > event.** A waiter woken by the event therefore always observes a view at
//! > least as new as the event; the reverse order can hang a wait forever.
//!
//! The order is enforced one level down, by [`StatusView::commit`] taking the
//! events it is about. What is enforced *here* is that everything a transition
//! touches moves together: the tracker's deadlines onto the wheel, the queue
//! entry in or out, the fast view, the slow mirror. A fold that updated three
//! of the four is how two views of one truth start disagreeing.
//!
//! The manifest cache lives here too, because binding one is a consequence of
//! a [`Directive::Identified`] and of nothing else.

use std::sync::Arc;

use amx_core::agent::{AgentKind, AgentSnapshot, EpochMillis};
use amx_core::{Event, PaneId};
use tokio::time::Instant;

use super::{AgentHub, Tracked, load_manifest};
use crate::actor::{AgentCall, CoreCommand, StatusUpdate};
use crate::agent::fusion::{Directive, Input};
use crate::agent::manifest::Manifest;

impl AgentHub {
    // --------------------------------------------------------- the directives

    /// Apply one input to `pane`'s tracker, if it has one.
    pub(super) fn apply(&mut self, pane: PaneId, input: Input) -> Vec<Directive> {
        self.panes
            .get_mut(&pane)
            .map(|tracked| tracked.tracker.apply(input))
            .unwrap_or_default()
    }

    /// Fold `directives` into the queue, the wheel and both read models.
    ///
    /// The one place a `StatusView` write or an agent event happens, which is
    /// what makes the ordering §3 fixes a property of the module rather than of
    /// each call site.
    pub(super) fn absorb(&mut self, pane: PaneId, directives: &[Directive]) {
        if directives.is_empty() {
            return;
        }
        let now = Instant::now();
        // The sequence this transition's event is about to take. A concurrent
        // publish from another actor can only make the real number *higher*,
        // which is the safe direction: the view a woken waiter reads is never
        // newer than the event that woke it.
        let seq = self.ctx.bus.head() + 1;
        let mut moved = false;
        let mut identified = None;
        let mut refreshed = false;

        if let Some(tracked) = self.panes.get_mut(&pane) {
            for directive in directives {
                match directive {
                    Directive::Arm { deadline, after } => {
                        tracked.deadlines.insert(*deadline, now + *after);
                    }
                    Directive::Disarm { deadline } => {
                        tracked.deadlines.remove(deadline);
                    }
                    Directive::Ref { session_ref } => {
                        tracked.session_ref = Some(session_ref.clone());
                        refreshed = true;
                    }
                    Directive::Status { .. } => {
                        tracked.transition_seq = seq;
                        // The wall-clock half of the same edge, read once for
                        // the whole fold. D-M4-4: absolute epoch milliseconds,
                        // so `amx agents --json` and an external consumer get
                        // the instant, and no surface ever renders an age
                        // against a clock that is not the one that stamped it.
                        tracked.since = wall_clock();
                        moved = true;
                    }
                    Directive::Identified { kind } => identified = Some(kind.clone()),
                    Directive::Enqueue | Directive::Dequeue => {}
                }
            }
        }

        // Assembled once, from state the fold above has already settled, and
        // used by whichever of the two queue arms runs: D15 wants an
        // `attention_enqueued` a notifier can render straight
        // (`api/backend blocked (permission_dialog)`) without a follow-up call,
        // and the dequeue that clears it says the same thing about the pane it
        // is about.
        let identity = self.identity(pane);

        // The queue and the events, in the fixed order the machine emits them:
        // the status, then the queue, then the timers.
        let mut events = Vec::new();
        for directive in directives {
            match directive {
                Directive::Status { from, to, cause } => events.push(Event::AgentStatus {
                    pane,
                    from: *from,
                    to: *to,
                    cause: *cause,
                }),
                Directive::Identified { kind } => events.push(Event::AgentIdentified {
                    pane,
                    kind: kind.clone(),
                }),
                Directive::Enqueue => {
                    self.attention.retain(|queued| *queued != pane);
                    self.attention.push(pane);
                    events.push(Event::AttentionEnqueued {
                        pane,
                        workspace: identity.workspace.clone(),
                        name: identity.name.clone(),
                        reason: identity.reason.clone(),
                        since: identity.since,
                    });
                }
                Directive::Dequeue => {
                    let before = self.attention.len();
                    self.attention.retain(|queued| *queued != pane);
                    if self.attention.len() != before {
                        events.push(Event::AttentionDequeued {
                            pane,
                            workspace: identity.workspace.clone(),
                            name: identity.name.clone(),
                            reason: identity.reason.clone(),
                            since: identity.since,
                        });
                    }
                }
                Directive::Ref { .. } | Directive::Arm { .. } | Directive::Disarm { .. } => {}
            }
        }

        if let Some(kind) = identified {
            self.bind_manifest(pane, &kind);
        }
        if events.is_empty() && !refreshed {
            // Timers moved and nothing else. Neither read model carries a
            // deadline, so there is nothing to write and — 04 §2 gives every
            // *transition* a sequence number, not every write — nothing to say.
            return;
        }
        if moved {
            self.probe.counted_transition();
        }
        let retired = self
            .panes
            .get(&pane)
            .is_some_and(|tracked| tracked.tracker.is_exited());
        let status = self
            .panes
            .get(&pane)
            .filter(|_| !retired)
            .map(Tracked::snapshot);
        let attention = self.attention.clone();
        self.write(StatusUpdate {
            pane,
            status: status.clone(),
            attention: attention.clone(),
            events,
        });
        self.mirror(pane, status, attention);
        if retired {
            self.panes.remove(&pane);
        }
    }

    /// The one call in the tree that writes a [`StatusView`].
    ///
    /// Two callers reach it and both are the hub's: [`absorb`](Self::absorb),
    /// which is a transition, and
    /// [`inherit`](super::AgentHub::inherit), which is a predecessor's
    /// transition arriving in a new process. Funnelling them through one line
    /// is what keeps `docs/08-m2-plan.md` §3's write-then-publish ordering a
    /// property of the module — and it is what `agent_events_have_exactly_one_
    /// publisher` counts.
    ///
    /// [`StatusView`]: crate::actor::StatusView
    pub(super) fn write(&self, update: StatusUpdate) -> Option<amx_core::Seq> {
        self.view.commit(&self.ctx.bus, update)
    }

    /// Push one pane's status into `Core`'s mirror, never waiting.
    ///
    /// The queue position is filled in *here* rather than left for the reader,
    /// because `Core` answers `session.state` from this map directly and has no
    /// queue of its own to derive it from.
    pub(super) fn mirror(
        &self,
        pane: PaneId,
        status: Option<AgentSnapshot>,
        attention: Vec<PaneId>,
    ) {
        let status = status.map(|mut status| {
            status.attention = attention
                .iter()
                .position(|queued| *queued == pane)
                .and_then(|index| u32::try_from(index).ok());
            Box::new(status)
        });
        let _ = self.core.try_send(CoreCommand::Agent(AgentCall::Status {
            pane,
            status,
            attention,
        }));
    }

    // ----------------------------------------------------------- the manifest

    /// Bind `pane` to the compiled manifest of the agent just identified.
    pub(super) fn bind_manifest(&mut self, pane: PaneId, kind: &AgentKind) {
        let Some(name) = self
            .registry
            .resolve(kind)
            .and_then(|stanza| stanza.manifest.clone())
        else {
            return;
        };
        let compiled = self.manifest(&name);
        if let Some(tracked) = self.panes.get_mut(&pane) {
            tracked.manifest = compiled;
        }
    }

    /// The compiled manifest called `name`, loading it the first time.
    ///
    /// An override file of that name shadows the bundled one whole — the same
    /// rule the registry's merge keeps, for the same reason: half a rule set
    /// from each is a manifest no file on disk describes.
    pub(super) fn manifest(&mut self, name: &str) -> Option<Arc<Manifest>> {
        if let Some(compiled) = self.manifests.get(name) {
            return Some(Arc::clone(compiled));
        }
        let compiled = Arc::new(load_manifest(&self.ctx.config_path, name)?);
        self.manifests
            .insert(name.to_owned(), Arc::clone(&compiled));
        Some(compiled)
    }

    /// Forget every compiled manifest and re-bind the panes that were using
    /// one.
    ///
    /// The config watcher's reload event is the trigger (R-M2-13): a user
    /// editing a rule to fix a wrong detection sees it take effect without
    /// restarting the session, which is the whole of hot reloading in M2.
    pub(super) fn reload_manifests(&mut self) {
        self.manifests.clear();
        let bindings: Vec<(PaneId, AgentKind)> = self
            .panes
            .iter()
            .filter_map(|(pane, tracked)| Some((*pane, tracked.tracker.kind.clone()?)))
            .collect();
        for (pane, kind) in bindings {
            self.bind_manifest(pane, &kind);
        }
    }
}

/// Now, in epoch milliseconds — the units D-M4-4 fixed for
/// [`AgentSnapshot::since`](amx_core::agent::AgentSnapshot::since).
///
/// The one wall clock in this actor, and it is read *only* to stamp a
/// transition. Everything the hub decides — spacing, confirmation caps,
/// staleness — runs off [`tokio::time::Instant`], which a test can pause; this
/// is the value a person is shown, and there is no monotonic spelling of "4m
/// ago" that survives being carried to another machine.
///
/// A clock set before 1970 is the only way [`SystemTime::duration_since`] can
/// fail, and the answer to it is the one an absent `since` already means:
/// nobody can say when this happened. A zero would say 1970 and be believed.
///
/// [`SystemTime::duration_since`]: std::time::SystemTime::duration_since
fn wall_clock() -> Option<EpochMillis> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| EpochMillis::try_from(since.as_millis()).ok())
}
