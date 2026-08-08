//! The method table's third product: the dispatch trait.
//!
//! The point of the table is that a method cannot exist without a handler. This
//! file implements [`Dispatch`] for a stub server; it fails to compile the day
//! a row is added without one, which is the property that replaces W6's
//! hand-synced lists.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use amx_core::{Layout, PaneId, RowId, SessionId, ShortNumber, WorkspaceId};
use amx_proto::control::{
    Call, Dispatch, Method, agent, client, dispatch, pane, session, stream, wait, workspace,
};
use amx_proto::hello::ServerInfo;
use amx_proto::rpc::RpcError;
use amx_proto::stream::{StreamId, StreamKind};

struct StubServer {
    seq: u64,
}

impl Dispatch for StubServer {
    async fn ping(&mut self, _params: session::PingParams) -> Result<session::PingReply, RpcError> {
        Ok(session::PingReply {
            server: ServerInfo {
                name: "amx".into(),
                version: "0.1.0".into(),
            },
            session: SessionId::new_v4(),
            seq: self.seq,
        })
    }

    async fn workspace_create(
        &mut self,
        _params: workspace::CreateParams,
    ) -> Result<workspace::CreateReply, RpcError> {
        Ok(workspace::CreateReply {
            workspace: WorkspaceId::new_v4(),
            short: ShortNumber::FIRST,
            seq: self.seq,
        })
    }

    async fn pane_split(
        &mut self,
        params: pane::SplitParams,
    ) -> Result<pane::SplitReply, RpcError> {
        if params.direction == pane::SplitDirection::Horizontal {
            return Err(RpcError::new(RpcError::INTERNAL_ERROR, "stub refuses"));
        }
        Ok(pane::SplitReply {
            pane: PaneId::new_v4(),
            short: ShortNumber::new(2),
            seq: self.seq,
        })
    }

    async fn workspace_rename(
        &mut self,
        params: workspace::RenameParams,
    ) -> Result<workspace::RenameReply, RpcError> {
        let _ = params;
        Ok(workspace::RenameReply { seq: self.seq })
    }

    async fn workspace_kill(
        &mut self,
        params: workspace::KillParams,
    ) -> Result<workspace::KillReply, RpcError> {
        let _ = params;
        Ok(workspace::KillReply {
            panes: Vec::new(),
            seq: self.seq,
        })
    }

    async fn workspace_switch(
        &mut self,
        params: workspace::SwitchParams,
    ) -> Result<workspace::SwitchReply, RpcError> {
        let _ = params;
        Ok(workspace::SwitchReply {
            focused_pane: None,
            seq: self.seq,
        })
    }

    async fn pane_zoom(&mut self, params: pane::ZoomParams) -> Result<pane::ZoomReply, RpcError> {
        let _ = params;
        Ok(pane::ZoomReply {
            zoomed: true,
            seq: self.seq,
        })
    }

    async fn pane_swap(&mut self, params: pane::SwapParams) -> Result<pane::SwapReply, RpcError> {
        let _ = params;
        Ok(pane::SwapReply { seq: self.seq })
    }

    async fn pane_move(&mut self, params: pane::MoveParams) -> Result<pane::MoveReply, RpcError> {
        let _ = params;
        Ok(pane::MoveReply { seq: self.seq })
    }

    async fn pane_close(
        &mut self,
        params: pane::CloseParams,
    ) -> Result<pane::CloseReply, RpcError> {
        let _ = params;
        Ok(pane::CloseReply { seq: self.seq })
    }

    async fn pane_focus(
        &mut self,
        params: pane::FocusParams,
    ) -> Result<pane::FocusReply, RpcError> {
        let _ = params;
        Ok(pane::FocusReply {
            pane: Some(PaneId::new_v4()),
            seq: self.seq,
        })
    }

    async fn pane_resize(
        &mut self,
        params: pane::ResizeParams,
    ) -> Result<pane::ResizeReply, RpcError> {
        let _ = params;
        Ok(pane::ResizeReply {
            resized: true,
            seq: self.seq,
        })
    }

    async fn pane_rename(
        &mut self,
        params: pane::RenameParams,
    ) -> Result<pane::RenameReply, RpcError> {
        let _ = params;
        Ok(pane::RenameReply { seq: self.seq })
    }

    async fn session_report(
        &mut self,
        _params: session::ReportParams,
    ) -> Result<session::ReportReply, RpcError> {
        Ok(session::ReportReply {
            seq: self.seq,
            report: session::RestoreReport::default(),
            handoff: None,
        })
    }

    async fn session_handoff(
        &mut self,
        params: session::HandoffParams,
    ) -> Result<session::HandoffReply, RpcError> {
        // The stub refuses, which is a defined answer and the one shape a
        // caller must handle: acceptance is not completion, and a refusal is
        // the only outcome that arrives on this connection at all.
        Ok(session::HandoffReply {
            accepted: false,
            reason: Some(format!("{} is not a staged amx", params.binary.display())),
            seq: self.seq,
        })
    }

    async fn session_state(
        &mut self,
        _params: session::StateParams,
    ) -> Result<session::StateReply, RpcError> {
        let workspace = WorkspaceId::new_v4();
        let pane = PaneId::new_v4();
        Ok(session::StateReply {
            seq: self.seq,
            focused_workspace: Some(workspace),
            workspaces: vec![session::WorkspaceState {
                workspace,
                short: ShortNumber::FIRST,
                label: Some("dev".into()),
                layout: Layout::with_root(pane),
                focus: Some(pane),
                worktree: None,
            }],
            panes: vec![session::PaneState {
                pane,
                short: ShortNumber::FIRST,
                label: None,
                cwd: None,
                rows: 24,
                cols: 80,
                history_head: RowId::from_raw(3),
                history_floor: RowId::from_raw(0),
                agent: None,
            }],
            attention: Vec::new(),
            restore: None,
        })
    }

    async fn stream_bind(
        &mut self,
        params: stream::BindParams,
    ) -> Result<stream::BindReply, RpcError> {
        let _ = params;
        Ok(stream::BindReply {
            stream: StreamId::new(1),
            channel: 1,
            max_frame: 1 << 20,
        })
    }

    async fn pane_history(
        &mut self,
        params: stream::HistoryParams,
    ) -> Result<stream::HistoryReply, RpcError> {
        let _ = params;
        Ok(stream::HistoryReply {
            chunks: 1,
            seq: self.seq,
        })
    }

    async fn client_viewport(
        &mut self,
        params: client::Viewport,
    ) -> Result<client::ViewportReply, RpcError> {
        let _ = params;
        Ok(client::ViewportReply { seq: self.seq })
    }
    // ------------------------------------------------------ M2's twelve rows
    //
    // The stub answers every one, because the property this suite proves is
    // that the table *routes* — a row with no arm here would not compile, which
    // is the same guarantee `dispatch/mod.rs` gives the real server.

    async fn agent_report(
        &mut self,
        _params: Box<agent::ReportParams>,
    ) -> Result<agent::ReportReply, RpcError> {
        Ok(agent::ReportReply {
            accepted: true,
            seq: self.seq,
        })
    }

    async fn agent_start(
        &mut self,
        _params: agent::StartParams,
    ) -> Result<agent::StartReply, RpcError> {
        Ok(agent::StartReply {
            pane: PaneId::new_v4(),
            short: ShortNumber::FIRST,
            readiness: agent::Readiness::Ready,
            agent: None,
            seq: self.seq,
        })
    }

    async fn agent_prompt(
        &mut self,
        _params: agent::PromptParams,
    ) -> Result<agent::PromptReply, RpcError> {
        Ok(agent::PromptReply {
            pane: PaneId::new_v4(),
            agent: None,
            satisfied: true,
            submitted_seq: self.seq,
        })
    }

    async fn agent_explain(
        &mut self,
        _params: agent::ExplainParams,
    ) -> Result<agent::ExplainReply, RpcError> {
        Ok(agent::ExplainReply {
            pane: PaneId::new_v4(),
            kind: None,
            manifest: None,
            manifest_version: None,
            matched: None,
            region_preview: Vec::new(),
            rules: Vec::new(),
            agent: None,
        })
    }

    async fn agent_next(
        &mut self,
        _params: agent::NextParams,
    ) -> Result<agent::NextReply, RpcError> {
        Ok(agent::NextReply {
            pane: None,
            workspace: None,
            waiting: 0,
            seq: self.seq,
        })
    }

    async fn wait(&mut self, _params: wait::WaitParams) -> Result<wait::WaitReply, RpcError> {
        Ok(wait::WaitReply {
            pane: PaneId::new_v4(),
            satisfied: true,
            agent: None,
            status: None,
            seq: self.seq,
        })
    }

    async fn events_subscribe(
        &mut self,
        _params: wait::SubscribeParams,
    ) -> Result<wait::SubscribeReply, RpcError> {
        Ok(wait::SubscribeReply { seq: self.seq })
    }

    async fn pane_send_text(
        &mut self,
        _params: pane::SendTextParams,
    ) -> Result<pane::SendTextReply, RpcError> {
        Ok(pane::SendTextReply {
            pane: PaneId::new_v4(),
            seq: self.seq,
        })
    }

    async fn pane_send_keys(
        &mut self,
        params: pane::SendKeysParams,
    ) -> Result<pane::SendKeysReply, RpcError> {
        Ok(pane::SendKeysReply {
            pane: PaneId::new_v4(),
            keys: u32::try_from(params.keys.len()).unwrap_or(u32::MAX),
            seq: self.seq,
        })
    }

    async fn pane_run(&mut self, _params: pane::RunParams) -> Result<pane::RunReply, RpcError> {
        Ok(pane::RunReply {
            pane: PaneId::new_v4(),
            bracketed: false,
            seq: self.seq,
        })
    }

    async fn pane_read(&mut self, _params: pane::ReadParams) -> Result<pane::ReadReply, RpcError> {
        Ok(pane::ReadReply {
            pane: PaneId::new_v4(),
            rows: Vec::new(),
            seq: self.seq,
        })
    }

    async fn pane_wait_output(
        &mut self,
        _params: wait::WaitOutputParams,
    ) -> Result<wait::WaitOutputReply, RpcError> {
        Ok(wait::WaitOutputReply {
            pane: PaneId::new_v4(),
            matched: false,
            line: None,
            row: None,
            seq: self.seq,
        })
    }
}

#[test]
fn dispatch_routes_every_call_to_its_handler() {
    let mut server = StubServer { seq: 17 };

    let ping = block_on(dispatch(
        &mut server,
        Call::Ping(session::PingParams::default()),
    ))
    .unwrap();
    assert_eq!(ping["seq"], 17);

    let created = block_on(dispatch(
        &mut server,
        Call::WorkspaceCreate(workspace::CreateParams::default()),
    ))
    .unwrap();
    assert_eq!(created["short"], 1);

    let split = Call::PaneSplit(pane::SplitParams {
        pane: PaneId::new_v4(),
        direction: pane::SplitDirection::Vertical,
        command: None,
        cwd: None,
    });
    assert_eq!(split.method(), Method::PaneSplit);
    let reply = block_on(dispatch(&mut server, split)).unwrap();
    assert_eq!(reply["short"], 2);
}

#[test]
fn dispatch_routes_the_rest_of_the_m0_verb_surface() {
    let mut server = StubServer { seq: 5 };

    let renamed = block_on(dispatch(
        &mut server,
        Call::WorkspaceRename(workspace::RenameParams {
            workspace: WorkspaceId::new_v4(),
            label: "scratch".into(),
        }),
    ))
    .unwrap();
    assert_eq!(renamed["seq"], 5);

    let killed = block_on(dispatch(
        &mut server,
        Call::WorkspaceKill(workspace::KillParams {
            workspace: WorkspaceId::new_v4(),
        }),
    ))
    .unwrap();
    assert_eq!(killed["panes"], serde_json::json!([]));

    let switched = block_on(dispatch(
        &mut server,
        Call::WorkspaceSwitch(workspace::SwitchParams {
            workspace: WorkspaceId::new_v4(),
        }),
    ))
    .unwrap();
    assert_eq!(switched["focused_pane"], serde_json::Value::Null);

    let zoomed = block_on(dispatch(
        &mut server,
        Call::PaneZoom(pane::ZoomParams {
            pane: PaneId::new_v4(),
        }),
    ))
    .unwrap();
    assert_eq!(zoomed["zoomed"], true);

    let swapped = block_on(dispatch(
        &mut server,
        Call::PaneSwap(pane::SwapParams {
            pane: PaneId::new_v4(),
            with: PaneId::new_v4(),
        }),
    ))
    .unwrap();
    assert_eq!(swapped["seq"], 5);

    let moved = block_on(dispatch(
        &mut server,
        Call::PaneMove(pane::MoveParams {
            pane: PaneId::new_v4(),
            to: WorkspaceId::new_v4(),
        }),
    ))
    .unwrap();
    assert_eq!(moved["seq"], 5);

    let closed = block_on(dispatch(
        &mut server,
        Call::PaneClose(pane::CloseParams {
            pane: PaneId::new_v4(),
        }),
    ))
    .unwrap();
    assert_eq!(closed["seq"], 5);

    let focused = block_on(dispatch(
        &mut server,
        Call::PaneFocus(pane::FocusParams {
            workspace: WorkspaceId::new_v4(),
            direction: pane::MoveDirection::Left,
        }),
    ))
    .unwrap();
    assert_ne!(focused["pane"], serde_json::Value::Null);

    let resized = block_on(dispatch(
        &mut server,
        Call::PaneResize(pane::ResizeParams {
            pane: PaneId::new_v4(),
            direction: pane::MoveDirection::Right,
            delta: 0.05,
        }),
    ))
    .unwrap();
    assert_eq!(resized["resized"], true);

    let state = block_on(dispatch(
        &mut server,
        Call::SessionState(session::StateParams {}),
    ))
    .unwrap();
    assert_eq!(state["seq"], 5);
    assert_eq!(state["workspaces"].as_array().unwrap().len(), 1);

    let bound = block_on(dispatch(
        &mut server,
        Call::StreamBind(stream::BindParams {
            kind: StreamKind::PaneGrid {
                pane: PaneId::new_v4(),
            },
            generation: None,
        }),
    ))
    .unwrap();
    assert_eq!(bound["channel"], 1);

    let fetched = block_on(dispatch(
        &mut server,
        Call::PaneHistory(stream::HistoryParams {
            pane: PaneId::new_v4(),
            first: RowId::from_raw(0),
            last: RowId::from_raw(3),
            request: 7,
        }),
    ))
    .unwrap();
    assert_eq!(fetched["chunks"], 1);

    let viewed = block_on(dispatch(
        &mut server,
        Call::ClientViewport(client::Viewport {
            rows: 40,
            cols: 120,
            panes: vec![],
        }),
    ))
    .unwrap();
    assert_eq!(viewed["seq"], 5);
}

#[test]
fn a_handler_error_becomes_the_calls_error_not_a_panic() {
    let mut server = StubServer { seq: 0 };
    let call = Call::PaneSplit(pane::SplitParams {
        pane: PaneId::new_v4(),
        direction: pane::SplitDirection::Horizontal,
        command: None,
        cwd: None,
    });
    let error = block_on(dispatch(&mut server, call)).unwrap_err();
    assert_eq!(error.code, RpcError::INTERNAL_ERROR);
}

/// Poll a future that never yields, so the test needs no runtime.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("stub handlers never yield"),
    }
}
