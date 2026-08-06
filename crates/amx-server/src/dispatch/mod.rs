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
//! # Handler seams for T16
//!
//! T16 owns the `pane.*` and `workspace.*` behaviour and lands
//! `dispatch/pane.rs` and `dispatch/workspace.rs`. Every handler below that is
//! not yet backed by a [`CoreCommand`] variant is marked `T16 seam` and answers
//! [`NOT_IMPLEMENTED`]. Filling one in is: add the mailbox variant in
//! `actor/mod.rs`, handle it in `actor/core.rs`, and replace the seam body with
//! a `self.call(...)` line — the connection, framing and writer layers below
//! this module do not change.

use amx_proto::control::{Call, Dispatch, pane, session, workspace};
use amx_proto::rpc::RpcError;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::actor::{CoreCommand, CoreHandle, PaneCall, Reply, SessionCall, WorkspaceCall};

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
    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> CoreCommand) -> Result<T, RpcError> {
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

/// T16 seam: `method` is in the table but has no handler yet.
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
        params: workspace::CreateParams,
    ) -> Result<workspace::CreateReply, RpcError> {
        self.call(|reply| CoreCommand::Workspace(WorkspaceCall::Create { params, reply }))
            .await
    }

    /// T16 seam — `dispatch/workspace.rs`.
    async fn workspace_rename(
        &mut self,
        _params: workspace::RenameParams,
    ) -> Result<workspace::RenameReply, RpcError> {
        Err(seam("workspace.rename"))
    }

    /// T16 seam — `dispatch/workspace.rs`.
    async fn workspace_kill(
        &mut self,
        _params: workspace::KillParams,
    ) -> Result<workspace::KillReply, RpcError> {
        Err(seam("workspace.kill"))
    }

    /// T16 seam — `dispatch/workspace.rs`.
    async fn workspace_switch(
        &mut self,
        _params: workspace::SwitchParams,
    ) -> Result<workspace::SwitchReply, RpcError> {
        Err(seam("workspace.switch"))
    }

    async fn pane_split(
        &mut self,
        params: pane::SplitParams,
    ) -> Result<pane::SplitReply, RpcError> {
        self.call(|reply| CoreCommand::Pane(PaneCall::Split { params, reply }))
            .await
    }

    /// T16 seam — `dispatch/pane.rs`.
    async fn pane_zoom(&mut self, _params: pane::ZoomParams) -> Result<pane::ZoomReply, RpcError> {
        Err(seam("pane.zoom"))
    }

    /// T16 seam — `dispatch/pane.rs`.
    async fn pane_swap(&mut self, _params: pane::SwapParams) -> Result<pane::SwapReply, RpcError> {
        Err(seam("pane.swap"))
    }

    /// T16 seam — `dispatch/pane.rs`.
    async fn pane_move(&mut self, _params: pane::MoveParams) -> Result<pane::MoveReply, RpcError> {
        Err(seam("pane.move"))
    }

    /// T16 seam — `dispatch/pane.rs`.
    async fn pane_close(
        &mut self,
        _params: pane::CloseParams,
    ) -> Result<pane::CloseReply, RpcError> {
        Err(seam("pane.close"))
    }
}
