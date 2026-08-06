//! Control routing: one decoded call, one `Core` command, one reply.
//!
//! 04 §4 derives dispatch from the method table rather than hand-syncing it, so
//! this module implements [`amx_proto::control::Dispatch`] and nothing else
//! decides what a method name means. A row added to the table stops compiling
//! here until it has a handler — the property that replaces W6's four
//! hand-synced lists.
//!
//! Handlers do not touch session state. Each one turns typed parameters into a
//! [`CoreCommand`] carrying a `oneshot` reply channel, hands it to the `Core`
//! actor and awaits the answer; the `Core` is the only thing that mutates the
//! state tree and the only thing that publishes the resulting event.
//!
//! # Handler seams
//!
//! T16 filled the last two of these — `pane.*` and `workspace.*`, in
//! [`pane`] and [`workspace`] — so M0's method table has no handler left
//! answering [`NOT_IMPLEMENTED`]. The mechanism itself stays: a method landed
//! in the shared table before its `Core` wiring exists is a compile error here
//! until it gets a handler, and until then [`seam`] is what a build in that
//! state answers with rather than `METHOD_NOT_FOUND` — reporting an
//! unimplemented method as unknown would tell a client to stop offering it.

mod pane;
mod workspace;

use amx_proto::control::{
    Call, Dispatch, pane as pane_proto, session, workspace as workspace_proto,
};
use amx_proto::rpc::RpcError;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::actor::{CoreCommand, CoreHandle, Reply, SessionCall};

/// The JSON-RPC code for a method this build knows but has not implemented.
///
/// Deliberately not `METHOD_NOT_FOUND`: the method exists in the table both
/// peers share, so reporting it as unknown would tell a client to stop offering
/// it. `-32000` is inside JSON-RPC 2.0's implementation-defined server error
/// range.
pub const NOT_IMPLEMENTED: i32 = -32000;

/// Routes control calls to the `Core` actor.
#[derive(Clone, Debug)]
pub struct Router {
    core: CoreHandle,
}

impl Router {
    /// Route calls to `core`.
    #[must_use]
    pub const fn new(core: CoreHandle) -> Self {
        Self { core }
    }

    /// Send one command and await its reply.
    ///
    /// A `Core` that has stopped is an internal error for this call, not a
    /// reason to tear the connection down: the client gets a typed failure and
    /// the session's shutdown path closes the socket in its own time.
    pub(crate) async fn call<T>(
        &self,
        make: impl FnOnce(Reply<T>) -> CoreCommand,
    ) -> Result<T, RpcError> {
        let (tx, rx) = oneshot::channel();
        self.core
            .send(make(tx))
            .await
            .map_err(|_| RpcError::new(RpcError::INTERNAL_ERROR, "session core is gone"))?;
        rx.await.map_err(|_| {
            RpcError::new(RpcError::INTERNAL_ERROR, "session core dropped the reply")
        })?
    }
}

/// `method` is in the table but has no handler yet.
#[allow(
    dead_code,
    reason = "no method in M0's table is currently a seam; kept for the next one that lands ahead of its Core wiring"
)]
fn seam(method: &'static str) -> RpcError {
    RpcError::new(NOT_IMPLEMENTED, format!("{method} is not implemented yet"))
}

/// Decode `method`/`params` and run the handler for it.
///
/// Both failure modes here are replies rather than disconnects: an unknown
/// method is `METHOD_NOT_FOUND` and parameters that do not fit are
/// `INVALID_PARAMS`, so a peer built against a different revision of the table
/// keeps its session.
pub async fn handle(
    router: &mut Router,
    method: &str,
    params: Option<Value>,
) -> Result<Value, RpcError> {
    let call = Call::decode(method, params)?;
    amx_proto::control::dispatch(router, call).await
}

impl Dispatch for Router {
    async fn ping(&mut self, params: session::PingParams) -> Result<session::PingReply, RpcError> {
        self.call(|reply| CoreCommand::Session(SessionCall::Ping { params, reply }))
            .await
    }

    async fn workspace_create(
        &mut self,
        params: workspace_proto::CreateParams,
    ) -> Result<workspace_proto::CreateReply, RpcError> {
        workspace::create(self, params).await
    }

    async fn workspace_rename(
        &mut self,
        params: workspace_proto::RenameParams,
    ) -> Result<workspace_proto::RenameReply, RpcError> {
        workspace::rename(self, params).await
    }

    async fn workspace_kill(
        &mut self,
        params: workspace_proto::KillParams,
    ) -> Result<workspace_proto::KillReply, RpcError> {
        workspace::kill(self, params).await
    }

    async fn workspace_switch(
        &mut self,
        params: workspace_proto::SwitchParams,
    ) -> Result<workspace_proto::SwitchReply, RpcError> {
        workspace::switch(self, params).await
    }

    async fn pane_split(
        &mut self,
        params: pane_proto::SplitParams,
    ) -> Result<pane_proto::SplitReply, RpcError> {
        pane::split(self, params).await
    }

    async fn pane_zoom(
        &mut self,
        params: pane_proto::ZoomParams,
    ) -> Result<pane_proto::ZoomReply, RpcError> {
        pane::zoom(self, params).await
    }

    async fn pane_swap(
        &mut self,
        params: pane_proto::SwapParams,
    ) -> Result<pane_proto::SwapReply, RpcError> {
        pane::swap(self, params).await
    }

    async fn pane_move(
        &mut self,
        params: pane_proto::MoveParams,
    ) -> Result<pane_proto::MoveReply, RpcError> {
        pane::move_pane(self, params).await
    }

    async fn pane_close(
        &mut self,
        params: pane_proto::CloseParams,
    ) -> Result<pane_proto::CloseReply, RpcError> {
        pane::close(self, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::{NOT_IMPLEMENTED, seam};

    #[test]
    fn seam_reports_not_implemented_not_missing() {
        let err = seam("pane.example");
        assert_eq!(err.code, NOT_IMPLEMENTED);
        assert!(err.message.contains("pane.example"));
    }
}
