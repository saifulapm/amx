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

use amx_core::{PaneId, SessionId, ShortNumber, WorkspaceId};
use amx_proto::control::{Call, Dispatch, Method, dispatch, pane, session, workspace};
use amx_proto::hello::ServerInfo;
use amx_proto::rpc::RpcError;

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
