//! One hook report, reduced — and the precedence table that judges it.
//!
//! The reduction is where D-M2-4's division of labour lands: `amx _hook`
//! forwards every event it is installed for, tagged with what it knows, and
//! *policy lives here*. herdr baked the filtering into installed scripts, so
//! changing policy meant reinstalling hooks on every machine; changing it here
//! is shipping a binary.
//!
//! [`precedence`] is deliberately a free function of `(class, edge)` and
//! nothing else. It cannot see the pane's current state, so it cannot express
//! "leave `Working` if …" — deciding what a held state is allowed to do with an
//! edge belongs to [`Tracker`](super::Tracker), and keeping the table blind to
//! the pane is what makes it readable as the measurement it encodes.

use amx_core::agent::{AgentState, CoverageClass, ExitAuthority, RefKind, SessionRef};
use amx_proto::control::agent::{HookEvent, ReportParams};

/// What a hook report reduces to once the machine has read it.
///
/// The reduction is the point: `amx _hook` forwards everything it is installed
/// for, tagged, and *this* is where policy lives (D-M2-4). herdr baked the
/// filtering into installed scripts, so changing policy meant reinstalling
/// hooks on every machine; changing it here is shipping a binary.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HookEdge {
    /// The event, as the agent named it.
    pub event: HookEvent,
    /// Whether it was scoped to a subagent.
    ///
    /// The single most load-bearing field in the module. `true` means the
    /// report carried an `agent_id`, which V01 §3 M4 measured as present on
    /// every subagent-scoped event and absent from every parent one — 44
    /// `Stop`s without, 8 `SubagentStop`s with.
    pub subagent: bool,
    /// The tool a `PreToolUse`/`PermissionRequest` names, when it names one.
    pub tool: Option<String>,
    /// `Notification`'s type: `permission_prompt` or `idle_prompt`.
    pub notification: Option<String>,
    /// The conversation this report identifies, when it carries one.
    ///
    /// Taken from **every** `SessionStart`, not just the first: V01 §3 M8
    /// measured `/clear` minting a new session id inside one process, and the
    /// `source` field is the only warning that the previous ref is now stale.
    pub session_ref: Option<SessionRef>,
}

/// `Notification`'s type for a permission dialog nobody has answered.
///
/// V01 §3 M5 measured it 6.0 s into a block, carrying
/// `message: "Claude needs your permission"`. It is the one hook-borne second
/// witness a held [`Blocked`](AgentState::Blocked) ever gets.
pub const PERMISSION_PROMPT: &str = "permission_prompt";

/// `Notification`'s type for an agent that has been at its prompt a while.
///
/// Measured at 60 s, including 60 s after a *silent* Esc-interrupt — the only
/// hook-borne evidence that an interrupted turn ever ended. Far too slow to
/// drive a status line, which is why it is not an exit edge; see
/// [`precedence`].
pub const IDLE_PROMPT: &str = "idle_prompt";

impl HookEdge {
    /// Reduce one report to an edge.
    ///
    /// `ref_kind` comes from the reporting agent's stanza. V02's doc comment
    /// asked for exactly this ("building the [`SessionRef`] needs the stanza's
    /// `ref_kind`, so the caller supplies the kind rather than this guessing")
    /// while its signature took no such argument; the parameter is that
    /// sentence, honoured. There is no defensible default — a
    /// [`RefKind::Path`] agent's `session_id` is not a path, and guessing
    /// either way would mint a ref that plans the wrong argv.
    ///
    /// A subagent-scoped report yields **no** ref. Its payload carries the
    /// parent's `session_id` (V01 §3 M4), so reading one here would let a
    /// subagent event write the pane's resume ref — a smaller version of the
    /// same mistake as letting it write the pane's state.
    #[must_use]
    pub fn from_report(report: &ReportParams, ref_kind: RefKind) -> Self {
        let subagent = !report.scope.is_parent();
        Self {
            event: report.event.clone(),
            subagent,
            tool: report.tool.clone(),
            notification: report.notification.clone(),
            session_ref: if subagent {
                None
            } else {
                reported_ref(report, ref_kind)
            },
        }
    }

    /// Whether this edge is a `Notification` of `kind`.
    #[must_use]
    pub fn is_notification(&self, kind: &str) -> bool {
        self.event == HookEvent::Notification && self.notification.as_deref() == Some(kind)
    }
}

/// The ref `report` carries, in the shape `ref_kind` calls for.
///
/// An id ref reads `session_id`; a path ref reads `transcript_path`, which V01
/// §2 measured as `<session_id>.jsonl` under the project's history directory —
/// the same conversation, spelled the way a `path` agent's resume template
/// wants it. A value that does not survive [`SessionRef::new`]'s shape rules is
/// dropped rather than reported: a malformed ref is a resume amx will not
/// offer, not an error to answer a hook with.
fn reported_ref(report: &ReportParams, ref_kind: RefKind) -> Option<SessionRef> {
    let value = match ref_kind {
        RefKind::Id => report.session_id.clone()?,
        RefKind::Path => report.transcript_path.as_ref()?.to_str()?.to_owned(),
    };
    SessionRef::new(ref_kind, value).ok()
}

/// What one hook edge is allowed to do to a tracker, given its coverage class.
///
/// The precedence table of 04 §5 and D-M2-6, as a value. Three outcomes and no
/// fourth: an edge either enters a state now, asks to leave the held one, or
/// changes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeEffect {
    /// Apply this state immediately. Entry edges only, and only for a class
    /// whose [`hook_entries_apply`](CoverageClass::hook_entries_apply).
    Enter(AgentState),
    /// This edge says the held state is over.
    ///
    /// Carries the [`ExitAuthority`] that makes it legal, which for every class
    /// but [`CoverageClass::Full`] means this variant is unreachable — the
    /// authority cannot be built. That is the type doing the work 04 §5's
    /// prose does.
    Leave(ExitAuthority),
    /// Nothing: corroboration, a subagent's business, or an event this build
    /// has never heard of.
    Ignore,
}

/// What `edge` may do to a pane running an agent of class `class`.
///
/// The measured table, with the recording behind each row:
///
/// | Edge | `edges` class | Why |
/// |---|---|---|
/// | `UserPromptSubmit` | `Enter(Working)` | Fires on both agents, median 26 ms after the keystroke |
/// | `PreToolUse` | `Enter(Working)` | Claude Code always; on Codex *conditional* (a plain file read produced only `PostToolUse`), so it corroborates there rather than driving |
/// | `PostToolUse` | `Enter(Working)` | The turn continues after a tool returns |
/// | `PostToolUseFailure` | `Enter(Working)` | Same: a tool that failed on its own hands the turn back to the model. Never an interrupt — V01 §3 M1 measured Esc during a tool call emitting nothing at all |
/// | `PermissionRequest` | `Enter(Blocked)` | Starts 8–14 ms *before* the dialog paints, so the entry is never late relative to the screen |
/// | `Stop` | `Ignore` | An exit, and exits are screen-owned — see the module docs |
/// | `SessionEnd` | `Ignore` | Likewise, and V01 §3 M8 measured `/clear` emitting one mid-session with the pane still very much alive |
/// | `SessionStart` | `Ignore` for state; carries the ref | Codex fires it only at the first prompt, so it says nothing about *doing* |
/// | `Notification` | `Ignore` | A 6 s/60 s backstop, far too slow to drive a status line |
/// | anything `subagent` | `Ignore` | Never touches the parent — not on entry, not on exit |
///
/// For [`CoverageClass::Identity`] and [`CoverageClass::None`] every row is
/// `Ignore`: those classes' state is the screen's, entirely.
///
/// The two exit rows are not written as `Ignore` but *computed*: they ask
/// [`ExitAuthority::from_hook`] for permission and take the `None` it returns
/// for every class amx ships. Spelling them that way is what makes the
/// [`CoverageClass::Full`] arm exist and stay honest — a class whose hooks do
/// cover the whole lifecycle closes its own turns, and nothing here has to be
/// rewritten to let it.
#[must_use]
pub fn precedence(class: CoverageClass, edge: &HookEdge) -> EdgeEffect {
    // First, and before the class is even consulted: 04 §5's "subagent-scoped
    // events never override the parent turn's state". V01 §3 M4 measured an
    // anonymous `SubagentStop` landing 1.9–3.0 s after the parent's `Stop` on
    // essentially every tool-using turn, so this is the common path, not a
    // corner one.
    if edge.subagent {
        return EdgeEffect::Ignore;
    }
    if !class.hook_entries_apply() {
        // `identity` carries session identity only, `none` carries nothing at
        // all — and the ref is read outside this table, so an `identity` agent
        // still resumes.
        return EdgeEffect::Ignore;
    }
    match edge.event {
        HookEvent::UserPromptSubmit
        | HookEvent::PreToolUse
        | HookEvent::PostToolUse
        | HookEvent::PostToolUseFailure => EdgeEffect::Enter(AgentState::Working),
        HookEvent::PermissionRequest => EdgeEffect::Enter(AgentState::Blocked),
        HookEvent::Stop | HookEvent::SessionEnd => match ExitAuthority::from_hook(class) {
            Some(authority) => EdgeEffect::Leave(authority),
            None => EdgeEffect::Ignore,
        },
        // `SessionStart` is identity, not state; `Notification` is a backstop
        // the tracker reads for corroboration only; the subagent pair cannot
        // reach here with parent scope on either measured agent, and would
        // assert nothing about the parent if it did; `Other` is an event this
        // build has never heard of, and these tools ship weekly.
        HookEvent::SessionStart
        | HookEvent::Notification
        | HookEvent::SubagentStart
        | HookEvent::SubagentStop
        | HookEvent::Other(_) => EdgeEffect::Ignore,
    }
}
