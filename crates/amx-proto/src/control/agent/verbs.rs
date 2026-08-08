//! The four things a person asks an agent to do, and what comes back.
//!
//! `agent.start`, `agent.prompt`, `agent.explain` and `agent.next` — 04 §5's
//! verb list minus the two D-M2-9 delivers through `pane.*`. Split out of
//! [`super`] by X02 before M4's payloads pushed that file past the soft budget
//! (`docs/11-m4-plan.md` R-M4-5); the types are V02's, moved and not changed.

use std::path::PathBuf;

use amx_core::agent::{AgentKind, AgentSnapshot, AgentState};
use amx_core::{PaneId, Seq, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::control::pane::PaneTarget;

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
/// It was empty, and a struct rather than a unit for the reason `PingParams`
/// is: a field added later must not change the wire shape from `null` to an
/// object. M4 is that later, and the shape held.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct NextParams {
    /// Cycle only this workspace's blocked agents.
    ///
    /// D15's scoped cycling: the attention queue itself stays global and
    /// block-time ordered — fairness is the server's job — and this narrows
    /// only *which* of its entries this call will focus, so clearing one
    /// project's queue never yanks focus across projects. A scoped call never
    /// reorders the queue, and an empty scope is an honest empty reply exactly
    /// as the unscoped call already is.
    ///
    /// The workspace by id, like every other `workspace` parameter on the
    /// table. A label is resolved by whoever is holding `session.state`, which
    /// the CLI already is; putting a name-or-id target here would put a second
    /// resolver in the server for a namespace that has exactly one method of
    /// resolution today.
    ///
    /// Additive and optional: absent behaves exactly as this row did before the
    /// field existed, which is what keeps the M2 goldens unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
}

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
