//! What `agent.start` has to find out before it can spawn anything.
//!
//! Three lookups, split out of [`super`] so that file stays inside the module
//! budget (HACKING.md's soft 500, and R-M2-5's rule that no split waits for the
//! hard limit). They sit together because they are the same *kind* of step: a
//! question answered before any process exists, so that everything refusable
//! about an `agent.start` is refused while refusing it is still free.
//!
//! # Task ownership
//!
//! **V13**, with the verbs above; **V17** added [`hub`] when `agent.explain`
//! and `agent.next` became the second and third handlers to need the mailbox,
//! and one wording for "this session has no agent hub" beat three.

use amx_core::agent::AgentKind;
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::session;
use amx_proto::rpc::RpcError;
use tokio::sync::oneshot;

use crate::actor::{AgentCommand, AgentHandle, CoreCommand, Reply, SessionCall};
use crate::agent::registry::AgentStanza;
use crate::dispatch::Router;

/// How long an `agent.start` with no stated timeout waits for readiness.
///
/// Generous, and V01 §6 is why: Codex took 2.8–4.6 s to finish its startup
/// gates against Claude Code's 1.1 s, and a machine under load is slower than a
/// machine being measured. The number that matters is not this one but the
/// caller's — this is only what a caller who did not choose gets, and it is
/// sized so that "not ready" means the agent really did not come up rather than
/// that amx was impatient.
pub(super) const START_TIMEOUT_MS: u64 = 30_000;

/// The session's `AgentHub`, or the refusal for a session without one.
///
/// Three of the five `agent.*` handlers ask the hub a question, and all three
/// mean the same thing by its absence: nothing in this session tracks an agent,
/// so there is no answer to give. `agent.report` is the exception and stays one
/// — a hook is told `accepted: false` and never an error, because it has no way
/// to act on one and no permission to print it.
pub(super) fn hub(router: &Router) -> Result<AgentHandle, RpcError> {
    router.agent().cloned().ok_or_else(|| {
        RpcError::new(
            RpcError::INTERNAL_ERROR,
            "this session has no agent hub, so it cannot start or track an agent",
        )
    })
}

/// Ask the hub one question and wait for its answer.
///
/// Both failures are the hub being gone rather than the question being wrong:
/// a closed mailbox, or a reply channel dropped by the drain that answers
/// nothing which would leave the actor (§3's shutdown discipline). Neither is a
/// reason to tear the connection down, so both become typed replies.
pub(super) async fn ask<T>(
    hub: &AgentHandle,
    make: impl FnOnce(Reply<T>) -> AgentCommand,
) -> Result<T, RpcError> {
    let (tx, rx) = oneshot::channel();
    hub.send(make(tx))
        .await
        .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "the agent hub is gone"))?;
    rx.await
        .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "the agent hub dropped the reply"))?
}

/// The stanza the registry has for `kind`, asked of the hub.
///
/// Dispatch does not parse `agents.toml`; the hub did, once, at assembly, with
/// the user's override folded in (D-M2-2). A session with no hub is a session
/// that cannot start an agent, and saying so beats spawning one nothing would
/// ever track.
pub(super) async fn stanza(
    router: &Router,
    kind: &AgentKind,
) -> Result<Box<AgentStanza>, RpcError> {
    let hub = hub(router)?;
    ask(&hub, |reply| AgentCommand::Stanza {
        kind: kind.clone(),
        reply,
    })
    .await
}

/// The session's state tree, which is where labels and layouts live.
pub(super) async fn session_state(router: &Router) -> Result<session::StateReply, RpcError> {
    router
        .call(|reply| {
            CoreCommand::Session(SessionCall::State {
                params: session::StateParams::default(),
                reply,
            })
        })
        .await
}

/// The pane a new agent's pane is split off.
///
/// `agent.start` names a workspace, not a pane, and a split needs one — so the
/// workspace's focused pane is the anchor, falling back to whatever pane its
/// layout holds. There is no "create a workspace for it" fallback: a session
/// with no panes at all has no shell to inherit a directory from and no client
/// looking at it, and inventing a workspace to hide that would make the failure
/// arrive later and further away.
pub(super) fn anchor(
    state: &session::StateReply,
    workspace: Option<WorkspaceId>,
) -> Result<PaneId, RpcError> {
    let chosen = match workspace.or(state.focused_workspace) {
        Some(id) => state
            .workspaces
            .iter()
            .find(|known| known.workspace == id)
            .ok_or_else(|| {
                RpcError::new(
                    RpcError::INVALID_PARAMS,
                    format!("no workspace {id} in this session"),
                )
            })?,
        None => state.workspaces.first().ok_or_else(|| {
            RpcError::new(
                RpcError::INVALID_PARAMS,
                "this session has no workspace to start an agent in; attach a client first",
            )
        })?,
    };
    chosen
        .focus
        .or_else(|| chosen.layout.panes().first().copied())
        .ok_or_else(|| {
            RpcError::new(
                RpcError::INVALID_PARAMS,
                format!(
                    "workspace {} has no pane to split an agent off",
                    chosen.workspace
                ),
            )
        })
}
