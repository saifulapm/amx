//! First-run behavior: every workspace starts with a live shell, and the
//! first attached client of a fresh session finds a workspace waiting.
//!
//! Two rules are pinned here. `workspace.create`, served through the real
//! actor loop, spawns a shell behind the fresh workspace's root pane — a
//! workspace whose root pane has no process behind it is a dead rectangle no
//! keystroke can reach. And a hello that declares an attach seeds a session
//! with no workspaces with exactly one, before the welcome is written, so a
//! bare `amx` never lands its user in an empty session; two racing first
//! attaches still seed one workspace, not two, because the `Core` serializes
//! its mailbox.
//!
//! Liveness is asserted the way `tests/verbs.rs` asserts it: against the
//! process and the actor path, not against a reply. The shell behind the root
//! pane writes its prompt, the pane actor reads it and reports damage, and
//! the damage reaches the bus — that chain only completes over a live pty
//! with a live child on the far end. The pid claim rides
//! `PaneCommand::ForegroundCwd`, which answers `Some` only by reading the
//! foreground process group out of `/proc`.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use amx_core::{
    Bus, Ctx, Delivery, Event, PaneId, Scheduled, SessionName, Subscription, WorkspaceId,
};
use amx_proto::control::workspace;
use amx_proto::version::window;
use amx_server::actor::core::Core;
use amx_server::actor::{CoreCommand, CoreHandle, PaneCommand, SessionCall, WorkspaceCall};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

mod support;

use support::{PATIENCE, Server, result_of};

/// A `Ctx` with fabricated, never-touched-on-disk paths — nothing at this
/// level binds a socket, so there is no filesystem state to collide over.
fn fresh_ctx(tag: &str) -> Ctx {
    let root = PathBuf::from("/amx-test-firstrun").join(tag);
    Ctx {
        session: SessionName::new("test").expect("valid session name"),
        runtime_dir: root.join("runtime"),
        socket: root.join("runtime/sock"),
        state_dir: root.join("state"),
        bus: Arc::new(Bus::new(64)),
        cancel: CancellationToken::new(),
    }
}

/// A `Core` on the real actor loop, plus the bus subscription watching it.
struct Harness {
    ctx: Ctx,
    tx: mpsc::Sender<CoreCommand>,
    events: Subscription,
    task: JoinHandle<Core>,
}

impl Harness {
    fn start(tag: &str) -> Self {
        let ctx = fresh_ctx(tag);
        let events = ctx.bus.subscribe();
        let (tx, rx) = mpsc::channel(64);
        let core = Core::new(ctx.clone(), CoreHandle::new(tx.clone()));
        let task = tokio::spawn(core.run(rx, |_: &Scheduled| {}));
        Self {
            ctx,
            tx,
            events,
            task,
        }
    }

    /// Wait for the first bus event `want` accepts.
    async fn wait_for_event(&mut self, want: impl FnMut(&Event) -> bool) -> Event {
        wait_for_event(&mut self.events, want).await
    }

    /// End the actor loop with `Shutdown` — not a cancel, so the panes stay
    /// live — and hand the `Core` back for inspection.
    async fn into_core(self) -> (Core, Ctx) {
        self.tx
            .send(CoreCommand::Shutdown)
            .await
            .expect("core mailbox is open");
        let core = self.task.await.expect("core task did not panic");
        (core, self.ctx)
    }
}

/// Ask `core` for `pane`'s foreground process cwd, through the pane's own
/// live actor.
async fn foreground_cwd_of(core: &Core, pane: PaneId) -> Option<PathBuf> {
    let handle = core
        .pane_handle(pane)
        .expect("the pane is backed by a live actor");
    let (reply, answer) = oneshot::channel();
    handle
        .send(PaneCommand::ForegroundCwd(reply))
        .await
        .expect("the pane actor is running");
    tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("an answer before the deadline")
        .expect("the pane actor answered")
}

/// The ids of every workspace in `core`'s state, in order.
fn workspaces_of(core: &Core) -> Vec<WorkspaceId> {
    core.state().workspaces().map(|ws| ws.id()).collect()
}

// ---------------------------------------------------------- workspace.create

#[tokio::test]
async fn workspace_create_spawns_a_live_root_pane() {
    let mut harness = Harness::start("create-live");

    let (reply, answer) = oneshot::channel();
    harness
        .tx
        .send(CoreCommand::Workspace(WorkspaceCall::Create {
            params: workspace::CreateParams::default(),
            reply,
        }))
        .await
        .expect("core mailbox is open");
    let created = answer
        .await
        .expect("core answered")
        .expect("create succeeds");

    // The shell behind the root pane wrote its prompt: pty → pane actor →
    // bus. This is the read chain, and only a live process completes it.
    let damaged = harness
        .wait_for_event(|event| matches!(event, Event::PaneDamage { .. }))
        .await;
    let Event::PaneDamage { pane, .. } = damaged else {
        unreachable!("wait_for_event only accepts damage");
    };

    let (core, ctx) = harness.into_core().await;
    let root = core
        .state()
        .workspace(created.workspace)
        .expect("the workspace just created")
        .layout()
        .panes()[0];
    assert_eq!(pane, root, "the damage came from the workspace's root pane");
    assert!(
        foreground_cwd_of(&core, root).await.is_some(),
        "the root pane's foreground process group has a live pid in /proc"
    );

    ctx.cancel.cancel();
}

// ------------------------------------------------------------------ seeding

#[tokio::test]
async fn two_racing_first_attaches_seed_exactly_one_workspace() {
    let mut harness = Harness::start("race");

    // Both attaches are queued before either is served: the order the `Core`
    // drains them in is the only thing between "one workspace" and "two".
    let (first, first_done) = oneshot::channel();
    let (second, second_done) = oneshot::channel();
    harness
        .tx
        .send(CoreCommand::Session(SessionCall::Attached { reply: first }))
        .await
        .expect("core mailbox is open");
    harness
        .tx
        .send(CoreCommand::Session(SessionCall::Attached {
            reply: second,
        }))
        .await
        .expect("core mailbox is open");
    first_done
        .await
        .expect("core answered")
        .expect("the first attach seeds");
    second_done
        .await
        .expect("core answered")
        .expect("the second attach is a no-op, not a failure");

    harness
        .wait_for_event(|event| matches!(event, Event::PaneDamage { .. }))
        .await;

    let (core, ctx) = harness.into_core().await;
    let workspaces = workspaces_of(&core);
    assert_eq!(
        workspaces.len(),
        1,
        "two racing first attaches must seed one workspace, not two"
    );
    let seeded = workspaces[0];
    assert_eq!(
        core.state().active_workspace(),
        Some(seeded),
        "the seeded workspace is the one the session is switched to"
    );
    let root = core
        .state()
        .workspace(seeded)
        .expect("the seeded workspace")
        .layout()
        .panes()[0];
    assert!(
        foreground_cwd_of(&core, root).await.is_some(),
        "the seeded root pane has a live shell behind it"
    );

    ctx.cancel.cancel();
}

#[tokio::test]
async fn first_attach_to_a_fresh_session_seeds_one_workspace_with_a_shell() {
    let server = Server::start("firstrun-seed").await;
    let mut events = server.ctx.bus.subscribe();

    // The full path: a real socket, a hello that declares an attach, and the
    // seeding finished before the welcome comes back.
    let mut client = server.connect().await;
    client.hello_as_attach(window()).await;
    let seeded = wait_for_event(&mut events, |event| {
        matches!(event, Event::WorkspaceCreated { .. })
    })
    .await;
    let Event::WorkspaceCreated { workspace } = seeded else {
        unreachable!("wait_for_event only accepts creations");
    };

    // A live shell, not just a state entry: its prompt reaches the bus.
    wait_for_event(&mut events, |event| {
        matches!(event, Event::PaneDamage { .. })
    })
    .await;

    // A second attach finds the workspace and seeds nothing. Its welcome is
    // written only after the `Core` served its attach, so by the time `ping`
    // answers, any second seed would already be on the bus before `seq`.
    let mut second = server.connect().await;
    second.hello_as_attach(window()).await;
    let reply = second.request(1, "ping", json!({})).await;
    let head = result_of(&reply)["seq"].as_u64().expect("ping carries seq");

    let mut creations = 0;
    let mut switches_to_seeded = 0;
    while let Ok(Some(delivery)) = tokio::time::timeout(PATIENCE, events.recv()).await {
        let envelope = match delivery {
            Delivery::Event(envelope) => envelope,
            Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
        };
        match envelope.event {
            Event::WorkspaceCreated { .. } => creations += 1,
            Event::FocusChanged { workspace: ws, .. } if ws == workspace => {
                switches_to_seeded += 1;
            }
            _ => {}
        }
        if envelope.seq >= head {
            break;
        }
    }
    assert_eq!(
        creations, 0,
        "the second attach must not seed a second workspace"
    );
    assert!(
        switches_to_seeded <= 1,
        "seeding switches to the seeded workspace exactly once"
    );

    server.shutdown().await;
}

/// Wait for the first bus event `want` accepts, on a bare subscription.
async fn wait_for_event(events: &mut Subscription, mut want: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let delivery = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an event before the deadline")
            .expect("the bus is open");
        match delivery {
            Delivery::Event(envelope) if want(&envelope.event) => return envelope.event,
            Delivery::Event(_) => {}
            Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
        }
    }
}

/// The seeding must not fire for a verb connection: `attach: false` (or an
/// older client with no field at all) leaves an empty session empty.
#[tokio::test]
async fn a_verb_connection_never_seeds_a_workspace() {
    let server = Server::start("firstrun-verb").await;
    let mut events = server.ctx.bus.subscribe();

    let mut client = server.attach().await;
    let reply = client.request(1, "ping", json!({})).await;
    let head = result_of(&reply)["seq"].as_u64().expect("ping carries seq");

    // Every event up to the ping's seq is already on the bus; none of them
    // may be a workspace creation.
    let mut sub_head = 0;
    while sub_head < head {
        let delivery = tokio::time::timeout(Duration::from_millis(500), events.recv())
            .await
            .expect("the published events are already buffered")
            .expect("the bus is open");
        match delivery {
            Delivery::Event(envelope) => {
                assert!(
                    !matches!(envelope.event, Event::WorkspaceCreated { .. }),
                    "a verb connection seeded a workspace"
                );
                sub_head = envelope.seq;
            }
            Delivery::Gap { from, to } => panic!("the test fell behind the bus: {from}..={to}"),
        }
    }

    server.shutdown().await;
}
