//! T09: the root supervisor and the `Core` actor's batching/publishing
//! contract.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;
use std::sync::Arc;

use amx_core::{Bus, Ctx, Level, Scheduled, SessionName, ShortNumber};
use amx_proto::control::workspace;
use amx_server::actor::CoreCommand;
use amx_server::actor::WorkspaceCall;
use amx_server::actor::core::Core;
use amx_server::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A `Ctx` with fabricated, never-touched-on-disk paths, distinguished by
/// `tag` so parallel tests never share a runtime/state/socket path.
fn test_ctx(tag: &str) -> Ctx {
    let root = PathBuf::from("/amx-test").join(tag);
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        runtime_dir: root.join("runtime"),
        socket: root.join("runtime/sock"),
        state_dir: root.join("state"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn cancellation_token_shuts_every_task_down_and_join_set_reports_no_leaks() {
    let ctx = test_ctx("cancel");
    let mut runtime = Runtime::new(ctx);

    for _ in 0..3 {
        let cancel = runtime.ctx().cancel.clone();
        runtime.spawn(async move {
            cancel.cancelled().await;
        });
    }

    let report = runtime.shutdown().await;
    assert_eq!(report.joined, 3, "every spawned task must be joined");
    assert!(report.clean(), "no task should have panicked");
}

#[tokio::test]
async fn no_task_is_detached() {
    let ctx = test_ctx("detach");
    let mut runtime = Runtime::new(ctx);
    assert_eq!(runtime.task_count(), 0);

    for _ in 0..5 {
        let cancel = runtime.ctx().cancel.clone();
        runtime.spawn(async move {
            cancel.cancelled().await;
        });
    }
    assert_eq!(
        runtime.task_count(),
        5,
        "every spawn() call must land in the JoinSet"
    );

    let report = runtime.shutdown().await;
    // If any task had been spawned outside `Runtime::spawn` (a detached
    // `tokio::spawn`), the JoinSet would still report exactly 5 here while a
    // sixth task kept running unobserved. This only proves the ones we did
    // track were tracked; the absence of a stray task is what the module
    // being the sole `tokio::spawn` call site in the crate guarantees.
    assert_eq!(report.joined, 5);
}

#[test]
fn effects_fold_across_a_batch_into_one_scheduled_output() {
    let ctx = test_ctx("fold");
    let mut core = Core::new(ctx);

    for _ in 0..3 {
        let (reply, _answer) = oneshot::channel();
        core.absorb(CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        }));
    }

    let mut scheduled = Scheduled::new();
    core.drain_scheduled(&mut scheduled);
    assert_eq!(
        scheduled.level(),
        Level::Layout,
        "three creates fold into one Layout-level batch, not three separate ones"
    );

    let mut next = Scheduled::new();
    core.drain_scheduled(&mut next);
    assert!(
        next.is_empty(),
        "a batch with no new commands must not resurface the previous one's effects"
    );
}

#[test]
fn core_actor_publishes_one_event_per_state_transition() {
    let ctx = test_ctx("events");
    let bus = ctx.bus.clone();
    let mut core = Core::new(ctx);
    assert_eq!(bus.head(), 0);

    for _ in 0..3 {
        let (reply, _answer) = oneshot::channel();
        core.absorb(CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        }));
    }

    assert_eq!(
        bus.head(),
        3,
        "each create is one transition and must publish exactly one event"
    );
}

#[test]
fn ctx_paths_are_derived_from_the_passed_env_not_process_env() {
    let a = test_ctx("session-a");
    let b = test_ctx("session-b");

    let runtime_a = Runtime::new(a.clone());
    let runtime_b = Runtime::new(b.clone());

    // A runtime's paths are exactly what its Ctx carried in, nothing
    // resolved afresh (there is no `std::env::var` call anywhere in
    // `runtime.rs`/`actor/core.rs` for a path to come from instead).
    assert_eq!(runtime_a.ctx().runtime_dir, a.runtime_dir);
    assert_eq!(runtime_a.ctx().socket, a.socket);
    assert_eq!(runtime_a.ctx().state_dir, a.state_dir);

    // Two sessions built from two independent Ctx values never collide,
    // which is only possible if neither fell back to a shared process-global
    // source for any of these paths.
    assert_ne!(runtime_a.ctx().runtime_dir, runtime_b.ctx().runtime_dir);
    assert_ne!(runtime_a.ctx().socket, runtime_b.ctx().socket);
    assert_ne!(runtime_a.ctx().state_dir, runtime_b.ctx().state_dir);
}

#[tokio::test]
async fn core_actor_runs_under_the_runtime_and_answers_calls() {
    let ctx = test_ctx("wire");
    let mut runtime = Runtime::new(ctx.clone());
    let core = Core::new(ctx);

    let (tx, rx) = mpsc::channel(8);
    let (scheduled_tx, mut scheduled_rx) = mpsc::unbounded_channel();
    let sink = move |scheduled: &Scheduled| {
        let _ = scheduled_tx.send(scheduled.level());
    };

    runtime.spawn(core.run(rx, sink));
    assert_eq!(runtime.task_count(), 1);

    let (reply, answer) = oneshot::channel();
    tx.send(CoreCommand::Workspace(WorkspaceCall::Create {
        params: workspace::CreateParams::default(),
        reply,
    }))
    .await
    .expect("mailbox open");

    let created = answer
        .await
        .expect("core replied")
        .expect("create succeeded");
    assert_eq!(created.short, ShortNumber::FIRST);

    let level = scheduled_rx
        .recv()
        .await
        .expect("the batch scheduled output before answering the next recv");
    assert_eq!(level, Level::Layout);

    drop(tx);
    let report = runtime.shutdown().await;
    assert_eq!(report.joined, 1);
    assert!(report.clean());
}
