//! What a restore could not do, and how it says so (U06).
//!
//! 04 §6: panes and workspaces that fail to respawn "produce a restore report
//! shown in the status line and queryable via `amx session report` — never
//! log-only". So every failure here is checked for its *report entry*, not for
//! a log line and not merely for the absence of a crash: a restore that quietly
//! dropped a workspace would pass a test that only counted survivors.
//!
//! The table under test is D-M1-9's — a vanished cwd degrades, a failed spawn
//! prunes the pane, an emptied workspace prunes, an unreadable snapshot is a
//! whole-session loss — plus the two surfaces that carry it: `session.report`
//! for the entries and `session.state` for the counts the client's indicator
//! reads (U07 renders it).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_core::{Event, Layout, PaneId, WorkspaceId};
use amx_proto::control::session::{self, RestoreEntity, RestoreSeverity, StateReply};
use amx_server::actor::{CoreCommand, CoreHandle, SessionCall};
use amx_server::dispatch::Router;
use amx_server::persist::{Snapshot, VERSION, snapshot};
use serde_json::json;

mod support;

use support::PATIENCE;
use support::restore_rig::{
    Fixture, entries, pane_row, report_of, row_of, snapshot_of, workspace_row,
};

// ----------------------------------------------------- prune-and-report table

#[tokio::test]
async fn missing_cwd_respawns_in_home_and_reports_degraded() {
    let mut fx = Fixture::new("degr");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let gone = fx.missing("a-directory-that-was-deleted");
    let home = fx.home();

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
            vec![pane_row(pane, 1, Some("editor"), Some(gone.clone()))],
        ),
        &opts,
    );

    assert_eq!(summary.restored, 2, "the workspace and its pane came back");
    assert_eq!((summary.lost, summary.degraded), (0, 1));

    let running = fx.start();
    let report = report_of(&running).await;
    let degraded = entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane);
    assert_eq!(degraded.len(), 1);
    assert_eq!(degraded[0].pane, Some(pane));
    assert_eq!(degraded[0].label.as_deref(), Some("editor"));
    assert_eq!(
        degraded[0].path.as_deref(),
        Some(gone.as_path()),
        "the entry names the directory that vanished, not the one it settled for",
    );

    // The pane is alive, in the home directory it degraded into. Compared
    // canonicalized: the live process reports its real cwd, and darwin's
    // $TMPDIR reaches the same directory through the /var -> /private/var
    // symlink the expectation was built from.
    let capture = running.capture().await;
    assert_eq!(
        capture.snapshot.panes[0]
            .cwd
            .as_deref()
            .map(|p| p.canonicalize().expect("the settled cwd exists")),
        Some(home.canonicalize().expect("the home directory exists"))
    );
    let _ = running.wiring_of(pane).await;

    running.into_core().await;
}

#[tokio::test]
async fn spawn_failure_prunes_the_pane_and_reports_lost() {
    let mut fx = Fixture::new("lost");
    let (ws, good, doomed) = (WorkspaceId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let gone = fx.missing("a-directory-that-was-deleted");

    // The home it would fall back to does not exist either, so the respawn
    // fails in `chdir` — a real spawn failure, not a simulated one.
    let opts = fx.opts_without_home();
    let summary = fx.core.restore(
        snapshot_of(
            vec![workspace_row(
                ws,
                1,
                None,
                row_of(&[good, doomed]),
                Some(doomed),
            )],
            vec![
                pane_row(good, 1, None, Some(work)),
                pane_row(doomed, 2, Some("agent"), Some(gone.clone())),
            ],
        ),
        &opts,
    );

    assert_eq!(summary.restored, 2, "the workspace and its surviving pane");
    assert_eq!((summary.lost, summary.degraded), (1, 0));

    let running = fx.start();
    let report = report_of(&running).await;
    let lost = entries(&report, RestoreSeverity::Lost, RestoreEntity::Pane);
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].pane, Some(doomed));
    assert_eq!(lost[0].label.as_deref(), Some("agent"));
    assert!(
        !lost[0].reason.is_empty(),
        "a loss the user reads has to say why",
    );
    assert!(
        entries(&report, RestoreSeverity::Degraded, RestoreEntity::Pane).is_empty(),
        "a pane that was pruned was not also respawned somewhere else",
    );

    // The layout collapsed onto the survivor, exactly as a close would leave it.
    let state = running.state().await;
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(
        state.panes.iter().map(|pane| pane.pane).collect::<Vec<_>>(),
        vec![good]
    );
    assert_eq!(
        state.workspaces[0].focus,
        Some(good),
        "focus that pointed at the pruned pane moves to what is left",
    );

    running.into_core().await;
}

#[tokio::test]
async fn workspace_losing_every_pane_is_pruned_and_reported() {
    let mut fx = Fixture::new("wsprune");
    let (kept, doomed) = (WorkspaceId::new_v4(), WorkspaceId::new_v4());
    let (alive, dead) = (PaneId::new_v4(), PaneId::new_v4());
    let work = fx.dir_named("work");
    let gone = fx.missing("a-directory-that-was-deleted");

    let opts = fx.opts_without_home();
    let summary = fx.core.restore(
        Snapshot {
            version: VERSION,
            // The focused workspace is the one about to be pruned: focus has to
            // land somewhere real rather than on a workspace that is gone.
            focused_workspace: Some(doomed),
            workspaces: vec![
                workspace_row(kept, 1, None, Layout::with_root(alive), Some(alive)),
                workspace_row(
                    doomed,
                    2,
                    Some("agents"),
                    Layout::with_root(dead),
                    Some(dead),
                ),
            ],
            panes: vec![
                pane_row(alive, 1, None, Some(work)),
                pane_row(dead, 2, None, Some(gone)),
            ],
        },
        &opts,
    );

    assert_eq!(summary.restored, 2, "one workspace and one pane");
    assert_eq!(
        (summary.lost, summary.degraded),
        (2, 0),
        "the pane that could not spawn, and the workspace it emptied",
    );

    let running = fx.start();
    let report = report_of(&running).await;
    let lost_ws = entries(&report, RestoreSeverity::Lost, RestoreEntity::Workspace);
    assert_eq!(lost_ws.len(), 1);
    assert_eq!(lost_ws[0].workspace, Some(doomed));
    assert_eq!(lost_ws[0].label.as_deref(), Some("agents"));

    let state = running.state().await;
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|ws| ws.workspace)
            .collect::<Vec<_>>(),
        vec![kept],
    );
    assert_eq!(
        state.focused_workspace,
        Some(kept),
        "a session left with workspaces and no focus would render as nothing",
    );

    running.into_core().await;
}

#[tokio::test]
async fn a_snapshot_from_a_newer_amx_is_a_whole_session_loss() {
    let mut fx = Fixture::new("newer");
    let bytes = serde_json::to_vec(&json!({
        "version": VERSION + 1,
        "workspaces": [],
        "panes": [],
    }))
    .expect("encode the future snapshot");
    std::fs::write(snapshot::path(&fx.ctx.state_dir), bytes).expect("write the snapshot");

    let opts = fx.opts();
    let summary = fx
        .core
        .restore_from_disk(&opts)
        .expect("a snapshot that cannot be read is a loss, not an absence");
    assert_eq!((summary.restored, summary.lost), (0, 1));

    let running = fx.start();
    let report = report_of(&running).await;
    let lost = entries(&report, RestoreSeverity::Lost, RestoreEntity::Session);
    assert_eq!(lost.len(), 1);
    assert!(
        lost[0].reason.contains("newer amx"),
        "the user is told which way the version window failed: {}",
        lost[0].reason,
    );

    // The session still starts, and still seeds: a refused start would be a
    // worse answer than a fresh session with a report.
    running
        .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
        .await;
    assert_eq!(running.state().await.workspaces.len(), 1);

    running.into_core().await;
}
// ----------------------------------------------------------------- the wire

#[tokio::test]
async fn session_report_returns_entries_and_session_state_carries_counts() {
    let mut fx = Fixture::new("wire");
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let gone = fx.missing("a-directory-that-was-deleted");
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
            vec![pane_row(pane, 1, Some("editor"), Some(gone.clone()))],
        ),
        &opts,
    );

    let running = fx.start();
    // Through the real dispatch table, the way a client reaches it.
    let mut router = Router::new(CoreHandle::new(running.tx.clone()));

    let reply = amx_server::dispatch::handle(&mut router, "session.report", Some(json!({})))
        .await
        .expect("session.report is implemented");
    let report: session::ReportReply = serde_json::from_value(reply).expect("decode the report");
    assert_eq!(report.report.entries.len(), 1);
    let entry = &report.report.entries[0];
    assert_eq!(entry.severity, RestoreSeverity::Degraded);
    assert_eq!(entry.entity, RestoreEntity::Pane);
    assert_eq!(entry.label.as_deref(), Some("editor"));
    assert_eq!(entry.path.as_deref(), Some(gone.as_path()));

    let reply = amx_server::dispatch::handle(&mut router, "session.state", Some(json!({})))
        .await
        .expect("session.state answers");
    let state: StateReply = serde_json::from_value(reply).expect("decode the state");
    let summary = state.restore.expect("a server that restored says so");
    assert_eq!(
        (summary.restored, summary.lost, summary.degraded),
        (2, 0, 1)
    );
    assert!(!summary.is_clean(), "this is what the ⚠ indicator reads");

    running.into_core().await;
}

#[tokio::test]
async fn restore_publishes_one_session_restored_with_the_counts() {
    let mut fx = Fixture::new("bus");
    let mut events = fx.ctx.bus.subscribe();
    let (ws, pane) = (WorkspaceId::new_v4(), PaneId::new_v4());
    let gone = fx.missing("a-directory-that-was-deleted");
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
            vec![pane_row(pane, 1, None, Some(gone))],
        ),
        &opts,
    );

    // `SessionRestored` is the last thing restore publishes, so draining up to
    // it also proves the per-entity events came first — a client that replays
    // the bus sees the workspace and the pane appear before it is told what the
    // restore cost.
    let mut created = 0;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let summary = loop {
        let delivery = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an event before the deadline")
            .expect("the bus is open");
        let amx_core::Delivery::Event(envelope) = delivery else {
            panic!("the test fell behind the bus");
        };
        match envelope.event {
            Event::WorkspaceCreated { .. } | Event::PaneCreated { .. } => created += 1,
            event @ Event::SessionRestored { .. } => break event,
            _ => {}
        }
    };
    assert_eq!(
        created, 2,
        "restore publishes the ordinary per-entity events too",
    );
    assert_eq!(
        summary,
        Event::SessionRestored {
            workspaces: 1,
            panes: 1,
            lost: 0,
            degraded: 1,
        },
    );

    fx.drain().await;
}
