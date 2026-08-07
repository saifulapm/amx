//! T16: the M0 verb surface — workspace create/rename/kill/switch, pane
//! split/zoom/swap/move/close — driven straight at `Core`'s mailbox.
//!
//! `workspace.create`'s reply carries no pane id (04 §4's frozen wire types),
//! so nothing over the wire can name a workspace's root pane to split from —
//! `Core::state()` is the only way to learn it, and that is only reachable on
//! an owned `Core` before it is handed to [`Core::run`] or after `run` hands
//! it back. These tests drive `Core` directly rather than through the
//! `Gateway`/socket stack T10 built, the same pattern `tests/runtime.rs`
//! already established, for exactly that reason.
//!
//! The two behaviors that carry the value — a split inheriting the source
//! pane's live foreground-process cwd (04 §7), and swap/move never touching
//! the process behind a pane — are proven against a *real* spawned process,
//! identified independently of anything `Core` reports by scanning `/proc`
//! for the pid whose `cwd` matches a directory the test controls. That is
//! more work than trusting a reply, but a reply is exactly what a buggy
//! implementation could get right by accident (e.g. copying a pane's
//! recorded cwd instead of reading its live foreground process); an
//! independently observed pid is not.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use amx_core::{Bus, Ctx, PaneId, Scheduled, SessionName, ShortNumber, WorkspaceId};
use amx_proto::RpcError;
use amx_proto::control::{pane, workspace};
use amx_server::actor::core::Core;
use amx_server::actor::{CoreCommand, CoreHandle, PaneCall, WorkspaceCall};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

mod support;

use support::TempDir;

/// How long a test waits for a spawned process to show up (or disappear) in
/// `/proc`.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a poll loop waits between looks at its condition.
const TICK: Duration = Duration::from_millis(5);

/// How long a loop whose every look has a side effect waits between tries.
const RETRY: Duration = Duration::from_millis(50);

/// A `Ctx` with fabricated, never-touched-on-disk paths — nothing here binds
/// a socket, so there is no filesystem state for it to collide over.
fn fresh_ctx(tag: &str) -> Ctx {
    let root = PathBuf::from("/amx-test-verbs").join(tag);
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

/// Send one command and decode its typed reply.
async fn call<T>(
    tx: &mpsc::Sender<CoreCommand>,
    make: impl FnOnce(oneshot::Sender<Result<T, RpcError>>) -> CoreCommand,
) -> Result<T, RpcError> {
    let (reply, answer) = oneshot::channel();
    tx.send(make(reply)).await.expect("core mailbox is open");
    answer.await.expect("core answered")
}

/// Split `source`, asserting the call succeeds.
async fn split_from(tx: &mpsc::Sender<CoreCommand>, params: pane::SplitParams) -> pane::SplitReply {
    call(tx, |reply| {
        CoreCommand::Pane(PaneCall::Split { params, reply })
    })
    .await
    .expect("split succeeds")
}

fn split_params(source: PaneId, cwd: Option<&Path>, command: Option<&[&str]>) -> pane::SplitParams {
    pane::SplitParams {
        pane: source,
        direction: pane::SplitDirection::Vertical,
        command: command.map(|argv| argv.iter().map(|s| (*s).to_owned()).collect()),
        cwd: cwd.map(Path::to_path_buf),
    }
}

/// A `Core`, running, and the sender a dispatch-layer caller would use.
struct Harness {
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    task: JoinHandle<Core>,
}

impl Harness {
    /// Start a session with one workspace and its (process-less) root pane.
    async fn start(tag: &str) -> (Self, WorkspaceId, PaneId) {
        let ctx = fresh_ctx(tag);
        let (tx, rx) = mpsc::channel(64);
        let mut core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));

        // Through `absorb`, `workspace.create` mints state and spawns no
        // process (only the live path through `run()` does), so calling it
        // directly needs no Tokio runtime interaction, and leaves
        // `core.state()` readable to learn the root pane's id before `core`
        // is handed to `run()`.
        let (reply, answer) = oneshot::channel();
        core.absorb(CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        }));
        let created = answer
            .await
            .expect("core answered")
            .expect("create succeeds");
        let root_pane = core
            .state()
            .workspace(created.workspace)
            .expect("the workspace just created")
            .layout()
            .panes()[0];

        let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));
        (Self { ctx, tx, task }, created.workspace, root_pane)
    }

    /// Cancel the session and return the `Core` once its actor loop stops.
    async fn stop(self) -> Core {
        self.ctx.cancel.cancel();
        self.task.await.expect("core task did not panic")
    }
}

/// The pid of the process whose cwd is exactly `dir`, if one exists right
/// now.
///
/// Independent of anything `Core` or a pane's actor reports: this reads
/// `/proc` directly, the same ground truth T05's and T08's own tests read.
#[cfg(target_os = "linux")]
fn scan_proc_for_cwd(dir: &Path) -> Option<u32> {
    let dir = std::fs::canonicalize(dir).ok()?;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        // `/proc` also holds non-pid entries (`self`, `cpuinfo`, ...): skip
        // just this one rather than bailing out of the whole scan.
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let link = format!("/proc/{pid}/cwd");
        if std::fs::read_link(&link).ok().as_deref() == Some(dir.as_path()) {
            return Some(pid);
        }
    }
    None
}

/// The pid of the process whose cwd is exactly `dir`, if one exists right
/// now.
///
/// darwin mounts no `/proc`; libproc is the OS's own answer to the same
/// question, read here over this process's descendants — every pane these
/// tests spawn lives under the test process, `Core` being in-process. Still
/// independent of `Core`'s reports: nothing consulted here passed through
/// the server's bookkeeping.
#[cfg(target_os = "macos")]
fn scan_proc_for_cwd(dir: &Path) -> Option<u32> {
    use amx_core::platform::{ProcessId, ProcessTree as _};
    let dir = std::fs::canonicalize(dir).ok()?;
    let tree = amx_server::platform::UnixProcessTree;
    let mut queue = vec![ProcessId(std::process::id())];
    while let Some(next) = queue.pop() {
        if tree.cwd(next).is_ok_and(|cwd| cwd == dir) {
            return Some(next.0);
        }
        queue.extend(tree.children(next).unwrap_or_default());
    }
    None
}

/// Poll until a process with cwd `dir` shows up.
async fn find_process_in(dir: &Path) -> u32 {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(pid) = scan_proc_for_cwd(dir) {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "no process ever appeared with cwd {}",
            dir.display()
        );
        tokio::time::sleep(TICK).await;
    }
}

/// Poll until no process has cwd `dir` — i.e. whatever was there has exited.
async fn wait_for_no_process_in(dir: &Path) {
    let deadline = Instant::now() + PATIENCE;
    while scan_proc_for_cwd(dir).is_some() {
        assert!(
            Instant::now() < deadline,
            "a process never exited out of {}",
            dir.display()
        );
        tokio::time::sleep(TICK).await;
    }
}

// --------------------------------------------------------------- workspace

#[tokio::test]
async fn workspace_create_returns_a_uuid_and_a_stable_short_number() {
    let ctx = fresh_ctx("ws-create");
    let (tx, rx) = mpsc::channel(8);
    let core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));
    let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));

    let first = call(&tx, |reply| {
        CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        })
    })
    .await
    .expect("first create succeeds");
    let second = call(&tx, |reply| {
        CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        })
    })
    .await
    .expect("second create succeeds");

    assert_ne!(
        first.workspace, second.workspace,
        "each create mints a fresh uuid"
    );
    assert_eq!(first.short, ShortNumber::FIRST);
    assert_eq!(
        second.short,
        ShortNumber::new(first.short.get() + 1),
        "short numbers are stable/assigned, not re-derived from a count"
    );

    ctx.cancel.cancel();
    let _ = task.await;
}

#[tokio::test]
async fn workspace_kill_reports_which_panes_it_took_with_it() {
    let (harness, ws, root) = Harness::start("ws-kill").await;

    let split = split_from(&harness.tx, split_params(root, None, None)).await;

    let killed = call(&harness.tx, |reply| {
        CoreCommand::Workspace(WorkspaceCall::Kill {
            params: workspace::KillParams { workspace: ws },
            reply,
        })
    })
    .await
    .expect("kill succeeds");

    let mut panes = killed.panes;
    panes.sort();
    let mut expected = vec![root, split.pane];
    expected.sort();
    assert_eq!(
        panes, expected,
        "the reply names every pane the workspace held, not just the one that was split"
    );

    harness.stop().await;
}

// -------------------------------------------------------------------- pane

#[tokio::test]
async fn split_inherits_the_source_pane_foreground_process_cwd() {
    let (harness, _ws, root) = Harness::start("split-inherit").await;
    let start = TempDir::new("split-inherit-start");
    let target = TempDir::new("split-inherit-target");

    // The source pane starts in `start`, then its own foreground process
    // moves to `target` — proving inheritance has to read the *live*
    // foreground process rather than copy the source's own recorded cwd.
    let cd_then_wait = format!("cd '{}' && exec sleep 60", target.path().display());
    let source = split_from(
        &harness.tx,
        split_params(root, Some(start.path()), Some(&["sh", "-c", &cd_then_wait])),
    )
    .await;

    // A split with no explicit `cwd` should land its own (default-shell)
    // process in `target` once the source has settled there. `cd && exec`
    // is not atomic from this test's point of view, so retry rather than
    // race it.
    let deadline = Instant::now() + PATIENCE;
    loop {
        split_from(&harness.tx, split_params(source.pane, None, None)).await;
        if scan_proc_for_cwd(target.path()).is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a split from the source pane never inherited its foreground cwd"
        );
        // Every look re-splits, so pace retries slower than a tick.
        tokio::time::sleep(RETRY).await; // deliberate
    }

    harness.stop().await;
}

#[tokio::test]
async fn split_falls_back_to_the_pane_cwd_when_the_foreground_cwd_is_unreadable() {
    let (harness, _ws, root) = Harness::start("split-fallback").await;
    let recorded = TempDir::new("split-fallback-recorded");

    // A source pane whose process exits almost immediately: once it is gone
    // there is no foreground process left to read a cwd from at all.
    let source = split_from(
        &harness.tx,
        split_params(root, Some(recorded.path()), Some(&["true"])),
    )
    .await;
    wait_for_no_process_in(recorded.path()).await;

    // A split from that (now process-less) pane still succeeds — the pane's
    // own entry in the layout outlives its process — and its default-shell
    // child lands in the source's own recorded cwd, not the daemon's cwd.
    let deadline = Instant::now() + PATIENCE;
    loop {
        split_from(&harness.tx, split_params(source.pane, None, None)).await;
        if scan_proc_for_cwd(recorded.path()).is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a split from a pane with no live process never fell back to its recorded cwd"
        );
        // Every look re-splits, so pace retries slower than a tick.
        tokio::time::sleep(RETRY).await; // deliberate
    }

    harness.stop().await;
}

#[tokio::test]
async fn swap_does_not_restart_either_process() {
    let (harness, _ws, root) = Harness::start("swap").await;
    let dir_a = TempDir::new("swap-a");
    let dir_b = TempDir::new("swap-b");

    let a = split_from(
        &harness.tx,
        split_params(root, Some(dir_a.path()), Some(&["sleep", "60"])),
    )
    .await;
    let b = split_from(
        &harness.tx,
        split_params(root, Some(dir_b.path()), Some(&["sleep", "60"])),
    )
    .await;

    let pid_a_before = find_process_in(dir_a.path()).await;
    let pid_b_before = find_process_in(dir_b.path()).await;

    call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Swap {
            params: pane::SwapParams {
                pane: a.pane,
                with: b.pane,
            },
            reply,
        })
    })
    .await
    .expect("swap succeeds");

    assert_eq!(
        find_process_in(dir_a.path()).await,
        pid_a_before,
        "the pane that started in dir_a must still be that same process"
    );
    assert_eq!(
        find_process_in(dir_b.path()).await,
        pid_b_before,
        "the pane that started in dir_b must still be that same process"
    );

    harness.stop().await;
}

#[tokio::test]
async fn move_pane_between_workspaces_does_not_restart_the_process() {
    let (harness, _ws_a, root_a) = Harness::start("move").await;
    let created_b = call(&harness.tx, |reply| {
        CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        })
    })
    .await
    .expect("second workspace");

    let dir = TempDir::new("move-dir");
    let moved = split_from(
        &harness.tx,
        split_params(root_a, Some(dir.path()), Some(&["sleep", "60"])),
    )
    .await;

    let pid_before = find_process_in(dir.path()).await;

    call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Move {
            params: pane::MoveParams {
                pane: moved.pane,
                to: created_b.workspace,
            },
            reply,
        })
    })
    .await
    .expect("move succeeds");

    assert_eq!(
        find_process_in(dir.path()).await,
        pid_before,
        "moving a pane into another workspace must not restart its process"
    );

    harness.stop().await;
}

#[tokio::test]
async fn focus_moves_to_the_geometric_neighbour_and_reports_the_landing_pane() {
    let (harness, ws, root) = Harness::start("focus").await;
    let _split = split_from(
        &harness.tx,
        split_params(root, None, Some(&["sleep", "60"])),
    )
    .await;

    // The split landed focus on the new pane, whose slot is on the right;
    // moving left lands back on the root.
    let landed = call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Focus {
            params: pane::FocusParams {
                workspace: ws,
                direction: pane::MoveDirection::Left,
            },
            reply,
        })
    })
    .await
    .expect("focus succeeds");
    assert_eq!(landed.pane, Some(root), "the reply names the landing pane");

    // Already at the left edge: a legal no-op that stays put.
    let stayed = call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Focus {
            params: pane::FocusParams {
                workspace: ws,
                direction: pane::MoveDirection::Left,
            },
            reply,
        })
    })
    .await
    .expect("focus at the edge is a no-op, not an error");
    assert_eq!(stayed.pane, Some(root));

    let core = harness.stop().await;
    assert_eq!(
        core.state().workspace(ws).expect("workspace").focus(),
        Some(root),
        "the server's canonical focus actually moved"
    );
}

#[tokio::test]
async fn resize_nudges_the_containing_split_and_a_lone_pane_is_a_clean_noop() {
    let (harness, ws, root) = Harness::start("resize").await;

    // A lone pane has no split to nudge: reported as such, not an error.
    let noop = call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Resize {
            params: pane::ResizeParams {
                pane: root,
                direction: pane::MoveDirection::Right,
                delta: 0.25,
            },
            reply,
        })
    })
    .await
    .expect("resizing a lone pane succeeds");
    assert!(!noop.resized, "a lone pane reports resized: false");

    let split = split_from(
        &harness.tx,
        split_params(root, None, Some(&["sleep", "60"])),
    )
    .await;

    let grown = call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Resize {
            params: pane::ResizeParams {
                pane: root,
                direction: pane::MoveDirection::Right,
                delta: 0.25,
            },
            reply,
        })
    })
    .await
    .expect("resize succeeds");
    assert!(grown.resized);

    // A delta that is not a plain magnitude is refused before it reaches the
    // layout.
    let refused = call(&harness.tx, |reply| {
        CoreCommand::Pane(PaneCall::Resize {
            params: pane::ResizeParams {
                pane: root,
                direction: pane::MoveDirection::Right,
                delta: -1.0,
            },
            reply,
        })
    })
    .await
    .expect_err("a negative delta is invalid params");
    assert_eq!(refused.code, RpcError::INVALID_PARAMS);

    let core = harness.stop().await;
    let ws_state = core.state().workspace(ws).expect("workspace");
    let rects = ws_state.layout().rects(ws_state.area());
    let width = |pane: PaneId| {
        rects
            .iter()
            .find(|(id, _)| *id == pane)
            .expect("pane has a rect")
            .1
            .w
    };
    assert!(
        width(root) > width(split.pane),
        "growing right made the root's slot the wider one ({} vs {})",
        width(root),
        width(split.pane)
    );
}

#[tokio::test]
async fn foreground_cwd_is_part_of_pane_api_state() {
    let (harness, _ws, root) = Harness::start("pane-state").await;
    let dir = TempDir::new("pane-state-dir");

    let created = split_from(
        &harness.tx,
        split_params(root, Some(dir.path()), Some(&["sleep", "1"])),
    )
    .await;

    let core = harness.stop().await;
    let recorded = core
        .state()
        .pane(created.pane)
        .expect("the pane just created")
        .cwd();
    assert_eq!(
        recorded,
        Some(dir.path()),
        "a pane's cwd — foreground_cwd at the moment it was started — is part of its session-state record"
    );
}
