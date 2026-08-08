//! Restore: putting yesterday's session back (U06).
//!
//! Restore runs at startup, before the gateway accepts anyone (D-M1-9).
//! Everything asserted here is asserted against real state and real
//! processes — a restored pane is a pty with a shell on the far end. What it
//! does when a pane will *not* come back is `tests/restore_loss.rs`; what
//! capture writes is `tests/capture.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{Layout, PaneId, WorkspaceId};
use amx_proto::control::session::{RestoreEntity, RestoreSeverity, StateReply};
use amx_server::actor::{CoreCommand, SessionCall};
use amx_server::persist::io::SyncAll;
use amx_server::persist::{Snapshot, sidecar, snapshot};
use amx_server::session::serve::StopOn;
use amx_server::session::{daemon, serve};
use serde_json::json;

mod support;

use support::restore_rig::{
    Fixture, entries, pane_row, report_of, row_of, save_sidecar, screen, snapshot_of, wait_until,
    workspace_row,
};
use support::{PATIENCE, TempDir, ctx_under, result_of};

#[tokio::test]
async fn restore_rebuilds_workspaces_panes_and_shorts_with_the_same_uuids() {
    let mut fx = Fixture::new("ids");
    let (build, notes) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let (a, b, c) = (PaneId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let saved = snapshot_of(
        vec![
            workspace_row(build, 4, Some("build"), row_of(&[a, b]), Some(b)),
            workspace_row(notes, 7, None, Layout::with_root(c), Some(c)),
        ],
        vec![
            pane_row(a, 11, Some("editor"), Some(work.clone())),
            pane_row(b, 12, None, Some(work.clone())),
            pane_row(c, 13, Some("notes"), Some(work)),
        ],
    );

    let opts = fx.opts();
    let summary = fx.core.restore(saved, &opts);
    assert_eq!(summary.restored, 5, "two workspaces and three panes");
    assert!(summary.is_clean(), "nothing here should have been lost");

    let running = fx.start();
    let state = running.state().await;
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|ws| ws.workspace)
            .collect::<Vec<_>>(),
        vec![build, notes],
    );
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|ws| ws.short.get())
            .collect::<Vec<_>>(),
        vec![4, 7],
        "short numbers are stable across restarts (04 §6)",
    );
    assert_eq!(state.workspaces[0].label.as_deref(), Some("build"));
    assert_eq!(state.workspaces[0].focus, Some(b));
    assert_eq!(state.focused_workspace, Some(build));
    assert_eq!(
        state
            .panes
            .iter()
            .map(|pane| (pane.pane, pane.short.get()))
            .collect::<Vec<_>>(),
        vec![(a, 11), (b, 12), (c, 13)],
    );
    assert_eq!(state.panes[0].label.as_deref(), Some("editor"));
    // Every restored pane has a live process behind it, not just a rectangle.
    for pane in [a, b, c] {
        let _ = running.wiring_of(pane).await;
    }
    assert!(state.restore.is_some_and(|restore| restore.is_clean()));

    running.into_core().await;
}

#[tokio::test]
async fn a_new_workspace_after_a_restore_takes_the_lowest_free_short() {
    // 04 §6's mapping is lowest-free, so a create after a restore fills the
    // holes the snapshot left and steps over what came back. Stepping over is
    // the load-bearing half: a create that reused a restored number would give
    // two objects one number, and the second would be unaddressable.
    let mut fx = Fixture::new("next");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                4,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 9, None, Some(work))],
        ),
        &opts,
    );

    let running = fx.start();
    let mut created = Vec::new();
    for _ in 0..4 {
        let reply: amx_proto::control::workspace::CreateReply = running
            .call(|reply| {
                CoreCommand::Workspace(amx_server::actor::WorkspaceCall::Create {
                    params: amx_proto::control::workspace::CreateParams {
                        label: None,
                        focus: false,
                        worktree: None,
                    },
                    reply,
                })
            })
            .await;
        created.push(reply.short.get());
    }
    assert_eq!(
        created,
        vec![1, 2, 3, 5],
        "the free numbers below the restored 4, and then past it",
    );

    let state = running.state().await;
    let mut shorts: Vec<u32> = state.panes.iter().map(|pane| pane.short.get()).collect();
    shorts.sort_unstable();
    assert_eq!(
        shorts,
        vec![1, 2, 3, 4, 9],
        "each new workspace's root pane took the lowest free number too, and \
         the restored pane kept 9",
    );

    running.into_core().await;
}

#[tokio::test]
async fn a_pane_the_snapshot_has_no_row_for_takes_no_other_panes_number() {
    // A layout naming a pane the snapshot has no record of is already a
    // degraded restore (D-M1-9), and the pane comes back anyway — but it must
    // not be handed a number a pane further down the file is about to claim.
    // Assignment is lowest-free, so settling the recorded numbers first is the
    // whole of what keeps the two apart.
    let mut fx = Fixture::new("orphan");
    let (ws, orphan, recorded) = (WorkspaceId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                row_of(&[orphan, recorded]),
                Some(recorded),
            )],
            // Only the second pane has a row, and it holds number 1 — the
            // number the first pane would otherwise be given on the way past.
            vec![pane_row(recorded, 1, None, Some(work))],
        ),
        &opts,
    );
    assert_eq!(summary.degraded, 1, "the pane with no row is degraded");

    let running = fx.start();
    let state = running.state().await;
    let shorts: Vec<(PaneId, u32)> = state
        .panes
        .iter()
        .map(|pane| (pane.pane, pane.short.get()))
        .collect();
    assert_eq!(
        shorts,
        vec![(recorded, 1), (orphan, 2)],
        "the recorded number went back where it was, and the pane without one \
         took the lowest still free",
    );

    running.into_core().await;
}

#[tokio::test]
async fn restored_session_suppresses_the_first_attach_seed() {
    let mut fx = Fixture::new("seed");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let opts = fx.opts();
    fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(work))],
        ),
        &opts,
    );

    let running = fx.start();
    // Two attaches, the way two clients would: the seed is already a no-op for
    // a session that has workspaces, and a restored session is one.
    for _ in 0..2 {
        running
            .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
            .await;
    }
    let state = running.state().await;
    assert_eq!(state.workspaces.len(), 1, "restore must not be re-seeded");
    assert_eq!(state.workspaces[0].workspace, ws);
    assert_eq!(state.panes.len(), 1);
    assert_eq!(state.panes[0].pane, pane);

    running.into_core().await;
}

#[tokio::test]
async fn empty_or_missing_snapshot_falls_through_to_the_normal_seed() {
    // No file at all.
    let mut fx = Fixture::new("fresh");
    let opts = fx.opts();
    assert!(
        fx.core.restore_from_disk(&opts).is_none(),
        "a session that was never saved has nothing to restore",
    );

    // And a file describing a session with nothing in it, which is what a
    // server that was stopped before its first workspace leaves behind.
    snapshot::save(&fx.ctx.state_dir, &Snapshot::empty(), &SyncAll).expect("write the snapshot");
    assert!(fx.core.restore_from_disk(&opts).is_none());

    let running = fx.start();
    running
        .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
        .await;
    let state = running.state().await;
    assert_eq!(state.workspaces.len(), 1, "the ordinary first-attach seed");
    assert!(
        state.restore.is_none(),
        "a server that restored nothing must not claim it restored something",
    );

    running.into_core().await;
}
// ------------------------------------------------------------------ sidecars
#[tokio::test]
async fn saved_scrollback_is_replayed_into_the_restored_pane() {
    let mut fx = Fixture::new("replay");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    // The second line was soft-wrapped when it was saved: replay joins it back
    // into one logical line and lets the pane's current width re-wrap it.
    save_sidecar(
        &fx.ctx.state_dir,
        pane,
        &[
            ("cargo test --all", false),
            ("running 3 ", true),
            ("tests", false),
        ],
    );

    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(work))],
        ),
        &opts,
    );
    assert!(summary.is_clean(), "a readable sidecar costs nothing");

    let running = fx.start();
    let wiring = running.wiring_of(pane).await;
    wait_until(
        "the restored scrollback is on the pane's grid",
        async || {
            let text = screen(&wiring.frames.latest());
            text.contains("cargo test --all") && text.contains("running 3 tests")
        },
    )
    .await;

    running.into_core().await;
}

#[tokio::test]
async fn an_unreadable_sidecar_degrades_the_pane_instead_of_losing_it() {
    let mut fx = Fixture::new("badside");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let damaged = sidecar::path(&fx.ctx.state_dir, pane);
    std::fs::create_dir_all(sidecar::dir(&fx.ctx.state_dir)).expect("create history/");
    std::fs::write(&damaged, b"this is not a sidecar").expect("write the damaged sidecar");

    let opts = fx.opts();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 1, None, Some(work))],
        ),
        &opts,
    );
    assert_eq!(
        (summary.restored, summary.lost, summary.degraded),
        (2, 0, 1),
        "a pane's history is not the pane",
    );

    let running = fx.start();
    let report = report_of(&running).await;
    let degraded = entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane);
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].pane, Some(pane));
    assert_eq!(degraded[0].path.as_deref(), Some(damaged.as_path()));
    let _ = running.wiring_of(pane).await;

    running.into_core().await;
}
// ------------------------------------------------------------ the serve path

#[tokio::test]
async fn the_server_restores_its_snapshot_before_it_accepts_a_client() {
    // The whole path, from a file on disk to what the first client is told.
    // Restore slots between the bind and the accept loop (D-M1-9), so the
    // earliest connection that can exist already sees the restored session —
    // there is no window in which a client attaches to an empty one and has it
    // seeded out from under the restore.
    let dir = TempDir::new("srv");
    let ctx = ctx_under(dir.path());
    std::fs::create_dir_all(&ctx.state_dir).expect("create the state dir");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).expect("create the pane's cwd");

    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    snapshot::save(
        &ctx.state_dir,
        &snapshot_of(
            vec![workspace_row(
                ws,
                2,
                Some("build"),
                Layout::with_root(pane),
                Some(pane),
            )],
            vec![pane_row(pane, 3, Some("editor"), Some(work))],
        ),
        &SyncAll,
    )
    .expect("write the snapshot");

    let served = tokio::spawn(serve::serve(ctx.clone(), StopOn::Cancellation));
    daemon::await_ready(&ctx.socket, PATIENCE)
        .await
        .expect("the server bound its socket");

    let mut client = support::connect_to(&ctx.socket).await;
    // An *attaching* hello: the seed runs before the welcome is written, and
    // has to find the restored workspace already there.
    client.hello_as_attach(amx_proto::version::window()).await;
    let reply = client.request(1, "session.state", json!({})).await;
    let state: StateReply =
        serde_json::from_value(result_of(&reply).clone()).expect("decode session.state");

    assert_eq!(state.workspaces.len(), 1, "restored, not re-seeded");
    assert_eq!(state.workspaces[0].workspace, ws);
    assert_eq!(state.workspaces[0].short.get(), 2);
    assert_eq!(state.workspaces[0].label.as_deref(), Some("build"));
    assert_eq!(state.panes[0].pane, pane);
    assert_eq!(state.panes[0].short.get(), 3);
    assert_eq!(state.panes[0].label.as_deref(), Some("editor"));
    assert!(
        state.restore.is_some_and(|restore| restore.is_clean()),
        "a clean restore still says it restored: the indicator needs the counts",
    );

    ctx.cancel.cancel();
    let report = served
        .await
        .expect("the serve task finished")
        .expect("the server ran");
    assert!(report.clean(), "every task and connection joined");
}
