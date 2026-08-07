//! `agent.*` payloads.
//!
//! Five rows of `docs/08-m2-plan.md` §4: `agent.report` (the hook path),
//! `agent.start`, `agent.prompt`, `agent.explain` and `agent.next`. `agent
//! rename` and `agent read` are deliberately *not* here — D-M2-9 delivers them
//! by resolving the agent's target and calling the existing `pane.rename` /
//! `pane.read` rows, because the agent's name **is** the pane's label. The CLI
//! surface matches 04 §5's verb list; the method table stays one row per
//! behavior (R-M2-10 flags the reading in case 04 meant a distinct semantic —
//! it names none).
//!
//! # Task ownership
//!
//! V02 froze these shapes. **V09** fills `agent.report`, **V13** `agent.start`
//! and `agent.prompt`, **V06** `agent.explain`, **V08** `agent.next`.

use std::path::PathBuf;

use amx_core::agent::{AgentKind, AgentSnapshot, AgentState, HookToken, RefSource};
use amx_core::{PaneId, Seq, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::control::pane::PaneTarget;

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

/// Parameters of `agent.start`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StartParams {
    /// The name to give it, which becomes the pane's label (D-M2-9).
    ///
    /// One namespace, not two: amx already has a persisted, renameable,
    /// user-visible per-pane name, and inventing a second would mean a second
    /// rename verb, a second persistence field and a picker showing two names.
    pub name: String,
    /// Which agent to start, by registry id or alias.
    pub kind: AgentKind,
    /// Arguments appended to the stanza's `start` argv.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Where to start it. Absent means the same default a split takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Which workspace to put it in. Absent means the focused one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// How long to wait for readiness before reporting failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// How an `agent.start` ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    /// The agent owns the pane's foreground and has been observed idle.
    Ready,
    /// The timeout expired first.
    ///
    /// Reported as a failure, with the pane **left running** for inspection —
    /// herdr's semantics, and V13's `start_timeout_reports_failure_but_leaves_
    /// the_pane_running`. V01 §4 measured why the timeout has to be generous:
    /// Codex emits no hook at all until the first prompt, so its readiness is
    /// tier-2 and tier-3 evidence only.
    TimedOut,
}

/// Reply to `agent.start`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StartReply {
    /// The pane the agent runs in. Present either way: a timed-out start still
    /// made a pane, and the caller is told which one.
    pub pane: PaneId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// Whether it came up.
    pub readiness: Readiness,
    /// What the pane's agent status was when this returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
    /// The bus sequence this reply was captured at.
    pub seq: Seq,
}

/// What `agent.prompt` waits for after submitting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptWait {
    /// Return as soon as the text is submitted.
    #[default]
    None,
    /// Return when the agent next blocks on the user.
    Blocked,
    /// Return when the agent next goes idle.
    Idle,
}

/// Parameters of `agent.prompt`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PromptParams {
    /// Which agent, by pane UUID or by unique label among agent panes.
    pub target: PaneTarget,
    /// The prompt, submitted through the same bracketed-paste-aware path
    /// `pane.run` uses.
    pub text: String,
    /// What to wait for.
    #[serde(default, skip_serializing_if = "PromptWait::is_none")]
    pub wait: PromptWait,
    /// How long to wait for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl PromptWait {
    /// Whether this is the no-wait case.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Reply to `agent.prompt`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PromptReply {
    /// The pane the prompt went to.
    pub pane: PaneId,
    /// Its status when this returned. `None` when the wait timed out with the
    /// pane no longer tracked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
    /// Whether the wait was satisfied, or `true` outright for
    /// [`PromptWait::None`].
    pub satisfied: bool,
    /// The bus sequence the prompt was submitted at.
    ///
    /// The wait's floor: the predicate requires a transition strictly later
    /// than this, so a pane that was *already* blocked when the prompt was sent
    /// cannot satisfy a `--wait blocked`. V13's acceptance test is exactly this
    /// case and it is the one that matters.
    pub submitted_seq: Seq,
}

/// Parameters of `agent.explain`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExplainParams {
    /// Which pane's detection to explain.
    pub target: PaneTarget,
}

/// One tier-2 rule's verdict, with the evidence for it.
///
/// 04 §5 keeps herdr's `agent explain` debuggability, and D-M2-3 says why in
/// one line: a detection you cannot interrogate is a detection you cannot fix.
/// So every rule reports, not only the winner.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RuleVerdict {
    /// The rule's name, as the manifest spells it.
    pub rule: String,
    /// Its priority; the highest matching rule wins, ties by file order.
    pub priority: i32,
    /// The region it looked at: `whole_recent`, `bottom_lines(N)`,
    /// `bottom_non_empty_lines(N)`, `title`.
    pub region: String,
    /// Whether its gate tree matched.
    pub matched: bool,
    /// The state it asserts, or `None` for a `skip_state_update` rule — the
    /// assert-nothing third outcome that freezes the previous state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asserts: Option<AgentState>,
    /// What in the region made it match or fail, in one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// Reply to `agent.explain`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExplainReply {
    /// The pane explained.
    pub pane: PaneId,
    /// Which agent's manifest was used, if the pane has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AgentKind>,
    /// Where the manifest came from — a bundled asset, or an override file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Its version, as the manifest declares it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_version: Option<String>,
    /// The rule that won, if any did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    /// The text the rules were evaluated against, as they saw it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region_preview: Vec<String>,
    /// Every rule, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleVerdict>,
    /// The pane's fused status — what the machine concluded once the hook tier
    /// had its say too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
}

/// Parameters of `agent.next`.
///
/// Empty, and a struct rather than a unit for the reason `PingParams` is: a
/// field added later must not change the wire shape from `null` to an object.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct NextParams {}

/// Reply to `agent.next`: the head of the attention queue, now focused.
///
/// There is deliberately no "query the queue" method — `session.state` carries
/// `attention` in queue order and *is* the query (D-M2-8). This row focuses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct NextReply {
    /// The pane focused, or `None` when the queue was empty — an honest empty,
    /// not an error (V08's `agent_next_focuses_the_head_and_reports_empty_
    /// honestly`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneId>,
    /// The workspace switched to on the way, when the head was elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// How many panes are still waiting, this one included.
    pub waiting: u32,
    /// The bus sequence the focus held at.
    pub seq: Seq,
}
