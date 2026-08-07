//! `wait` and `pane.wait_output`: the long-poll handlers.
//!
//! They sit together because they share the one thing that makes a wait safe.
//! 04 §2:
//!
//! > **Waits are state predicates, not event predicates.** … on any `gap` it
//! > re-evaluates state before consuming events after `to`. A transition
//! > falling inside a gap can therefore never hang a wait.
//!
//! [`Waiter`](amx_core::event::Waiter) already encodes that sequencing, and its
//! own doc comment names the hang you build by ignoring it: "a predicate that
//! accumulates event history is an event predicate wearing this trait's
//! clothes, and it will hang the first time its transition lands inside a gap."
//! These are its first consumers.
//!
//! The predicates read *live* state and nothing else:
//!
//! - `wait --until blocked|idle` reads `AgentHub`'s `StatusView`, which the hub
//!   writes **before** publishing the matching event (`docs/08-m2-plan.md` §3).
//!   That ordering is what makes a wait woken by the event certain to see a
//!   view at least as new as it.
//! - `wait --until exited` reads `Core`'s live pane table: the pane either has
//!   a process or it does not.
//! - `pane.wait_output` reads the pane's published snapshot through V05's text
//!   view, re-run per `PaneDamage` batch.
//!
//! Timeouts are parameters, never sleeps. The hygiene suite rejects a nap.
//!
//! # Why `exited` does not use `Waiter`
//!
//! [`StatePredicate::evaluate`](amx_core::event::StatePredicate::evaluate) is
//! synchronous, which is right for the two predicates that read shared memory —
//! the `StatusView` behind an `RwLock`, the pane's lock-free snapshot feed —
//! and impossible for the one that reads `Core`, because `Core` is an actor and
//! reading it is a mailbox round trip. Nothing here weakens the contract to fit:
//! [`exited`] runs `Waiter`'s sequencing step for step — subscribe, evaluate
//! before consuming anything, re-evaluate unconditionally on a gap — with an
//! `await` where the trait has none. The duplication is small, deliberate and
//! flagged; the alternative was a predicate that remembered a `PaneExited` it
//! had seen, which is the event predicate the whole design refuses.
//!
//! # Task ownership
//!
//! **V11** filled [`wait`] and [`wait_output`], and owns `conn/events.rs` and
//! the `amx events --json` CLI beside them.

use amx_core::agent::{AgentSnapshot, AgentState};
use amx_core::event::{StatePredicate, Waiter};
use amx_core::{Delivery, Event, PaneId, RowId};
use amx_proto::control::session;
use amx_proto::control::wait as proto;
use amx_proto::rpc::RpcError;
use regex::Regex;

use super::{Router, pane};
use crate::actor::{CoreCommand, PaneWiring, SessionCall, SnapshotFeed, StatusView, StreamCall};
use crate::conn::events::{ConnEvents, Polled, bounded, cancelled, poll};

/// `wait`: until a pane's agent is blocked or idle, or its process ends.
///
/// There is no `done` status to wait for (04 §2 says so outright) and no poll
/// interval anywhere: the predicate is evaluated once at subscribe time and
/// then only on events that could have changed it.
pub(super) async fn wait(
    router: &mut Router,
    params: proto::WaitParams,
) -> Result<proto::WaitReply, RpcError> {
    let events = router.events()?.clone();
    let pane = pane::resolve(router, &params.target).await?;
    // A target that names no pane at all is a caller error, not a condition
    // that will one day hold: said now, while there is a reply to say it in,
    // rather than left to expire as a timeout that reads like a slow agent.
    let state = session_state(router).await?;
    if !state.panes.iter().any(|known| known.pane == pane) {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            format!("no pane {pane} in this session"),
        ));
    }
    match params.until {
        proto::WaitUntil::Blocked => {
            status(&events, pane, AgentState::Blocked, params.timeout_ms).await
        }
        proto::WaitUntil::Idle => status(&events, pane, AgentState::Idle, params.timeout_ms).await,
        proto::WaitUntil::Exited => exited(router, &events, pane, params.timeout_ms).await,
    }
}

/// Wait for a pane's agent to reach `want`.
async fn status(
    events: &ConnEvents,
    pane: PaneId,
    want: AgentState,
    timeout_ms: Option<u64>,
) -> Result<proto::WaitReply, RpcError> {
    let view = events.status().clone();
    let waiter = Waiter::new(events.bus(), StatusIs { view, pane, want });
    let satisfied = match poll(waiter, timeout_ms, events.cancel()).await {
        Polled::Satisfied(_) => true,
        Polled::TimedOut => false,
        Polled::Cancelled => return Err(cancelled()),
    };
    Ok(proto::WaitReply {
        pane,
        satisfied,
        agent: events.status().get(pane),
        status: None,
        seq: events.bus().head(),
    })
}

/// Wait for a pane's process to end.
///
/// The predicate is "`Core` has no live process for this pane", which `Core`
/// updates *before* it publishes `PaneExited` (`actor/core/report.rs` removes
/// the host and then publishes) — the same write-before-publish ordering the
/// `StatusView` keeps, and the reason a waiter woken by that event cannot read
/// a stale answer.
///
/// The child's exit status is not live state — nothing keeps it once the host
/// is gone — so it is read off the `PaneExited` event as it passes and reported
/// when it was there to read. `None` therefore means "signalled, or the exit
/// was already history when the wait began", which is what the reply's field
/// documents.
async fn exited(
    router: &Router,
    events: &ConnEvents,
    pane: PaneId,
    timeout_ms: Option<u64>,
) -> Result<proto::WaitReply, RpcError> {
    let mut subscription = events.bus().subscribe();
    let mut status = None;
    let waiting = async {
        // Step 2 of `Waiter`'s sequencing: a condition that already holds must
        // not wait for a transition that already happened.
        if is_gone(router, pane).await? {
            return Ok(status);
        }
        loop {
            match subscription.recv().await {
                None => return Err(cancelled()),
                // A gap re-evaluates unconditionally: the exit may have been
                // published inside it, and re-reading state is the only honest
                // way to find out.
                Some(Delivery::Gap { .. }) => {
                    if is_gone(router, pane).await? {
                        return Ok(status);
                    }
                }
                Some(Delivery::Event(envelope)) => {
                    if let Event::PaneExited {
                        pane: exited,
                        status: code,
                    } = envelope.event
                        && exited == pane
                    {
                        status = code;
                        if is_gone(router, pane).await? {
                            return Ok(status);
                        }
                    }
                }
            }
        }
    };

    let (satisfied, status) = match bounded(waiting, timeout_ms, events.cancel()).await {
        Polled::Satisfied(Ok(status)) => (true, status),
        Polled::Satisfied(Err(err)) => return Err(err),
        Polled::TimedOut => (false, None),
        Polled::Cancelled => return Err(cancelled()),
    };
    Ok(proto::WaitReply {
        pane,
        satisfied,
        agent: events.status().get(pane),
        status,
        seq: events.bus().head(),
    })
}

/// `pane.wait_output`: until text or a regex appears on a pane's screen.
///
/// It matches the *screen*, not a byte stream: the predicate runs over the
/// visible grid per damage batch, so content that scrolled through between two
/// batches was never on screen when anything looked. `pane.read` and
/// `pane.history` are where a caller goes instead, and the params type says so.
///
/// The regex is compiled once per call and never per evaluation — a detection
/// path that recompiled under output pressure would pay for the pattern on
/// every damage batch.
pub(super) async fn wait_output(
    router: &mut Router,
    params: proto::WaitOutputParams,
) -> Result<proto::WaitOutputReply, RpcError> {
    let needle = Needle::parse(&params)?;
    let events = router.events()?.clone();
    let pane = pane::resolve(router, &params.target).await?;
    let feed = wiring(router, pane).await?.frames;

    let waiter = Waiter::new(
        events.bus(),
        ScreenHas {
            frames: feed,
            pane,
            needle,
        },
    );
    let line = match poll(waiter, params.timeout_ms, events.cancel()).await {
        Polled::Satisfied(line) => Some(line),
        Polled::TimedOut => None,
        Polled::Cancelled => return Err(cancelled()),
    };
    Ok(proto::WaitOutputReply {
        pane,
        matched: line.is_some(),
        line,
        // Always absent, and not by omission: only the visible grid is ever
        // matched, and a row still on the grid has not been committed to
        // history, so it has no stable id yet. The field exists for the day a
        // match can be cited afterwards.
        row: None::<RowId>,
        seq: events.bus().head(),
    })
}

// ------------------------------------------------------------- the predicates

/// A pane's agent has reached a state.
struct StatusIs {
    view: StatusView,
    pane: PaneId,
    want: AgentState,
}

impl StatePredicate for StatusIs {
    type Output = AgentSnapshot;

    fn evaluate(&mut self) -> Option<Self::Output> {
        self.view
            .get(self.pane)
            .filter(|status| status.state == self.want)
    }

    /// Agent transitions for this pane, and nothing else.
    ///
    /// A filter only: returning `false` can delay the next evaluation, never
    /// change the outcome, because a gap re-evaluates regardless of it.
    fn interested(&self, event: &Event) -> bool {
        match event {
            Event::AgentStatus { pane, .. }
            | Event::AgentIdentified { pane, .. }
            | Event::PaneExited { pane, .. } => *pane == self.pane,
            _ => false,
        }
    }
}

/// What `pane.wait_output` is looking for on the screen.
enum Needle {
    /// A literal substring.
    Text(String),
    /// A pattern, compiled once at the start of the call.
    Pattern(Box<Regex>),
}

impl Needle {
    /// Read the needle out of the parameters.
    ///
    /// Exactly one of `match` and `regex` is given; both or neither is
    /// `INVALID_PARAMS`, as the params type documents.
    fn parse(params: &proto::WaitOutputParams) -> Result<Self, RpcError> {
        match (&params.match_text, &params.regex) {
            (Some(text), None) => Ok(Self::Text(text.clone())),
            (None, Some(pattern)) => Regex::new(pattern)
                .map(|regex| Self::Pattern(Box::new(regex)))
                .map_err(|err| {
                    RpcError::new(
                        RpcError::INVALID_PARAMS,
                        format!("pane.wait_output: {pattern:?} is not a regex: {err}"),
                    )
                }),
            (Some(_), Some(_)) => Err(RpcError::new(
                RpcError::INVALID_PARAMS,
                "pane.wait_output takes `match` or `regex`, not both",
            )),
            (None, None) => Err(RpcError::new(
                RpcError::INVALID_PARAMS,
                "pane.wait_output needs one of `match` or `regex`",
            )),
        }
    }

    /// Whether `line` matches.
    fn matches(&self, line: &str) -> bool {
        match self {
            Self::Text(text) => line.contains(text.as_str()),
            Self::Pattern(regex) => regex.is_match(line),
        }
    }
}

/// A pane's visible grid carries a line the needle matches.
struct ScreenHas {
    frames: SnapshotFeed,
    pane: PaneId,
    needle: Needle,
}

impl StatePredicate for ScreenHas {
    type Output = String;

    fn evaluate(&mut self) -> Option<Self::Output> {
        let frame = self.frames.latest();
        frame
            .tail(frame.rows())
            .map(|row| row.line().trim_end())
            .find(|line| self.needle.matches(line))
            .map(str::to_owned)
    }

    /// Damage on this pane is the only thing that can change its screen.
    ///
    /// `PaneExited` is included so a wait on a dead pane is re-evaluated once
    /// more rather than sitting on a grid nothing will ever repaint; it still
    /// answers from the screen, which is what the contract promises.
    fn interested(&self, event: &Event) -> bool {
        match event {
            Event::PaneDamage { pane, .. }
            | Event::PaneResized { pane, .. }
            | Event::PaneExited { pane, .. } => *pane == self.pane,
            _ => false,
        }
    }
}

// ----------------------------------------------------------------- plumbing

/// Whether `Core` still has a live process for `pane`.
///
/// A refused wiring is the answer: `Core` serves one only from its live host
/// map, and removes the host as it folds the exit report. The refusal is told
/// apart from a failing session by its code — `INVALID_PARAMS` means "that pane
/// has no process", and anything else means the call itself did not get an
/// answer, which is not a fact about the pane and must not be read as one.
async fn is_gone(router: &Router, pane: PaneId) -> Result<bool, RpcError> {
    match wiring(router, pane).await {
        Ok(_) => Ok(false),
        Err(err) if err.code == RpcError::INVALID_PARAMS => Ok(true),
        Err(err) => Err(err),
    }
}

/// A pane's live plumbing.
async fn wiring(router: &Router, pane: PaneId) -> Result<PaneWiring, RpcError> {
    router
        .call(|reply| CoreCommand::Stream(StreamCall::Wiring { pane, reply }))
        .await
}

/// The session's state tree, for the one existence check every wait makes.
async fn session_state(router: &Router) -> Result<session::StateReply, RpcError> {
    router
        .call(|reply| {
            CoreCommand::Session(SessionCall::State {
                params: session::StateParams::default(),
                reply,
            })
        })
        .await
}
