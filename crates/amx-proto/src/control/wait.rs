//! `wait`, `pane.wait_output` and `events.subscribe` payloads.
//!
//! Three rows that share one contract, 04 §2's:
//!
//! > **Waits are state predicates, not event predicates.** `amx wait --until
//! > blocked` evaluates the predicate against current state at subscribe time,
//! > then uses events as notification; on any `gap` it re-evaluates state before
//! > consuming events after `to`. A transition falling inside a gap can
//! > therefore never hang a wait.
//!
//! [`Waiter`](amx_core::event::Waiter) already encodes that sequencing and has
//! had no consumers since M0; these rows are its first. The reply types below
//! all carry a [`Seq`] for the same reason every state-query reply does: "every
//! state-query response (and the subscribe reply) carries the bus sequence
//! number at which it was captured", which is what lets an external consumer
//! that saw a gap re-query and resume without racing new transitions.
//!
//! # Task ownership
//!
//! V02 froze these shapes. **V11** fills all three, plus the `amx events
//! --json` CLI that consumes the subscription.

use amx_core::agent::AgentSnapshot;
use amx_core::{PaneId, RowId, Seq};
use serde::{Deserialize, Serialize};

use crate::control::pane::PaneTarget;

/// What `wait` waits for.
///
/// 04 §2 fixes this list and its reading: "Wait targets: `--until blocked|idle`
/// are agent statuses; `--until exited` is the `pane.exited` event (process
/// end). **There is no `done` agent status.**"
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    /// The pane's agent is waiting on the user.
    Blocked,
    /// The pane's agent is at its prompt.
    Idle,
    /// The pane's process ended.
    Exited,
}

/// Parameters of `wait`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WaitParams {
    /// What to wait for.
    pub until: WaitUntil,
    /// Which pane, by UUID or by unique label.
    pub target: PaneTarget,
    /// How long to wait. Absent waits indefinitely — until the connection dies,
    /// which is the only other thing that ends it.
    ///
    /// A parameter, never a poll interval: the predicate is evaluated once at
    /// subscribe time and then on the events that could have changed it, so
    /// there is nothing here for a timer to do. The hygiene suite rejects a nap
    /// in the implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Reply to `wait`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WaitReply {
    /// The pane waited on.
    pub pane: PaneId,
    /// Whether the condition held before the timeout.
    pub satisfied: bool,
    /// The pane's agent status when the wait ended, for a status wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
    /// The child's exit status, for [`WaitUntil::Exited`] — `None` when it was
    /// signalled rather than exited normally, matching
    /// [`Event::PaneExited`](amx_core::Event::PaneExited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    /// The bus sequence the terminal condition held at.
    pub seq: Seq,
}

/// Parameters of `pane.wait_output`.
///
/// Exactly one of [`match_text`](Self::match_text) and [`regex`](Self::regex)
/// is given; both or neither is `INVALID_PARAMS`.
///
/// **This matches the screen, not a byte stream.** The predicate runs over the
/// pane's visible grid at evaluation time and is re-run per `PaneDamage` batch,
/// so a line that scrolled past between two batches was never on the screen
/// when anything looked. That is `pane.read` and history territory, and the
/// contract says so here rather than in a bug report.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WaitOutputParams {
    /// Which pane.
    pub target: PaneTarget,
    /// A literal substring to wait for.
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_text: Option<String>,
    /// A regular expression to wait for, compiled once per call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// How long to wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Reply to `pane.wait_output`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WaitOutputReply {
    /// The pane watched.
    pub pane: PaneId,
    /// Whether the pattern appeared before the timeout.
    pub matched: bool,
    /// The whole visible line that matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// That line's stable history id, once it has one.
    ///
    /// A visible row that has not scrolled into history yet has no id, which is
    /// most of them — the field is what makes a match citable afterwards when
    /// it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<RowId>,
    /// The bus sequence the match was observed at.
    pub seq: Seq,
}

/// Parameters of `events.subscribe`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SubscribeParams {
    /// Resume after this sequence, for a consumer that saw a gap and re-queried
    /// state.
    ///
    /// Absent subscribes from the bus head. A sequence the replay buffer has
    /// already dropped is not an error: the subscription's first delivery is a
    /// [`Gap`](amx_core::Delivery::Gap) covering what was missed, which is the
    /// same answer a subscriber that fell behind gets. Loss is visible, always.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<Seq>,
}

/// Reply to `events.subscribe`.
///
/// One field, and it is the whole contract: 04 §2 requires the subscribe reply
/// to carry the sequence it was taken at so "an external `amx events --json`
/// consumer that sees a gap re-queries state and resumes from the returned seq
/// without racing new transitions".
///
/// Deliveries follow on the control channel as JSON-RPC notifications with
/// method [`EVENT_METHOD`], one per [`Delivery`](amx_core::Delivery) — envelope
/// and `gap` alike. The subscription dies with the connection: cursors are
/// per-connection state, so there is nothing to persist and nothing to leak.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SubscribeReply {
    /// The bus sequence at subscribe time. The first delivery is the one after
    /// it.
    pub seq: Seq,
}

/// The JSON-RPC method name every event delivery arrives under.
///
/// A notification, never a request: the server does not want an answer and a
/// consumer that never reads one must not stall the connection. The params are
/// a [`Delivery`](amx_core::Delivery) verbatim, so a `gap` is as ordinary a
/// message as an event and cannot be skipped by a consumer that only handles
/// what it recognises.
pub const EVENT_METHOD: &str = "event";
