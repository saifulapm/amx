//! `session.handoff`, and the seam it answers at until W06 wires it.
//!
//! # The seam, reopened for M3
//!
//! A method that lands in the shared table before its implementation cannot
//! answer `METHOD_NOT_FOUND`: telling a client a method is unknown tells it to
//! stop offering it, and `amx update apply` is built to *find* this row on the
//! server it is upgrading. So a tabled-but-unwired row answers with the
//! implementation-defined [`NOT_IMPLEMENTED`] instead, naming the task that
//! owes it.
//!
//! That helper is a milestone's tool and has never been allowed to outlive one:
//! U01 opened it for M1 and U06/U07 closed it; V02 reopened it for M2's twelve
//! rows and V17 closed the last two and deleted it. W03 reopens it for M3 with
//! a list of exactly one:
//!
//! | Row | Owed by |
//! |---|---|
//! | `session.handoff` | **W06**, the export orchestrator |
//!
//! `tests/hygiene.rs` holds that list to one entry and fails the moment a
//! second `seam(…)` call site appears anywhere else; W14 deletes this module's
//! stub, the helper and the exemption together, which is M3's exit check.

use amx_proto::control::session::{HandoffParams, HandoffReply};
use amx_proto::rpc::RpcError;

use super::Router;

/// The code a tabled row answers with while its wiring is still being built.
///
/// Inside JSON-RPC 2.0's implementation-defined server-error range, and pinned
/// as a literal in `tests/skew.rs` so the wire-side assertion keeps meaning
/// "nobody answers this" after the constant here is deleted.
const NOT_IMPLEMENTED: i32 = -32000;

/// Answer a tabled row whose wiring has not landed, naming who owes it.
fn seam(method: &str, owner: &str) -> RpcError {
    RpcError::new(
        NOT_IMPLEMENTED,
        format!("{method} is in this build's method table but {owner} has not wired it yet"),
    )
}

/// `session.handoff` — hand this session over to a staged binary.
///
/// The whole call is W06's: pre-flight the successor with `_handoff-caps`,
/// quiesce and capture, retire the gateway after `restored`, fence the final
/// Persist push at commit, and resume everything on any pre-commit failure
/// (`docs/09-m3-plan.md` §3). None of that exists yet, and the parameters are
/// deliberately *not* validated here — a stub that refused a bad path would be
/// behavior, and this row's contract is that its behavior lands in one place.
///
/// # Errors
///
/// Always, today: the seam code, naming W06.
pub async fn handoff(
    _router: &mut Router,
    _params: HandoffParams,
) -> Result<HandoffReply, RpcError> {
    Err(seam("session.handoff", "W06"))
}
