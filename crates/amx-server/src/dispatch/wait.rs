//! `wait` and `pane.wait_output`: the long-poll handlers.
//!
//! Both are seams today; **V11** fills both, and they sit together because they
//! share the one thing that makes a wait safe. 04 §2:
//!
//! > **Waits are state predicates, not event predicates.** … on any `gap` it
//! > re-evaluates state before consuming events after `to`. A transition
//! > falling inside a gap can therefore never hang a wait.
//!
//! [`Waiter`](amx_core::event::Waiter) already encodes that sequencing, and its
//! own doc comment names the hang you build by ignoring it: "a predicate that
//! accumulates event history is an event predicate wearing this trait's
//! clothes, and it will hang the first time its transition lands inside a gap."
//! These are its first consumers.
//!
//! The predicates read *live* state and nothing else:
//!
//! - `wait --until blocked|idle` reads `AgentHub`'s `StatusView`, which the hub
//!   writes **before** publishing the matching event (`docs/08-m2-plan.md` §3).
//!   That ordering is what makes a wait woken by the event certain to see a
//!   view at least as new as it.
//! - `wait --until exited` reads `Core`'s pane table: the pane either exists or
//!   it does not.
//! - `pane.wait_output` reads the pane's published snapshot through V05's text
//!   view, re-run per `PaneDamage` batch.
//!
//! Timeouts are parameters, never sleeps. The hygiene suite rejects a nap.
//!
//! # Task ownership
//!
//! **V11** fills [`wait`] and [`wait_output`], and owns `conn/events.rs` and
//! the `amx events --json` CLI beside them.

use amx_proto::control::{Method, wait as proto};
use amx_proto::rpc::RpcError;

use super::{Router, seam};

/// `wait`: until a pane's agent is blocked or idle, or its process ends.
///
/// **V11** fills this. There is no `done` status to wait for (04 §2 says so
/// outright) and no poll interval anywhere — V11's
/// `wait_until_blocked_returns_the_instant_the_status_lands` measures it as
/// sub-tick, and `a_transition_inside_a_bus_gap_cannot_hang_a_wire_wait` forces
/// the replay overflow between flip and resume, which is T03's discipline
/// carried to the wire.
pub(super) async fn wait(
    router: &mut Router,
    params: proto::WaitParams,
) -> Result<proto::WaitReply, RpcError> {
    let _ = (router, params);
    seam(Method::Wait, "V11")
}

/// `pane.wait_output`: until text or a regex appears on a pane's screen.
///
/// **V11** fills this. It matches the *screen*, not a byte stream: the
/// predicate runs over the visible grid per damage batch, so content that
/// scrolled through between two batches was never on screen when anything
/// looked. The params type documents that, and `pane.read`/`pane.history` are
/// where a caller goes instead.
///
/// The regex is compiled once per call and never per evaluation — a detection
/// path that recompiled under output pressure would pay for the pattern on
/// every damage batch.
pub(super) async fn wait_output(
    router: &mut Router,
    params: proto::WaitOutputParams,
) -> Result<proto::WaitOutputReply, RpcError> {
    let _ = (router, params);
    seam(Method::PaneWaitOutput, "V11")
}
