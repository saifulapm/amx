//! `session.handoff`: the one place a running server is asked to become a
//! different one.
//!
//! The call itself is two steps, and the order of them is the point. The
//! pre-flight probe runs **here**, on the connection task, before `Core` has
//! heard that a handoff was asked for at all; only its verdict travels on to
//! `Core`, which freezes the session and hands it to the export orchestrator
//! (`docs/09-m3-plan.md` §3, D-M3-6 point 2). A staged binary that cannot read
//! this build's manifest, or whose protocol window would strand every attached
//! client, is therefore refused by a server that has not quiesced a pane —
//! where herdr validates its successor after pausing everything and pays a full
//! rollback for the same mistake.
//!
//! Running an exec from a connection task rather than from `Core` is the second
//! half of the same decision: `Core` serves every other verb from one mailbox
//! loop, and a wrong binary that never answers would hold all of them behind
//! it. The probe is bounded ([`PRE_FLIGHT_BOUND`](
//! crate::handoff::export::caps::PRE_FLIGHT_BOUND)) and the child is killed if
//! it overruns.
//!
//! **Acceptance is not completion.** The reply says whether the protocol
//! started; the exporter retires its gateway partway through, so this
//! connection dies before the swap finishes by design. The caller learns what
//! happened by reconnecting and reading `session.report`, whose `handoff` row
//! the orchestrator fills (D-M3-8).

use amx_proto::control::session::{HandoffParams, HandoffReply};
use amx_proto::rpc::RpcError;

use super::Router;
use crate::actor::{CoreCommand, SessionCall};
use crate::handoff::export::pre_flight;

/// `session.handoff` — hand this session over to a staged binary.
///
/// # Errors
///
/// Only for a `Core` that cannot be reached. A staged binary this session may
/// not be handed to is a *reply* — `accepted: false` with the reason — not a
/// failed call: the caller asked a legitimate question and got a true answer.
pub async fn handoff(router: &mut Router, params: HandoffParams) -> Result<HandoffReply, RpcError> {
    let preflight = pre_flight(&params.binary)
        .await
        .map_err(|err| err.to_string());
    router
        .call(|reply| {
            CoreCommand::Session(SessionCall::Handoff {
                params,
                preflight,
                reply,
            })
        })
        .await
}
