//! `agent.report`: the emitter's contract, and the vocabulary it speaks.
//!
//! One row, and the enums an agent's own hook system names its events with.
//! Split out of [`super`] by X02 (`docs/11-m4-plan.md` R-M4-5); the types are
//! V02's, moved and not changed. It sits alone because it changes for a reason
//! nothing else here shares — an agent shipping a new hook event — and because
//! it is the one payload written by a process that is not amx's client.
//!
//! # Task ownership
//!
//! V02 froze these shapes; **V09** fills `agent.report`.

use std::path::PathBuf;

use amx_core::agent::{AgentKind, HookToken, RefSource};
use amx_core::{PaneId, Seq};
use serde::{Deserialize, Serialize};

/// One lifecycle event, as the agent's hook system named it.
///
/// The variants are the events V01 measured firing on Claude Code 2.1.224 and
/// Codex CLI 0.147.0; [`Other`](Self::Other) carries anything else verbatim.
/// That fallback is the skew rule applied to a surface amx does not own: these
/// tools ship weekly, and an emitter that refused an event it had never heard
/// of would start dropping reports the week a new one is added.
///
/// The names are the agents' own PascalCase spellings, and both tools use the
/// same ones — V01 §4 measured Codex's payloads as "Claude-shaped", down to the
/// event keys.
///
/// Deliberately **not** subscribed by either shipped stanza, and so never seen
/// here in practice: `PostToolBatch` (V01 measured it firing for calls that
/// never ran, so it asserts nothing), `PermissionDenied` (never fired once,
/// on any deny path), `PreCompact`/`PostCompact` (compaction does not change a
/// pane's status and the ref survives it). They are absent from this enum for
/// the same reason: a variant nothing produces is an arm nothing tests.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum HookEvent {
    /// A session began. Carries the ref (D-M2-7), and amx takes it from
    /// **every** one, not just the first: V01 §3 M8 measured `/clear` minting a
    /// new session id inside one process.
    SessionStart,
    /// A session ended.
    SessionEnd,
    /// The user submitted a prompt. The dependable `Working` entry edge on both
    /// agents.
    UserPromptSubmit,
    /// A turn ended by itself. Never an exit edge for an `edges` agent — see
    /// [`ExitAuthority`](amx_core::agent::ExitAuthority).
    Stop,
    /// A tool call is about to run. A `Working` edge on Claude Code; on Codex
    /// it is *conditional* (V01 §4 measured a plain file read producing only
    /// `PostToolUse`), so it corroborates rather than drives.
    PreToolUse,
    /// A tool call finished.
    PostToolUse,
    /// A tool call failed on its own. Not what an interrupt produces: V01 §3 M1
    /// measured an Esc during a running tool call emitting nothing at all.
    PostToolUseFailure,
    /// A permission dialog is about to paint. Measured 8–14 ms *ahead* of the
    /// paint, which is why `Blocked` may be entered from it outright.
    PermissionRequest,
    /// A subagent turn began. Carries an `agent_id`; see [`ReportScope`].
    SubagentStart,
    /// A subagent turn ended. The hazard: V01 §3 M4 measured an anonymous one
    /// arriving 1.9–3.0 s **after** the parent's `Stop` on essentially every
    /// tool-using turn.
    SubagentStop,
    /// A wait has gone on long enough for the agent to say so — 6 s for a
    /// permission prompt, 60 s for an idle one, distinguished by the payload.
    /// A backstop, never an edge: too slow to drive a status line, but a free
    /// contradiction of a `Working` state a silent interrupt left behind.
    Notification,
    /// Something this build has never heard of, carried through unread.
    Other(String),
}

impl HookEvent {
    /// This event's name, as the agent spells it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PermissionRequest => "PermissionRequest",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::Notification => "Notification",
            Self::Other(name) => name,
        }
    }
}

impl From<String> for HookEvent {
    fn from(name: String) -> Self {
        match name.as_str() {
            "SessionStart" => Self::SessionStart,
            "SessionEnd" => Self::SessionEnd,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "Stop" => Self::Stop,
            "PreToolUse" => Self::PreToolUse,
            "PostToolUse" => Self::PostToolUse,
            "PostToolUseFailure" => Self::PostToolUseFailure,
            "PermissionRequest" => Self::PermissionRequest,
            "SubagentStart" => Self::SubagentStart,
            "SubagentStop" => Self::SubagentStop,
            "Notification" => Self::Notification,
            _ => Self::Other(name),
        }
    }
}

impl From<HookEvent> for String {
    fn from(event: HookEvent) -> Self {
        match event {
            HookEvent::Other(name) => name,
            other => other.as_str().to_owned(),
        }
    }
}

/// Whose turn a hook event is about.
///
/// The single most load-bearing field in this module. 04 §5: "Subagent-scoped
/// events never override the parent turn's state (herdr's 'never revive an idle
/// pane' lesson, kept as a rule of the fusion machine)."
///
/// V01 §3 M4 measured the discriminator and the hazard together. `agent_id` is
/// present on every subagent-scoped event and absent from every parent event —
/// across a whole run, all 44 `Stop` payloads lacked one and all 8
/// `SubagentStop` payloads carried one. And an *anonymous* `SubagentStop`, with
/// an `agent_id` no `SubagentStart` ever announced and a transcript path that
/// does not exist, arrives about two seconds after the parent's `Stop` on
/// essentially every tool-using turn. A machine that treated it as a parent
/// edge would churn a pane's status after every single tool call.
///
/// So the emitter *tags* rather than filters (D-M2-4 — herdr baked the filter
/// into installed scripts and paid for it with reinstalls), and the fusion
/// machine reads this field.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ReportScope {
    /// The pane's own agent. The only scope that may move the pane's state.
    #[default]
    Parent,
    /// A subagent of it. Never moves the pane's state — not on entry, not on
    /// exit.
    Subagent {
        /// The subagent's id, as the payload carried it.
        agent_id: String,
        /// Its type, when the payload named one. Empty on the anonymous
        /// `SubagentStop` described above, which is why this is optional.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
    },
}

impl ReportScope {
    /// Whether this scope may move the pane's own agent state.
    ///
    /// The rule, as one predicate, so the fusion machine and the exit test
    /// cannot read it differently.
    #[must_use]
    pub const fn is_parent(&self) -> bool {
        matches!(self, Self::Parent)
    }
}

/// Parameters of `agent.report`: one hook invocation, forwarded.
///
/// Sent by `amx _hook` (V09) and by nothing else. D-M2-4 sets the policy this
/// shape encodes: **the emitter forwards, tagged with what it knows, and
/// filters nothing but malformed input.** Every judgement — which events are
/// edges, what a subagent scope means, whether a `Stop` may close a turn —
/// lives in the fusion machine, so changing it means shipping a binary rather
/// than reinstalling hooks on every machine.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReportParams {
    /// The pane the hook believes it is running in, from `AMX_PANE_ID`.
    pub pane: PaneId,
    /// That pane's spawn token, from `AMX_HOOK_TOKEN`.
    ///
    /// A mismatch is dropped and counted, never answered with an error: a hook
    /// must never break or slow a turn, and a stale config reporting into the
    /// void is exactly the outcome the token exists to produce.
    pub token: HookToken,
    /// Which agent the emitter was installed for — `amx _hook claude`.
    pub agent: AgentKind,
    /// The hook path this came from, `amx:<agent-id>`.
    ///
    /// Carried *beside* [`agent`](Self::agent) rather than derived from it: the
    /// two agreeing is D-M2-7's source allowlist, checked against the stanza at
    /// report time and twice more before an argv is ever planned.
    pub source: RefSource,
    /// The event, as the agent named it.
    pub event: HookEvent,
    /// The emitter's own monotonic sequence: nanoseconds since the epoch.
    ///
    /// Nanoseconds, not milliseconds, and V01 §3 M9 is why: hooks subscribed to
    /// one event run in parallel and their processes start a median of 1.0 ms
    /// apart, with a measured minimum of 0.1 ms. A millisecond counter would
    /// tie, and a tie is an ordering the server has to guess at.
    pub seq: u64,
    /// Whose turn this is about.
    #[serde(default, skip_serializing_if = "ReportScope::is_parent")]
    pub scope: ReportScope,
    /// The agent's own session id, when the payload carried one.
    ///
    /// Forwarded raw. Turning it into a
    /// [`SessionRef`](amx_core::agent::SessionRef) needs the stanza's
    /// `ref_kind`, which the emitter does not have and the server does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The transcript file the payload named, when it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<PathBuf>,
    /// `SessionStart`'s `source`: `startup`, `resume`, `clear`, `compact`.
    ///
    /// The only warning that a ref captured moments ago is now stale — V01 §3
    /// M8 measured `/clear` replacing the session id inside one process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_source: Option<String>,
    /// The tool a `PreToolUse`/`PermissionRequest` is about.
    ///
    /// Enough for a status line to say *what* an agent is blocked on rather
    /// than merely that it is (V01 §3 M3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// `Notification`'s `notification_type`: `permission_prompt` or
    /// `idle_prompt`, which is what makes the backstop worth subscribing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<String>,
}

/// Reply to `agent.report`: an acknowledgement, and nothing an emitter acts on.
///
/// The emitter exits 0 whatever this says — a hook must never break a turn — so
/// the fields exist for tests and for `amx _hook` run by hand. `accepted:
/// false` is the token-mismatch case, which V09's tests use to tell "silently
/// succeeded" from "silently did nothing".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReportReply {
    /// Whether the report reached a tracked pane.
    pub accepted: bool,
    /// The bus head at reply time.
    pub seq: Seq,
}
