//! The persistence seams, before anything fills them.
//!
//! U01's job is that waves 1–3 build against signatures that already compile
//! and already behave. Two of those signatures are only reachable through a
//! mailbox, so this file drives them: `SessionCall::Capture` must be *answered*
//! by `Core`'s routing (a dropped reply channel is a hang for whoever asked),
//! and `PersistHandle` must behave like the other actor handles do when its
//! actor is not there.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;
use std::sync::Arc;

use amx_core::{Bus, Ctx, SessionName};
use amx_server::actor::core::Core;
use amx_server::actor::{
    CoreCommand, CoreHandle, MailboxError, PersistCommand, PersistHandle, SessionCall,
};
use amx_server::persist::Snapshot;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A `Ctx` with fabricated, never-touched-on-disk paths.
fn fresh_ctx(tag: &str) -> Ctx {
    let root = PathBuf::from("/amx-test-persist-mailbox").join(tag);
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        runtime_dir: root.join("runtime"),
        socket: root.join("runtime/sock"),
        state_dir: root.join("state"),
        config_path: root.join("config/amx/config.toml"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

#[test]
fn a_capture_request_is_answered_through_the_core_mailbox() {
    let ctx = fresh_ctx("capture");
    let (tx, _rx) = mpsc::channel(8);
    let mut core = Core::new(ctx, CoreHandle::new(tx));

    for sidecars in [false, true] {
        let (reply, mut answer) = oneshot::channel();
        core.absorb(CoreCommand::Session(SessionCall::Capture {
            sidecars,
            reply,
        }));
        // `absorb` never awaits, so the answer is already there — a `try_recv`
        // that comes back empty means the arm dropped the channel, which is a
        // hang for whoever asked.
        let capture = answer
            .try_recv()
            .expect("core must answer a capture rather than drop the channel");
        // U06 fills the body; what U01 owns is that the arm exists and the
        // caller is never left waiting on a channel nobody will send on.
        assert!(capture.snapshot.is_empty());
        assert!(capture.panes.is_empty());
    }
}

#[tokio::test]
async fn the_persist_handle_reports_a_missing_actor_rather_than_waiting() {
    let (tx, rx) = mpsc::channel(1);
    let handle = PersistHandle::new(tx);
    drop(rx);

    assert!(handle.is_closed());
    assert_eq!(
        handle
            .try_send(PersistCommand::Snapshot(Box::new(Snapshot::empty())))
            .unwrap_err(),
        MailboxError::Gone,
        "the shutdown push must fail fast, never block Core's own drain",
    );
    assert_eq!(handle.flush().await.unwrap_err(), MailboxError::Gone);
}

#[tokio::test]
async fn a_flush_waits_for_the_write_to_be_acknowledged() {
    let (tx, mut rx) = mpsc::channel(1);
    let handle = PersistHandle::new(tx);

    let flushing = tokio::spawn(async move { handle.flush().await });
    let PersistCommand::Flush { reply } = rx.recv().await.expect("the flush arrives") else {
        panic!("flush must arrive as Flush, not as a snapshot push");
    };
    let _ = reply.send(Ok(()));

    assert!(
        flushing.await.expect("the flush task joins").is_ok(),
        "a flush that was acknowledged is not a mailbox failure",
    );
}
