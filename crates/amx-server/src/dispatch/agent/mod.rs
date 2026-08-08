//! `agent.*` handlers.
//!
//! The bodies arrived in wave order and this file was edited **sequentially,
//! never concurrently** — `docs/08-m2-plan.md` §6 resolves the one collision it
//! would otherwise have had: V08 and V09 both land in wave 4, so the file was
//! assigned to V09 (the report arm) and V08's `agent.next` logic lives in
//! `actor/agent_hub/` with the arm here calling through the
//! [`AgentHandle`](crate::actor::AgentHandle) V02 already typed.
//!
//! **One seam is open**, and it is M4's: [`list`] routes, and what `Core`
//! answers with is a typed refusal until X10 writes the real one. The helper
//! that produces it lives in `actor/core/route.rs`, beside the arm that will
//! replace it; `tests/hygiene.rs` carries the ledger that stops it outliving
//! the milestone.
//!
//! # Task ownership
//!
//! | Handler | Filled by | Wave |
//! |---|---|---|
//! | [`report`] | V09 | 4 |
//! | [`next`] | V08 wrote the queue, V17 called it | 4 / 7 |
//! | [`explain`] | V06 wrote the explanation, V17 called it | 2 / 7 |
//! | [`start`] | V13 | 5 |
//! | [`prompt`] | V13 | 5 |
//! | [`list`] | **owed by X10** | M4 wave 3 |
//!
//! # Addressing
//!
//! Both of V13's verbs take their target the way D-M2-9 says every agent verb
//! does — pane UUID, then a label unique among *agent* panes — and neither of
//! them implements that rule: `agent/address.rs` does, for the pane-driving
//! verbs too, so the two scopes are one function with one order and cannot
//! learn to disagree about what a name means.

mod lookup;
mod waits;

use amx_core::Seq;
use amx_core::agent::AgentState;
use amx_proto::control::{agent, pane as pane_proto};
use amx_proto::rpc::RpcError;
use tokio::sync::oneshot;

use self::lookup::{anchor, session_state, stanza};
use self::waits::{ready, settled};
use super::Router;
use crate::actor::{AgentCommand, AgentQueryCall, CoreCommand, PaneCall};
use crate::agent::address::{self, Scope};

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
/// **V13 landed this.** Readiness is two facts and a deadline (04 §5's
/// "readiness handshake with timeout, herdr semantics"): the identity tier
/// confirms the agent binary owns the pane's foreground, *and* a status of
/// `Idle` has been observed. On expiry, report the failure honestly and leave
/// the pane alive for inspection.
///
/// V01 §4 constrains the timeout: Codex emits no hook at all until the first
/// prompt, so a readiness that waited on a `SessionStart` would wait forever on
/// an idle Codex. Readiness is tier-2 and tier-3 evidence, with hooks as a
/// bonus.
///
/// The order below is the order the failures want. Everything that can be
/// refused is refused *before* a process exists: an unknown agent, a name no
/// resolver could give back, a workspace with nothing to split. After the
/// spawn, the only remaining outcome is a readiness that did or did not land,
/// and neither of them takes the pane away.
pub(super) async fn start(
    router: &mut Router,
    params: agent::StartParams,
) -> Result<agent::StartReply, RpcError> {
    let events = router.events()?.clone();
    let stanza = stanza(router, &params.kind).await?;
    let state = session_state(router).await?;
    address::check_new_name(&params.name, &state.panes)?;
    let anchor = anchor(&state, params.workspace)?;

    // The stanza's argv first, the caller's extras after: a registry that could
    // be overridden *by* an argument would be a registry that decides nothing.
    let mut command = stanza.start.clone();
    command.extend(params.args);
    let split = router
        .call(|reply| {
            CoreCommand::Pane(PaneCall::Split {
                params: pane_proto::SplitParams {
                    pane: anchor,
                    direction: pane_proto::SplitDirection::Vertical,
                    command: Some(command),
                    // Absent means the split's own rule: the source pane's
                    // foreground-process cwd (04 §7), which is the directory
                    // the user is looking at and the one they meant.
                    cwd: params.cwd,
                },
                reply,
            })
        })
        .await?;

    // The name *is* the label (D-M2-9), so this is not decoration: it is what
    // makes the pane addressable by every later verb. `check_new_name` above
    // already refused everything `rename_pane` could, which leaves only "the
    // pane vanished between two calls" — reported with its id, since the pane
    // is running either way and the caller has to be told which one it is.
    router
        .call(|reply| {
            CoreCommand::Pane(PaneCall::Rename {
                params: pane_proto::RenameParams {
                    pane: split.pane,
                    label: params.name.clone(),
                },
                reply,
            })
        })
        .await
        .map_err(|err| {
            RpcError::new(
                err.code,
                format!(
                    "agent started in pane {} but could not be named {:?}: {}",
                    split.pane, params.name, err.message
                ),
            )
        })?;

    let readiness = ready(&events, split.pane, &stanza, params.timeout_ms).await?;
    Ok(agent::StartReply {
        pane: split.pane,
        short: split.short,
        readiness,
        agent: events.status().get(split.pane),
        seq: events.bus().head(),
    })
}

/// `agent.prompt`: submit a prompt, optionally waiting for what happens next.
///
/// **V13 landed this**, riding V12's `pane.run` for the submission itself. The
/// wait is a [`Waiter`] whose predicate requires the target status *and* a
/// transition sequence later than the submit — a pane that was already blocked
/// when the prompt was sent must not satisfy a `--wait blocked`.
///
/// The floor is read *before* the text is handed over, not after. Between the
/// two there is a window in which the agent has already reacted, and a floor
/// taken on the far side of it would sit above the very transition the caller
/// asked to wait for — the wait would then run until its timeout with the
/// answer already on screen.
pub(super) async fn prompt(
    router: &mut Router,
    params: agent::PromptParams,
) -> Result<agent::PromptReply, RpcError> {
    let events = router.events()?.clone();
    let state = session_state(router).await?;
    let pane = address::resolve(&params.target, &state.panes, Scope::Agent)?;

    let submitted_seq = events.bus().head();
    super::pane::run(
        router,
        pane_proto::RunParams {
            target: pane_proto::PaneTarget::from(pane),
            text: params.text,
        },
    )
    .await?;

    let satisfied = match params.wait {
        agent::PromptWait::None => true,
        agent::PromptWait::Blocked => {
            settled(
                &events,
                pane,
                AgentState::Blocked,
                submitted_seq,
                params.timeout_ms,
            )
            .await?
        }
        agent::PromptWait::Idle => {
            settled(
                &events,
                pane,
                AgentState::Idle,
                submitted_seq,
                params.timeout_ms,
            )
            .await?
        }
    };
    Ok(agent::PromptReply {
        pane,
        agent: events.status().get(pane),
        satisfied,
        submitted_seq,
    })
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
/// takes. What it could not land was *this arm*, because reaching a pane's
/// manifest, its frames and its fused status means `AgentCommand::Explain` and
/// a hub to answer it, and the hub was V08's — a wave later. **V17 joined the
/// two**, which is one mailbox round trip.
///
/// The target resolves in [`Scope::AnyPane`], and it is the one agent verb that
/// does. Every other one addresses something it is about to *drive*, where
/// D-M2-9's "unique among agent panes" is the right narrowing; this one answers
/// "why does amx not think this is an agent?", and a scope that refused to name
/// an unidentified pane would refuse exactly the pane the question is about. A
/// UUID resolves in either scope, so the difference is only ever about labels.
pub(super) async fn explain(
    router: &mut Router,
    params: agent::ExplainParams,
) -> Result<agent::ExplainReply, RpcError> {
    let hub = lookup::hub(router)?;
    let state = session_state(router).await?;
    let pane = address::resolve(&params.target, &state.panes, Scope::AnyPane)?;
    lookup::ask(&hub, |reply| AgentCommand::Explain { pane, reply }).await
}

/// `agent.next`: focus the head of the attention queue.
///
/// The queue logic lives in `actor/agent_hub/` — V08 wrote it there so that it
/// never edited this file (§6's wave-4 resolution) and **V17 called it**. The
/// arm is the handle round trip and nothing else: the hub reads its own queue,
/// posts the focus to `Core` fire-and-forget, and answers.
///
/// An empty queue is an honest empty reply, never an error: a prefix key that
/// raised an error dialog for "nothing is waiting" would be chrome, and 03 §4
/// has none.
pub(super) async fn next(
    router: &mut Router,
    params: agent::NextParams,
) -> Result<agent::NextReply, RpcError> {
    // X17 reads the scope; until then the row behaves exactly as it did before
    // the field existed, which is what the unchanged goldens assert.
    let agent::NextParams { workspace: _ } = params;
    let hub = lookup::hub(router)?;
    lookup::ask(&hub, |reply| AgentCommand::NextAttention { reply }).await
}

/// `agent.list`: every tracked agent, in one reply.
///
/// One mailbox round trip and no more, whatever the pane count
/// (`docs/11-m4-plan.md` D-M4-2): every field D15's reply needs is already
/// inside `Core` — the workspace labels, the pane labels, the hub's mirrored
/// statuses, the queue, and the published snapshot each `last_line` is read off
/// — so a per-pane fan-out would have bought nothing and cost 25 round trips at
/// the 25 agents the surface exists for.
///
/// **The row is the milestone's open seam.** This arm is finished; what `Core`
/// answers with is not, and until **X10** writes it the reply is the typed
/// refusal in `actor/core/route.rs`. Routing the call anyway rather than
/// refusing here is deliberate: the whole path — table, decode, mailbox, reply
/// — is exercised from wave 1, so the wave that fills it changes an answer
/// rather than discovering a route.
pub(super) async fn list(
    router: &mut Router,
    params: agent::ListParams,
) -> Result<agent::ListReply, RpcError> {
    router
        .call(|reply| CoreCommand::AgentQuery(AgentQueryCall::List { params, reply }))
        .await
}
