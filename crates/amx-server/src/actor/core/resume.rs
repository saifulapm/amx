//! The resume half of a restore: planning, claiming, and typing back in.
//!
//! Split out of [`super::restore`] under the R-M1-3 rule — no split waits for
//! the hard limit — along the seam D-M2-7 already draws. [`super::restore`]
//! answers "did this pane come back"; this file answers "did its *conversation*
//! come back", which is a different question with a different failure table:
//!
//! | Failure | Action | Reported as |
//! |---|---|---|
//! | the ref's source names another agent | restore a plain shell | degraded (pane) |
//! | this build no longer has the agent's stanza | restore a plain shell | degraded (pane) |
//! | another pane claimed the same conversation | restore a plain shell | degraded (pane) |
//! | the pane's shell would not start | release the claim, prune the pane | lost (pane) |
//!
//! The order is D-M2-7's, literally — *reserve the dedupe key before any spawn,
//! roll it back on spawn failure* — because a pane whose shell will not start
//! must leave its conversation free for a later pane in the same snapshot
//! rather than take it down with it.
//!
//! Launch is type-in, not exec. The saved argv is *recorded*, never run: 04's
//! respawn model keeps a pane its shell, so a pane started as `claude` comes
//! back as a shell with `claude --resume …` typed into it. The pane therefore
//! survives the agent's eventual exit, and the invocation lands in the user's
//! own history where they can see and re-run it.
//!
//! The planning and the validation themselves are not here — they are
//! [`crate::agent::resume`], which is pure. What is here is everything that
//! needs a `Ctx`, a pane and a report to write into.

use std::collections::HashMap;
use std::path::Path;

use amx_core::agent::AgentKind;
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::session::{RestoreEntity, RestoreLoss, RestoreReport, RestoreSeverity};

use super::Core;
use super::restore::RestoreOptions;
use crate::actor::{PaneHandle, SnapshotFeed};
use crate::agent::registry::Registry;
use crate::agent::resume::{self, Planned, Reservations, ResumeRefusal};
use crate::persist::PaneSnapshot;

/// One restore in progress: what it was asked for, what it has claimed, and
/// what it has had to report.
///
/// Threaded through the pass rather than held on `Core`, because none of it
/// outlives the restore: the registry is read once for this snapshot, the
/// reservations are a property of *this* set of panes, and the report is moved
/// onto `Core` at the end. It is one value rather than four parameters for the
/// ordinary reason — the pane-level handlers already carry a workspace, a pane
/// and a snapshot, and a fifth and sixth argument is where a call site stops
/// being readable.
pub(super) struct Restoring<'a> {
    /// What the caller could not put in the snapshot.
    pub(super) opts: &'a RestoreOptions,
    /// The agent registry this restore plans against: the compiled-in stanzas
    /// merged with the user's override, read once here.
    pub(super) registry: Registry,
    /// The conversations already handed to a pane (D-M2-7's dedupe).
    pub(super) reservations: Reservations,
    /// The panes a conversation was typed back into, and which agent it was —
    /// handed to `Core` so a later capture can spell their `start_source`
    /// honestly.
    pub(super) resumed: HashMap<PaneId, AgentKind>,
    /// Everything that could not be rebuilt, as the user will read it.
    pub(super) report: RestoreReport,
}

impl Core {
    /// Plan `pane`'s resume and claim its conversation, before anything spawns.
    ///
    /// `None` covers three different things, and only one of them is a loss:
    /// the pane had no agent (most panes), the pane's agent had no conversation
    /// captured yet (V01 §4 — an idle Codex reaches a restart in exactly that
    /// state), or the conversation was refused. Only the third is reported, and
    /// it is reported as *degraded*: the pane comes back, as a plain shell, and
    /// what it lost is the conversation.
    ///
    /// The two gates D-M2-7 puts on the way out of the file both live under
    /// this call. Shape is checked by [`SessionRef`](amx_core::agent::SessionRef)'s
    /// own deserializer, so a hand-edited `session.json` holding a relative path
    /// or a control character never parses into a snapshot at all; source
    /// allowlisting is [`resume::plan`]'s, cross-checked against the stanza the
    /// ref claims.
    pub(super) fn claim_resume(
        &self,
        workspace: WorkspaceId,
        pane: PaneId,
        saved: Option<&PaneSnapshot>,
        run: &mut Restoring<'_>,
    ) -> Option<Planned> {
        let agent = saved?.agent.as_ref()?;
        let refused = match resume::plan(&run.registry, agent) {
            Ok(planned) if run.reservations.reserve(&planned.key) => return Some(planned),
            // Somebody else in this same snapshot got there first.
            Ok(_) => ResumeRefusal::Claimed,
            Err(refusal) => refusal,
        };
        if refused == ResumeRefusal::NoConversation {
            // Nothing was captured, so nothing was lost — an agent that never
            // reported a session is not a conversation that went missing.
            return None;
        }
        run.report.entries.push(RestoreLoss {
            severity: RestoreSeverity::Degraded,
            entity: RestoreEntity::Pane,
            workspace: Some(workspace),
            pane: Some(pane),
            label: saved.and_then(|saved| saved.label.clone()),
            path: None,
            reason: format!("restored as a plain shell: {refused}"),
        });
        None
    }

    /// Type a planned resume into a pane that has just come back.
    ///
    /// The wait for the shell's first paint and the write itself are
    /// [`resume::type_in`]'s; what is here is the decision to drive it on a
    /// task of its own. It cannot be done inline — restore is synchronous, and
    /// a session of five agents would otherwise spend five prompt-waits before
    /// the gateway accepted anybody — and it cannot be driven by `Core`'s loop
    /// either, because the loop has not started yet.
    ///
    /// So it is a task outside the supervisor's `JoinSet`, which is the one
    /// place in the server that is true, and it is made safe the way the
    /// wedge-flake constraint (R-M1-2) requires rather than by hope: the future
    /// holds nothing but a pane handle and a frame reader, it is bounded by
    /// [`resume::FIRST_PAINT_BUDGET`], and it is cancelled by this session's own
    /// token — so it cannot outlive the shutdown that would have joined it, and
    /// it cannot keep a pane's mailbox open behind one. Giving it a real owner
    /// means `Core` holding task handles, which is `actor/core/mod.rs`'s to
    /// grow; recorded for V17 with the other seams.
    pub(super) fn launch_resume(
        &self,
        pane: PaneId,
        handle: &PaneHandle,
        frames: SnapshotFeed,
        planned: &Planned,
    ) {
        let line = resume::command_line(&planned.argv);
        let handle = handle.clone();
        let cancel = self.ctx.cancel.child_token();
        tokio::spawn(async move {
            let typed = resume::type_in(frames, handle, line, resume::FIRST_PAINT_BUDGET, cancel);
            if let Err(err) = typed.await {
                // The restore report was answered before this future was
                // spawned, so this one degraded outcome cannot reach it. The
                // user still sees the truth on the pane: a shell with nothing
                // typed into it.
                tracing::warn!(%pane, error = %err, "the saved conversation was not resumed");
            }
        });
    }
}

/// The agent registry this restore plans against: the builtin, plus the user's
/// override.
///
/// The same two documents `AgentHub` merges, read from the same path, because
/// both are derived from [`Ctx::config_path`](amx_core::Ctx::config_path) and a
/// test pointing `Ctx` at a tempdir moves both together. It is read here rather
/// than asked of the hub because restore runs on an owned `Core` before any
/// actor is serving (D-M1-9) — and a *request* to the hub is exactly what
/// `docs/08-m2-plan.md` §3 keeps out of this direction.
///
/// A malformed override costs the stanzas it names and nothing else: the
/// builtins stay as they were, so a typo in a user's file cannot stop yesterday's
/// conversations from coming back.
pub(super) fn load_registry(config_path: &Path) -> Registry {
    let builtin = Registry::builtin();
    let path = Registry::override_path(config_path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return builtin;
    };
    let (merged, diagnostics) = builtin.with_override(&text);
    for diagnostic in diagnostics {
        tracing::warn!(path = %path.display(), "{diagnostic}");
    }
    merged
}
