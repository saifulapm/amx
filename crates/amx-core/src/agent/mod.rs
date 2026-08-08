//! The agent layer's shared vocabulary (04 §5).
//!
//! Everything here is spoken by the server, the client and the wire alike:
//! what an agent *is* ([`AgentKind`]), how much its hook system actually covers
//! ([`CoverageClass`]), what a pane's agent is doing ([`AgentState`]), why it
//! last changed ([`StatusCause`]), and what a client or a snapshot sees of all
//! of it ([`AgentSnapshot`]). The resume refs live beside it in [`refs`].
//!
//! The fusion machine itself is not here — it is server-side, in
//! `amx-server/src/agent/fusion.rs`, because it has no client. What *is* here
//! is the part of it that must be spelled the same on both sides of the socket,
//! and the one piece of its precedence that is safer as a type than as a
//! convention: [`ExitAuthority`].
//!
//! # The measurement this vocabulary encodes
//!
//! `docs/notes/hook-coverage.md` (V01) measured Claude Code 2.1.224 and Codex
//! CLI 0.147.0 and put both in class [`CoverageClass::Edges`]:
//!
//! > Entry edges are complete, fast (median 26 ms) and precede the UI; **every
//! > exit-by-user is silent** — Esc during generation, Esc during a tool call,
//! > a dialog answered "No", and a dialog cancelled with Esc all emit nothing
//! > at all.
//!
//! So an `edges` agent's *entries* may be taken from a hook and its *exits* may
//! not, ever. That asymmetry is the whole of the fusion design, and it is why
//! [`CoverageClass::exit_is_screen_owned`] exists as a predicate and
//! [`ExitAuthority::from_hook`] returns [`Option`].
//!
//! # Task ownership
//!
//! V02 froze these types. **V03** fills the registry that supplies a
//! [`CoverageClass`] per agent, **V04** the tracker that consumes it, **V07**
//! the tier-3 probe that produces an [`Activity`], **V08** the hub that holds
//! the [`AgentSnapshot`]s, and **V15** the resume path that reads a
//! [`SessionRef`] back off disk.

pub mod refs;
pub mod status;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::event::Seq;
use crate::id::WorkspaceId;

pub use refs::{RefError, RefKind, RefSource, SessionRef, StartSource};
pub use status::{Activity, AgentState, CoverageClass, ExitAuthority, StatusCause};

/// Which agent a pane is running, by its registry stanza id.
///
/// The id and nothing else: 04 §5 makes `agents.toml` "the *only* place an
/// agent is defined", so a kind here is a *reference* into that file and never
/// a second definition of one. Whether a given kind names a stanza that exists
/// is the registry's question (V03), not this type's — a snapshot restored from
/// a build that shipped an agent this build dropped must still parse, and it
/// does, degrading to "no stanza" rather than to "unreadable file".
///
/// The character set is validated so a kind can be spelled into a file name, a
/// hook source (`amx:<id>`, see [`RefSource`]) or an argv without quoting, and
/// so a hand-edited `session.json` cannot smuggle a path or a shell fragment in
/// through this field. Deserialization goes through the same check as
/// [`AgentKind::new`] — there is no way to construct one that skipped it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentKind(String);

impl AgentKind {
    /// The longest an id may be.
    ///
    /// Far above anything a registry would use; it exists so an id read off
    /// disk cannot be megabytes long.
    pub const MAX_LEN: usize = 64;

    /// The id `text` names, if it is a well-formed one.
    ///
    /// Well-formed is: non-empty, at most [`AgentKind::MAX_LEN`] bytes, and
    /// made of ASCII lowercase letters, digits, `-` and `_` only.
    ///
    /// # Errors
    ///
    /// [`AgentKindError`] naming which of those it failed.
    pub fn new(text: impl Into<String>) -> Result<Self, AgentKindError> {
        let text = text.into();
        if text.is_empty() {
            return Err(AgentKindError::Empty);
        }
        if text.len() > Self::MAX_LEN {
            return Err(AgentKindError::TooLong { len: text.len() });
        }
        if let Some(bad) = text
            .chars()
            .find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '-' | '_'))
        {
            return Err(AgentKindError::BadCharacter { found: bad });
        }
        Ok(Self(text))
    }

    /// The id, as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for AgentKind {
    type Error = AgentKindError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

impl From<AgentKind> for String {
    fn from(kind: AgentKind) -> Self {
        kind.0
    }
}

/// Why a string is not an [`AgentKind`].
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum AgentKindError {
    /// The id was empty.
    #[error("an agent id cannot be empty")]
    Empty,
    /// The id was longer than [`AgentKind::MAX_LEN`].
    #[error("an agent id is at most {max} bytes, got {len}", max = AgentKind::MAX_LEN)]
    TooLong {
        /// How long it was.
        len: usize,
    },
    /// The id held something outside `[a-z0-9_-]`.
    #[error("an agent id holds only [a-z0-9_-], found {found:?}")]
    BadCharacter {
        /// The first offending character.
        found: char,
    },
}

/// The per-spawn secret a pane's hook reports must carry.
///
/// D-M2-4, quoted so nobody mistakes it for one: **this is not a security
/// boundary.** "The socket is 0600 and any same-user process can already drive
/// every pane — it is a *misattribution* guard: a stale hook config in a nested
/// or foreign session, or a pane id recycled across restarts, reports into the
/// void instead of into the wrong pane's status."
///
/// Minted per pane spawn, injected as `AMX_HOOK_TOKEN` (V07), remembered by
/// `AgentHub`, and carried back on every `agent.report`. A report whose token
/// does not match the pane it names is dropped and counted — V08's
/// `token_mismatch_is_dropped_and_counted`.
///
/// Opaque on purpose: the value is bytes the emitter copies from its
/// environment to the wire without reading, so the type carries no structure
/// for anything to depend on. It is `PartialEq` because comparing one is the
/// entire point.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HookToken(String);

impl HookToken {
    /// Adopt a token that came from an environment variable or the wire.
    ///
    /// Deliberately total: a malformed token is not an error to report, it is a
    /// token that will not match. Minting is V07's, which owns the randomness.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The token, as text, for writing into a child's environment.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Redacted: a token that appeared in a log line would outlive the pane that
/// minted it, in a file the user did not expect to hold one.
impl std::fmt::Debug for HookToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookToken(<redacted>)")
    }
}

/// Milliseconds since the Unix epoch: how every wall-clock instant is spelled.
///
/// Absolute, never an age, and `docs/11-m4-plan.md` D-M4-4 is why. The bus
/// sequence a status transition already carries
/// ([`AgentSnapshot::transition_seq`]) cannot be rendered as `4m`, so a wall
/// clock has to travel — and an age computed against the *reader's* clock is
/// wrong by the skew between two machines, which a remote attach is by
/// definition. So the instant travels absolute and every reply that carries one
/// carries the server's own `now` beside it: a surface renders `now − since`
/// from inside one reply and never against a clock of its own.
pub type EpochMillis = u64;

/// A workspace, named — the half of an agent's identity that is not the pane.
///
/// `docs/10-attention-surfaces.md` §D15 requires the attention events and every
/// `agent.list` row to say which project an agent belongs to, so a notifier can
/// render `api/backend blocked (permission_dialog)` with no follow-up query.
/// The id is what a consumer acts on; the label is what a person reads, and it
/// is optional because a workspace need not have one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentWorkspace {
    /// The workspace.
    pub id: WorkspaceId,
    /// Its label, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl AgentWorkspace {
    /// A workspace whose label is not known here.
    #[must_use]
    pub const fn unnamed(id: WorkspaceId) -> Self {
        Self { id, name: None }
    }
}

/// One pane's agent status, as the hub holds it and the wire reports it.
///
/// This is the value in `AgentHub`'s `StatusView` (`docs/08-m2-plan.md` §3), the
/// per-pane `agent` field on `session.state`, and what `agent.prompt --wait`
/// evaluates its predicate against. One shape for all three on purpose: a wait
/// that read a different projection of the truth than the status line renders
/// is how the two come to disagree.
///
/// [`transition_seq`](Self::transition_seq) is the field that makes a wait
/// honest. V13's `prompt_wait_uses_transition_seq_so_a_prior_blocked_state_
/// cannot_satisfy_it` is exactly this: a wait submitted at seq *N* requires a
/// transition later than *N*, so a pane that was already `Blocked` before the
/// prompt was sent does not satisfy it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Which agent this is, once identity has confirmed one.
    ///
    /// `None` for a pane whose foreground program has not been identified —
    /// which is every pane running a plain shell, and a pane running an agent
    /// amx has not recognised yet. A `None` kind and a
    /// [`Busy`](AgentState::Busy)/[`Quiet`](AgentState::Quiet) state travel
    /// together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AgentKind>,
    /// What it is doing.
    pub state: AgentState,
    /// What moved it there.
    pub cause: StatusCause,
    /// The bus sequence of the transition into [`state`](Self::state).
    ///
    /// Not "when this snapshot was taken": the *transition*'s sequence, held
    /// until the next transition replaces it.
    pub transition_seq: Seq,
    /// This pane's place in the attention queue, counted from zero, when it is
    /// on it.
    ///
    /// Present exactly when [`state`](Self::state) is
    /// [`Blocked`](AgentState::Blocked). The queue is ordered by block time and
    /// a re-block goes to the tail (D-M2-8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<u32>,
    /// The conversation this pane can be resumed into, once one is known.
    ///
    /// Captured from every `SessionStart`-equivalent report, not just the
    /// first: V01 §3 M8 measured `/clear` minting a **new** session id inside
    /// one process, so a ref captured before it is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<SessionRef>,
    /// The name of whatever last moved this pane into
    /// [`state`](Self::state) — the detector's own identifier, never a
    /// summary of it.
    ///
    /// `docs/11-m4-plan.md` D-M4-3 chose a detector's name over a second
    /// vocabulary, because the detectors already name themselves and a
    /// hand-maintained enum beside the manifests is a table that goes stale the
    /// first time a rule is added. So this carries the winning manifest rule's
    /// name for a screen-owned state (`permission_dialog`, `prompt_box_idle`,
    /// `footer_interrupt_hint_working`), the hook event's name for a
    /// hook-asserted entry (`PermissionRequest`), and nothing at all for a
    /// tier-3 [`Busy`](AgentState::Busy)/[`Quiet`](AgentState::Quiet), which no
    /// named rule produced.
    ///
    /// Additive and optional under the R-M1-8 terms every field below the label
    /// on the wire follows: absent when nothing named the transition, so a peer
    /// built before M4 reads exactly the bytes it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When this pane entered [`state`](Self::state), on a wall clock.
    ///
    /// The renderable half of [`transition_seq`](Self::transition_seq), which
    /// is a bus sequence and cannot be shown to a person as `4m`. D-M4-4 makes
    /// it absolute — see [`EpochMillis`] for why an age would have been wrong
    /// across a remote attach.
    ///
    /// `None` is honest rather than convenient: a pane whose status was
    /// re-derived rather than observed entering has no entry edge to report,
    /// and a zero would read as 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<EpochMillis>,
}

impl AgentSnapshot {
    /// The snapshot of a pane nothing has been detected in yet.
    #[must_use]
    pub const fn unidentified(transition_seq: Seq) -> Self {
        Self {
            kind: None,
            state: AgentState::Quiet,
            cause: StatusCause::Probe,
            transition_seq,
            attention: None,
            session_ref: None,
            reason: None,
            since: None,
        }
    }
}
