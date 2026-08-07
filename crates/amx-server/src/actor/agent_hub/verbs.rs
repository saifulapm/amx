//! What the question-shaped commands answer.
//!
//! Four of the hub's six mailbox variants ask something rather than tell it,
//! and all four answer without leaving the actor: `agent.next` reads its own
//! queue and posts the focus onwards, `agent.explain` re-evaluates the pane's
//! current frame, `agent.start`'s stanza lookup reads the registry the hub
//! parsed at assembly, and a command that arrives after cancellation is refused
//! from inside this module — no publish, no write, no sibling (R-M1-2).

use amx_core::PaneId;
use amx_core::agent::AgentKind;
use amx_proto::control::agent as proto;
use amx_proto::rpc::RpcError;
use amx_vt::SnapshotRef;

use super::{AgentHub, detect};
use crate::actor::{AgentCall, AgentCommand, CoreCommand};
use crate::agent::registry::AgentStanza;

impl AgentHub {
    // -------------------------------------------------------------- the verbs

    /// `agent.next`: the head of the attention queue, focused.
    ///
    /// The focus itself is a fire-and-forget message to `Core` and the reply
    /// does not wait for it: the hub must never park on a sibling, and the
    /// client learns focus moved from `FocusChanged` like it does for every
    /// other focus change.
    pub(super) fn next_attention(&self) -> proto::NextReply {
        let pane = self.attention.first().copied();
        if let Some(pane) = pane {
            let _ = self
                .core
                .try_send(CoreCommand::Agent(AgentCall::Focus { pane }));
        }
        proto::NextReply {
            pane,
            // A hint, from the workspace the pane was created in. `Core`
            // resolves the pane's *current* workspace when it focuses, so a
            // pane moved between workspaces since is focused correctly even
            // though this field names where it used to live.
            workspace: pane.and_then(|pane| self.workspaces.get(&pane).copied()),
            waiting: u32::try_from(self.attention.len()).unwrap_or(u32::MAX),
            seq: self.ctx.bus.head(),
        }
    }

    /// `agent.explain`: how this pane's status was detected, rule by rule.
    ///
    /// Every rule's verdict with its evidence, not only the winner's: 04 §5
    /// keeps herdr's `agent explain` because a detection you cannot interrogate
    /// is a detection you cannot fix.
    pub(super) fn explain(&mut self, pane: PaneId) -> Result<proto::ExplainReply, RpcError> {
        let Some(tracked) = self.panes.get(&pane) else {
            return Err(RpcError::new(
                RpcError::INVALID_PARAMS,
                format!("no agent tracker for pane {pane}"),
            ));
        };
        let kind = tracked.tracker.kind.clone();
        let status = tracked.snapshot();
        let Some(manifest) = tracked.manifest.clone() else {
            // No manifest means tier 2 is not running here at all — an
            // unidentified pane, or an agent whose stanza names none. Saying so
            // plainly beats an empty rule list that looks like "nothing
            // matched".
            return Ok(proto::ExplainReply {
                pane,
                kind,
                manifest: None,
                manifest_version: None,
                matched: None,
                region_preview: Vec::new(),
                rules: Vec::new(),
                agent: Some(status),
            });
        };
        let frame = self.frame(pane);
        let Some(tracked) = self.panes.get_mut(&pane) else {
            return Err(RpcError::new(
                RpcError::INVALID_PARAMS,
                format!("no agent tracker for pane {pane}"),
            ));
        };
        detect::fill(&mut tracked.screen, frame.as_deref());
        let explanation = manifest.explain(tracked.screen.screen(&tracked.title));
        Ok(explanation.into_reply(pane, kind, Some(status)))
    }

    /// What the registry says about one agent, by id or alias.
    ///
    /// The hub answers because the hub *has* the registry: parsed once at
    /// assembly and merged with the user's override there (D-M2-2), which is
    /// also the seam a suite plants a fake agent through. A refusal names the
    /// agents this session does know, since the common way to reach it is a
    /// typo or an override file that did not load.
    pub(super) fn stanza(&self, kind: &AgentKind) -> Result<Box<AgentStanza>, RpcError> {
        self.registry
            .resolve(kind)
            .map(|stanza| Box::new(stanza.clone()))
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .registry
                    .stanzas()
                    .iter()
                    .map(|stanza| stanza.id.as_str())
                    .collect();
                RpcError::new(
                    RpcError::INVALID_PARAMS,
                    format!(
                        "no agent named `{kind}` in this session's registry; it has {}",
                        known.join(", ")
                    ),
                )
            })
    }

    /// The frame `pane` last published, if it is still tracked.
    pub(super) fn frame(&self, pane: PaneId) -> Option<SnapshotRef> {
        self.panes.get(&pane).map(|tracked| tracked.frames.latest())
    }

    /// Answer one command without leaving the module: no publish, no write, no
    /// sibling.
    pub(super) fn refuse(&self, command: AgentCommand) {
        match command {
            AgentCommand::HookReport { reply, .. } => {
                // Silently unaccepted, never an error: a hook must not break or
                // slow a turn, and the emitter treats every outcome the same
                // way anyway (D-M2-4).
                let _ = reply.send(Ok(proto::ReportReply {
                    accepted: false,
                    seq: self.ctx.bus.head(),
                }));
            }
            AgentCommand::Explain { reply, .. } => {
                let _ = reply.send(Err(shutting_down()));
            }
            AgentCommand::NextAttention { reply } => {
                // Not an empty reply: an empty queue and a queue nobody can be
                // focused out of are different answers, and only one of them is
                // true here.
                let _ = reply.send(Err(shutting_down()));
            }
            AgentCommand::Stanza { reply, .. } => {
                // The registry is still right here and answering would cost
                // nothing — but the caller is an `agent.start` that is about to
                // spawn a process into a session that is going away, and the
                // honest answer to that is no.
                let _ = reply.send(Err(shutting_down()));
            }
            AgentCommand::PaneStarted { .. } | AgentCommand::PaneClosed { .. } => {}
        }
    }
}

/// What a command arriving after cancellation is told.
///
/// An error and not a plausible answer: a queue nobody can be focused out of is
/// not an empty queue, and saying so is the honest half of a refusal that
/// deliberately does nothing else.
fn shutting_down() -> RpcError {
    RpcError::new(RpcError::INTERNAL_ERROR, "the session is shutting down")
}
