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
//! | [`report`] | **V09 — landed** | 4 |
//! | [`next`] | V08 — the `AgentHub` actor | 4 |
//! | [`explain`] | V06 — the tier-2 manifest engine | 2 (sequential fill) |
//! | [`start`] | V13 — agent verbs and addressing | 5 |
//! | [`prompt`] | V13 — agent verbs and addressing | 5 |

use amx_core::Seq;
use amx_proto::control::{Method, agent};
use amx_proto::rpc::RpcError;
use tokio::sync::oneshot;

use super::{Router, seam};
use crate::actor::AgentCommand;

/// `agent.report`: one hook invocation from `amx _hook`.
///
/// **V09 landed this.** The shape is D-M2-4's and V08's mailbox's: decode, hand
/// the report to the [`AgentHandle`](crate::actor::AgentHandle) with
/// `try_send`, and ack. Nothing is filtered here — the emitter forwards
/// everything it is installed for and the fusion machine owns the policy, so
/// changing what a `SubagentStop` means is shipping a binary rather than
/// reinstalling hooks on every machine.
///
/// `try_send` and not `send`, unlike every other handler that talks to an
/// actor: a hook must never break *or slow* a turn, and the emitter is holding
/// its 500 ms budget open across this call. A hub whose 64-deep mailbox is
/// full is a hub that will not act on this report in time to matter anyway, so
/// the report is dropped and said to be dropped rather than parked behind a
/// backlog of stale status.
///
/// **Nothing here is an error reply.** A token mismatch, a full mailbox, a hub
/// that is not assembled, a hub that is shutting down: all of them are
/// `accepted: false`, which is exactly what the field means — the report did
/// not reach a tracked pane. An `RpcError` would tell the emitter something it
/// has no way to act on and no permission to print.
pub(super) async fn report(
    router: &mut Router,
    params: Box<agent::ReportParams>,
) -> Result<agent::ReportReply, RpcError> {
    // Cloned out of the router so the borrow ends here: the `unclaimed` paths
    // below need the router back to read the bus head.
    let Some(hub) = router.agent().cloned() else {
        return Ok(unclaimed(router.head().await?));
    };

    let (tx, rx) = oneshot::channel();
    let pane = params.pane;
    let token = params.token.clone();
    let handed = hub.try_send(AgentCommand::HookReport {
        pane,
        token,
        report: params,
        reply: tx,
    });
    if handed.is_err() {
        return Ok(unclaimed(router.head().await?));
    }

    match rx.await {
        Ok(reply) => reply,
        // The hub dropped the reply channel, which on this path means it is
        // gone: cancelled, draining its own mailbox to closure and answering
        // nothing (`docs/08-m2-plan.md` §3's shutdown discipline). The report
        // reached no tracked pane, and saying so is the whole answer.
        Err(_) => Ok(unclaimed(router.head().await?)),
    }
}

/// The ack for a report that reached the session but no tracked pane.
///
/// The `seq` is the bus head, read the ordinary way, so a report run by hand
/// still tells its caller where the session's event stream stands.
const fn unclaimed(seq: Seq) -> agent::ReportReply {
    agent::ReportReply {
        accepted: false,
        seq,
    }
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
