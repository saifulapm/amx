//! `events.subscribe`: the server→client event path.
//!
//! D-M2-5 records what was missing before **V11** and why it mattered: there
//! was **no server→client event path in the tree at all**. The JSON-RPC
//! notification type existed (`amx-proto/src/rpc.rs`), the server never sent
//! one, and the client dropped any it received (`amx-client/src/net.rs`) — it
//! still does until V14, which is the other half of this path. The client polls
//! `session.state` after every layout-mutating call, and `app/wired.rs` carries
//! an apology comment for the invalidation event it "cannot yet hear".
//!
//! Everything M2 promises — `⚑N` without polling, `amx events --json`, external
//! notifiers — rides this row. R-M2-4 flags it as M2's largest unbudgeted
//! piece, and as the pattern-setter for M3's reconnect resync, which is why its
//! gap contract gets the same golden treatment as the M0 protocol surfaces.
//!
//! The shape, from D-M2-5:
//!
//! - the reply carries the bus sequence at subscribe time — 04 §2's contract
//!   verbatim, and the thing that makes external resync possible;
//! - after which the connection's writer emits one JSON-RPC notification
//!   (method [`EVENT_METHOD`](amx_proto::control::wait::EVENT_METHOD)) per
//!   [`Delivery`](amx_core::Delivery) — envelope **or** `gap{from,to}` — on the
//!   control channel, which already outranks grid and bulk traffic in the
//!   writer's strict priority;
//! - a consumer that sees `gap` re-queries state (`session.state` carries its
//!   capture seq) and resumes. `amx events --json` prints deliveries as NDJSON
//!   and documents exactly that contract; the client applies the same rule, one
//!   `sync_state` per gap.
//!
//! Subscriptions die with the connection: cursors are per-connection state, so
//! there is nothing to persist and nothing to leak.
//!
//! # Task ownership
//!
//! **V11** filled it, together with `conn/events.rs` (new), the writer and
//! reader edits it needed, and `crates/amx/src/cmd/events.rs`.

use amx_proto::control::wait;
use amx_proto::rpc::RpcError;

use super::Router;

/// `events.subscribe`: register this connection for bus deliveries.
///
/// The whole handler is a hand-off: the cursor and the pump that drains it are
/// connection state, so [`ConnEvents`](crate::conn::events::ConnEvents) owns
/// both and this arm only says which sequence to start after. `after_seq` that
/// the replay buffer has already dropped is not refused — the first delivery is
/// a `gap` covering what was missed, which is the same answer a subscriber that
/// fell behind gets, and it keeps loss visible on the resume path too.
pub(super) async fn subscribe(
    router: &mut Router,
    params: wait::SubscribeParams,
) -> Result<wait::SubscribeReply, RpcError> {
    Ok(router.events()?.subscribe(params.after_seq))
}
