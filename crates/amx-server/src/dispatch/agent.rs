//! `agent.*` handlers.
//!
//! Every one is a seam today. The bodies arrive in wave order and this file is
//! edited **sequentially, never concurrently** — `docs/08-m2-plan.md` §6
//! resolves the one collision it would otherwise have: V08 and V09 both land in
//! wave 4, so the file is assigned to V09 (the report arm) and V08's
//! `agent.next` logic lives in `actor/agent_hub/` with the arm here calling
//! through the [`AgentHandle`](crate::actor::AgentHandle) V02 already typed.
//!
//! # Task ownership
//!
//! | Handler | Filled by | Wave |
//! |---|---|---|
//! | [`report`] | V09 — `amx _hook` and report ingestion | 4 |
//! | [`next`] | V08 — the `AgentHub` actor | 4 |
//! | [`explain`] | V06 — the tier-2 manifest engine | 2 (sequential fill) |
//! | [`start`] | V13 — agent verbs and addressing | 5 |
//! | [`prompt`] | V13 — agent verbs and addressing | 5 |

use amx_proto::control::{Method, agent};
use amx_proto::rpc::RpcError;

use super::{Router, seam};

/// `agent.report`: one hook invocation from `amx _hook`.
///
/// **V09** fills this. The shape of the fill is fixed by D-M2-4 and by V08's
/// mailbox: decode, hand the report to the [`AgentHandle`](crate::actor::AgentHandle)
/// with `try_send`, and ack. Nothing is filtered here — the emitter forwards
/// everything it is installed for and the fusion machine owns the policy, so
/// changing what a `SubagentStop` means is shipping a binary rather than
/// reinstalling hooks on every machine.
///
/// A token mismatch is not an error reply: it is `accepted: false`, dropped and
/// counted by the hub. A hook must never break or slow a turn.
pub(super) async fn report(
    router: &mut Router,
    params: Box<agent::ReportParams>,
) -> Result<agent::ReportReply, RpcError> {
    let _ = (router, params);
    seam(Method::AgentReport, "V09")
}

/// `agent.start`: spawn an agent in a new pane and wait for it to be ready.
///
/// **V13** fills this. Readiness is two facts and a deadline (04 §5's
/// "readiness handshake with timeout, herdr semantics"): the identity tier
/// confirms the agent binary owns the pane's foreground, *and* a status of
/// `Idle` has been observed. On expiry, report the failure honestly and leave
/// the pane alive for inspection.
///
/// V01 §4 constrains the timeout: Codex emits no hook at all until the first
/// prompt, so a readiness that waited on a `SessionStart` would wait forever on
/// an idle Codex. Readiness is tier-2 and tier-3 evidence, with hooks as a
/// bonus.
pub(super) async fn start(
    router: &mut Router,
    params: agent::StartParams,
) -> Result<agent::StartReply, RpcError> {
    let _ = (router, params);
    seam(Method::AgentStart, "V13")
}

/// `agent.prompt`: submit a prompt, optionally waiting for what happens next.
///
/// **V13** fills this, riding V12's `pane.run` for the submission itself. The
/// wait is a [`Waiter`](amx_core::event::Waiter) whose predicate requires the
/// target status *and* a transition sequence later than the submit — a pane
/// that was already blocked when the prompt was sent must not satisfy a
/// `--wait blocked`.
pub(super) async fn prompt(
    router: &mut Router,
    params: agent::PromptParams,
) -> Result<agent::PromptReply, RpcError> {
    let _ = (router, params);
    seam(Method::AgentPrompt, "V13")
}

/// `agent.explain`: how a pane's status was detected, rule by rule.
///
/// Every rule reports its verdict and its evidence, not only the winner: 04 §5
/// keeps herdr's `agent explain` because a detection you cannot interrogate is
/// a detection you cannot fix.
///
/// **V06 landed the explanation**, in
/// [`Manifest::explain`](crate::agent::manifest::Manifest::explain): a compiled
/// manifest plus one screen produces the whole reply bar the three fields only
/// the hub knows, which [`Explanation::into_reply`](crate::agent::manifest::Explanation::into_reply)
/// takes. What it could not land is *this arm*, because reaching a pane's
/// manifest, its frames and its fused status means `AgentCommand::Explain` and
/// a hub to answer it, and the hub is V08's — a wave later. So the seam stays
/// until the hub exists, and closing it is a mailbox round trip:
///
/// ```text
/// let pane = router.resolve(params.target).await?;
/// router.agent().send(AgentCommand::Explain { pane, reply }).await
/// ```
///
/// The count in `tests/hygiene.rs` (`SEAM_COUNT`) is why this could not simply
/// be deleted and left to V08 either: seams are counted, and the count and the
/// call sites move together.
pub(super) async fn explain(
    router: &mut Router,
    params: agent::ExplainParams,
) -> Result<agent::ExplainReply, RpcError> {
    let _ = (router, params);
    seam(Method::AgentExplain, "V06")
}

/// `agent.next`: focus the head of the attention queue.
///
/// **V08** fills this — the arm calls through the `AgentHandle`, and the queue
/// logic lives in `actor/agent_hub/` so that V08 never edits this file (§6's
/// wave-4 resolution). An empty queue is an honest empty reply, never an error:
/// a prefix key that raised an error dialog for "nothing is waiting" would be
/// chrome, and 03 §4 has none.
pub(super) async fn next(
    router: &mut Router,
    params: agent::NextParams,
) -> Result<agent::NextReply, RpcError> {
    let _ = (router, params);
    seam(Method::AgentNext, "V08")
}
