//! What `agent.start` and `agent.prompt` wait for.
//!
//! Two bounded waits and the two predicates they evaluate. Split out of
//! [`super`] by X02 before M4's row grew that file past the soft budget
//! (`docs/11-m4-plan.md` R-M4-5); the code is V13's, moved and not changed.
//!
//! They sit together because they share one property that is easy to lose: a
//! predicate here is a *filter*, and a gap on the bus re-evaluates whatever it
//! says, so `interested` returning `false` can delay an answer and can never
//! change one.

use std::time::Duration;

use amx_core::agent::{AgentKind, AgentSnapshot, AgentState};
use amx_core::event::{StatePredicate, Waiter};
use amx_core::{Event, PaneId, Seq};
use amx_proto::control::agent;
use amx_proto::rpc::RpcError;

use super::lookup::START_TIMEOUT_MS;
use crate::actor::StatusView;
use crate::agent::fusion::IDENTITY_GRACE;
use crate::agent::registry::AgentStanza;
use crate::conn::events::{ConnEvents, Polled, bounded, cancelled, poll};

/// Wait for a freshly spawned pane to be ready, or for the deadline.
///
/// Two things have to be true, and one thing has to have elapsed:
///
/// - the pane's status names **this** agent — identity, from the argv `Core`
///   spawned, from the first hook report, or from the foreground-job probe;
/// - that status is `Idle` — the agent is at its prompt rather than mid-turn;
/// - and the stanza's startup grace has passed.
///
/// The grace is the part that is easy to leave out and wrong to. It is the
/// window in which the fusion machine itself *refuses to read the screen*,
/// because "a booting TUI's splash matches nonsense" — so an `Idle` observed
/// inside it is the tracker's opening assumption and not evidence about
/// anything. Answering "ready" from it would mean `agent start claude`
/// returning a few milliseconds after the spawn, while Claude Code is still
/// painting its banner, which is the readiness handshake in name only. V01 §6
/// measured the two shipped agents far enough apart (1.1 s against 2.8–4.6 s)
/// that the wait is per-stanza data rather than one constant.
///
/// A timeout is an answer, not an error: [`Readiness::TimedOut`] with the pane
/// left running, which is herdr's semantics and the acceptance test
/// `start_timeout_reports_failure_but_leaves_the_pane_running`.
pub(super) async fn ready(
    events: &ConnEvents,
    pane: PaneId,
    stanza: &AgentStanza,
    timeout_ms: Option<u64>,
) -> Result<agent::Readiness, RpcError> {
    let grace = stanza
        .startup_grace_ms
        .map_or(IDENTITY_GRACE, Duration::from_millis);
    let settled = tokio::time::Instant::now() + grace;
    let view = events.status().clone();
    let kind = stanza.id.clone();
    let handshake = async move {
        // Not a poll interval: one deadline, the one the stanza states, after
        // which the question is asked exactly once and then answered by events.
        tokio::time::sleep_until(settled).await;
        Waiter::new(events.bus(), IsReady { view, pane, kind })
            .wait()
            .await
    };
    match bounded(
        handshake,
        Some(timeout_ms.unwrap_or(START_TIMEOUT_MS)),
        events.cancel(),
    )
    .await
    {
        Polled::Satisfied(Ok(_)) => Ok(agent::Readiness::Ready),
        Polled::TimedOut => Ok(agent::Readiness::TimedOut),
        Polled::Satisfied(Err(_)) | Polled::Cancelled => Err(cancelled()),
    }
}

/// Wait for a pane's agent to reach `want` through a transition later than
/// `floor`.
pub(super) async fn settled(
    events: &ConnEvents,
    pane: PaneId,
    want: AgentState,
    floor: Seq,
    timeout_ms: Option<u64>,
) -> Result<bool, RpcError> {
    let waiter = Waiter::new(
        events.bus(),
        MovedTo {
            view: events.status().clone(),
            pane,
            want,
            floor,
        },
    );
    match poll(waiter, timeout_ms, events.cancel()).await {
        Polled::Satisfied(_) => Ok(true),
        Polled::TimedOut => Ok(false),
        Polled::Cancelled => Err(cancelled()),
    }
}

// ------------------------------------------------------------- the predicates

/// A pane is running the agent it was started for, and is at its prompt.
struct IsReady {
    view: StatusView,
    pane: PaneId,
    kind: AgentKind,
}

impl StatePredicate for IsReady {
    type Output = AgentSnapshot;

    fn evaluate(&mut self) -> Option<Self::Output> {
        self.view
            .get(self.pane)
            .filter(|status| status.kind.as_ref() == Some(&self.kind))
            .filter(|status| status.state == AgentState::Idle)
    }

    /// A filter only: a gap re-evaluates whatever this says, so returning
    /// `false` can delay the answer and never change it.
    fn interested(&self, event: &Event) -> bool {
        match event {
            Event::AgentStatus { pane, .. }
            | Event::AgentIdentified { pane, .. }
            | Event::PaneExited { pane, .. } => *pane == self.pane,
            _ => false,
        }
    }
}

/// A pane's agent has reached a state *since* a given sequence.
///
/// The floor is the whole point (`AgentSnapshot::transition_seq`'s own
/// documentation says so): without it, `agent prompt --wait blocked` sent to a
/// pane that was already blocked returns instantly, having waited for nothing
/// and reported the state it was asked to watch for a change out of.
struct MovedTo {
    view: StatusView,
    pane: PaneId,
    want: AgentState,
    floor: Seq,
}

impl StatePredicate for MovedTo {
    type Output = AgentSnapshot;

    fn evaluate(&mut self) -> Option<Self::Output> {
        self.view
            .get(self.pane)
            .filter(|status| status.state == self.want)
            .filter(|status| status.transition_seq > self.floor)
    }

    fn interested(&self, event: &Event) -> bool {
        match event {
            Event::AgentStatus { pane, .. }
            | Event::AgentIdentified { pane, .. }
            | Event::PaneExited { pane, .. } => *pane == self.pane,
            _ => false,
        }
    }
}
