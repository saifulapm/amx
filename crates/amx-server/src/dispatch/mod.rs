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
//! # Handler seams: one, and it is M3's
//!
//! A method landed in the shared table before its `Core` wiring exists is a
//! compile error here until it gets a handler, and until then a `seam` helper
//! is what a build in that state answers with rather than `METHOD_NOT_FOUND`
//! — reporting an unimplemented method as unknown would tell a client to stop
//! offering it. The helper is a milestone's tool and it has never been allowed
//! to outlive one: T16 emptied the list for M0; U01 refilled it with M1's two
//! new rows, each carrying the task that closed it — U06 took `session.report`,
//! U07 took `pane.rename` — and the list emptied again, the helper retiring
//! with it.
//!
//! V02 brought both back for M2's twelve rows, together with the exemption in
//! `tests/hygiene.rs` that let them exist, because the two always move
//! together. V12 closed four — `pane.send_text`/`send_keys`/`run`/`read` — V09
//! `agent.report`, V11 three — `wait`/`pane.wait_output`/`events.subscribe` —
//! V13 two — `agent.start`/`agent.prompt` — and V17 closed the last two,
//! `agent.explain` and `agent.next`, deleting the helper and the exemption in
//! the same commit.
//!
//! **W03 reopens it for M3's one new row**, `session.handoff`, whose orchestrator
//! is W06's. The helper lives in [`session`] this time rather than here, so the
//! task that closes the row deletes the file, the helper and the exemption
//! together — and the list of owners it is held to is in `tests/hygiene.rs`.

mod agent;
mod events;
mod pane;
mod session;
mod stream;
mod wait;
mod workspace;

use amx_proto::control::{
    Call, Dispatch, agent as agent_proto, client as client_proto, pane as pane_proto,
    session as session_proto, stream as stream_proto, wait as wait_proto,
    workspace as workspace_proto,
};
use amx_proto::rpc::RpcError;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::actor::{AgentHandle, ClientCall, CoreCommand, CoreHandle, Reply, SessionCall};
use crate::conn::events::ConnEvents;
use crate::conn::streams::ConnStreams;

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
    /// The `AgentHub`'s mailbox, present once a session has assembled one.
    ///
    /// `Option` for the same reason [`streams`](Self::streams) is: it is a
    /// capability of the connection rather than of the table. A router without
    /// a hub answers `agent.report` honestly — the report reached the session
    /// and no tracked pane, which is what `accepted: false` means — instead of
    /// failing a call an agent's hook cannot be told about anyway.
    agent: Option<AgentHandle>,
    /// The connection's event subscription and the state its waits read.
    ///
    /// Present for the same reason and on the same terms as `streams`: `wait`,
    /// `pane.wait_output` and `events.subscribe` are the only rows that need
    /// the bus or the [`StatusView`](crate::actor::StatusView), and a `Router`
    /// built without them still serves the rest of the table.
    events: Option<ConnEvents>,
}

impl Router {
    /// Route calls to `core`.
    #[must_use]
    pub const fn new(core: CoreHandle) -> Self {
        Self {
            core,
            streams: None,
            agent: None,
            events: None,
        }
    }

    /// Adopt the connection's stream bindings.
    pub fn attach_streams(&mut self, streams: ConnStreams) {
        self.streams = Some(streams);
    }

    /// Adopt the connection's event state.
    pub fn attach_events(&mut self, events: ConnEvents) {
        self.events = Some(events);
    }

    /// The connection's stream bindings, if this router serves a connection.
    #[must_use]
    pub(crate) fn streams(&self) -> Option<&ConnStreams> {
        self.streams.as_ref()
    }

    /// Adopt the session's `AgentHub` mailbox.
    ///
    /// The call site the `agent.*` handlers were written against
    /// (`docs/08-m2-plan.md` §6's wave-4 resolution: `dispatch/agent.rs` is one
    /// task's file and reaches the hub *through the handle V02 typed*, so the
    /// hub's own task never edits the dispatch tree). A session that has
    /// assembled a hub hands it to every connection's router here.
    pub fn attach_agent(&mut self, agent: AgentHandle) {
        self.agent = Some(agent);
    }

    /// The session's `AgentHub`, if one is assembled.
    #[must_use]
    pub(crate) fn agent(&self) -> Option<&AgentHandle> {
        self.agent.as_ref()
    }

    /// The connection's event state, if this router serves a connection.
    pub(crate) fn events(&self) -> Result<&ConnEvents, RpcError> {
        self.events.as_ref().ok_or_else(|| {
            RpcError::new(
                RpcError::INTERNAL_ERROR,
                "this router serves no connection, so it has no event stream",
            )
        })
    }

    /// The bus head, via the ordinary `ping` path.
    pub(crate) async fn head(&self) -> Result<amx_core::Seq, RpcError> {
        let identity = self
            .call(|reply| {
                CoreCommand::Session(SessionCall::Ping {
                    params: session_proto::PingParams {},
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
    async fn ping(
        &mut self,
        params: session_proto::PingParams,
    ) -> Result<session_proto::PingReply, RpcError> {
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

    async fn pane_rename(
        &mut self,
        params: pane_proto::RenameParams,
    ) -> Result<pane_proto::RenameReply, RpcError> {
        pane::rename(self, params).await
    }

    async fn session_state(
        &mut self,
        params: session_proto::StateParams,
    ) -> Result<session_proto::StateReply, RpcError> {
        self.call(|reply| CoreCommand::Session(SessionCall::State { params, reply }))
            .await
    }

    async fn session_report(
        &mut self,
        params: session_proto::ReportParams,
    ) -> Result<session_proto::ReportReply, RpcError> {
        self.call(|reply| CoreCommand::Session(SessionCall::Report { params, reply }))
            .await
    }

    async fn session_handoff(
        &mut self,
        params: session_proto::HandoffParams,
    ) -> Result<session_proto::HandoffReply, RpcError> {
        session::handoff(self, params).await
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

    async fn agent_report(
        &mut self,
        params: Box<agent_proto::ReportParams>,
    ) -> Result<agent_proto::ReportReply, RpcError> {
        agent::report(self, params).await
    }

    async fn agent_start(
        &mut self,
        params: agent_proto::StartParams,
    ) -> Result<agent_proto::StartReply, RpcError> {
        agent::start(self, params).await
    }

    async fn agent_prompt(
        &mut self,
        params: agent_proto::PromptParams,
    ) -> Result<agent_proto::PromptReply, RpcError> {
        agent::prompt(self, params).await
    }

    async fn agent_explain(
        &mut self,
        params: agent_proto::ExplainParams,
    ) -> Result<agent_proto::ExplainReply, RpcError> {
        agent::explain(self, params).await
    }

    async fn agent_next(
        &mut self,
        params: agent_proto::NextParams,
    ) -> Result<agent_proto::NextReply, RpcError> {
        agent::next(self, params).await
    }

    async fn wait(
        &mut self,
        params: wait_proto::WaitParams,
    ) -> Result<wait_proto::WaitReply, RpcError> {
        wait::wait(self, params).await
    }

    async fn events_subscribe(
        &mut self,
        params: wait_proto::SubscribeParams,
    ) -> Result<wait_proto::SubscribeReply, RpcError> {
        events::subscribe(self, params).await
    }

    async fn pane_send_text(
        &mut self,
        params: pane_proto::SendTextParams,
    ) -> Result<pane_proto::SendTextReply, RpcError> {
        pane::send_text(self, params).await
    }

    async fn pane_send_keys(
        &mut self,
        params: pane_proto::SendKeysParams,
    ) -> Result<pane_proto::SendKeysReply, RpcError> {
        pane::send_keys(self, params).await
    }

    async fn pane_run(
        &mut self,
        params: pane_proto::RunParams,
    ) -> Result<pane_proto::RunReply, RpcError> {
        pane::run(self, params).await
    }

    async fn pane_read(
        &mut self,
        params: pane_proto::ReadParams,
    ) -> Result<pane_proto::ReadReply, RpcError> {
        pane::read(self, params).await
    }

    async fn pane_wait_output(
        &mut self,
        params: wait_proto::WaitOutputParams,
    ) -> Result<wait_proto::WaitOutputReply, RpcError> {
        wait::wait_output(self, params).await
    }
}
