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
//! A method landed in the shared table before its `Core` wiring exists is a
//! compile error here until it gets a handler, and until then [`seam`] is what
//! a build in that state answers with rather than `METHOD_NOT_FOUND` —
//! reporting an unimplemented method as unknown would tell a client to stop
//! offering it. T16 emptied the seam list for M0; U01 refilled it with M1's
//! two new rows, each carrying the task that closes it, and U06 closed
//! `session.report`:
//!
//! | Method | Filled by |
//! |---|---|
//! | `pane.rename` | U07 |
//!
//! When the last one lands, [`seam`] has no callers again — which is U10's
//! `grep`-level acceptance check.

mod pane;
mod stream;
mod workspace;

use amx_proto::control::{
    Call, Dispatch, client as client_proto, pane as pane_proto, session, stream as stream_proto,
    workspace as workspace_proto,
};
use amx_proto::rpc::RpcError;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::actor::{ClientCall, CoreCommand, CoreHandle, Reply, SessionCall};
use crate::conn::streams::ConnStreams;

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
    /// The connection's stream bindings, present on real client connections.
    ///
    /// `stream.bind` and `pane.history` need them; every other method routes
    /// to the `Core` alone, which is why a `Router` without them (a test
    /// driving dispatch directly) still serves the rest of the table.
    streams: Option<ConnStreams>,
}

impl Router {
    /// Route calls to `core`.
    #[must_use]
    pub const fn new(core: CoreHandle) -> Self {
        Self {
            core,
            streams: None,
        }
    }

    /// Adopt the connection's stream bindings.
    pub fn attach_streams(&mut self, streams: ConnStreams) {
        self.streams = Some(streams);
    }

    /// The connection's stream bindings, if this router serves a connection.
    #[must_use]
    pub(crate) fn streams(&self) -> Option<&ConnStreams> {
        self.streams.as_ref()
    }

    /// The bus head, via the ordinary `ping` path.
    pub(crate) async fn head(&self) -> Result<amx_core::Seq, RpcError> {
        let identity = self
            .call(|reply| {
                CoreCommand::Session(SessionCall::Ping {
                    params: session::PingParams {},
                    reply,
                })
            })
            .await?;
        Ok(identity.seq)
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

    async fn pane_focus(
        &mut self,
        params: pane_proto::FocusParams,
    ) -> Result<pane_proto::FocusReply, RpcError> {
        pane::focus(self, params).await
    }

    async fn pane_resize(
        &mut self,
        params: pane_proto::ResizeParams,
    ) -> Result<pane_proto::ResizeReply, RpcError> {
        pane::resize(self, params).await
    }

    /// Seam owned by **U07** (`docs/07-m1-plan.md` §4), which routes it into
    /// `dispatch::pane` once `Core`'s handler mutates state.
    async fn pane_rename(
        &mut self,
        params: pane_proto::RenameParams,
    ) -> Result<pane_proto::RenameReply, RpcError> {
        let _ = params;
        Err(seam("pane.rename"))
    }

    async fn session_state(
        &mut self,
        params: session::StateParams,
    ) -> Result<session::StateReply, RpcError> {
        self.call(|reply| CoreCommand::Session(SessionCall::State { params, reply }))
            .await
    }

    async fn session_report(
        &mut self,
        params: session::ReportParams,
    ) -> Result<session::ReportReply, RpcError> {
        self.call(|reply| CoreCommand::Session(SessionCall::Report { params, reply }))
            .await
    }

    async fn stream_bind(
        &mut self,
        params: stream_proto::BindParams,
    ) -> Result<stream_proto::BindReply, RpcError> {
        stream::bind(self, params).await
    }

    async fn pane_history(
        &mut self,
        params: stream_proto::HistoryParams,
    ) -> Result<stream_proto::HistoryReply, RpcError> {
        stream::history(self, params).await
    }

    async fn client_viewport(
        &mut self,
        params: client_proto::Viewport,
    ) -> Result<client_proto::ViewportReply, RpcError> {
        self.call(|reply| CoreCommand::Client(ClientCall::Viewport { params, reply }))
            .await
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
