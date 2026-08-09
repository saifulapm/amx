//! The one-round-trip claim, proved by taking the round trips away.
//!
//! A child module of [`super`] rather than a fourth section of it, because it
//! shares nothing with the rest of the suite. Everything there drives the real
//! socket path through a real `Core` with real panes under it; this drives a
//! `Core` with no Tokio runtime at all, by direct [`Core::absorb`] calls — the
//! shape `tests/runtime.rs` established for the two commands that have a
//! `_no_spawn` fallback. The split also keeps the parent under the module
//! budget on the day it was written, which R-M1-3 says not to postpone.

use amx_core::agent::{AgentSnapshot, AgentState, StatusCause};
use amx_core::{Bus, Ctx, SessionName, WorkspaceId};
use amx_proto::control::{agent, workspace};
use amx_server::actor::core::Core;
use amx_server::actor::{AgentCall, AgentQueryCall, CoreCommand, CoreHandle, WorkspaceCall};
use tokio::sync::{mpsc, oneshot};

use crate::support::TempDir;

/// D-M4-2, as a structural fact: the whole reply comes out of `Core`'s own
/// state.
///
/// This `Core` has no Tokio runtime under it, so it has spawned no pane actors
/// and there is no hub — and `Core::absorb` cannot await, so a handler that
/// asked a pane or the hub for anything could not be written there at all. What
/// the reply carries is therefore exactly what one mailbox round trip can
/// carry, at any pane count. The per-pane `StreamCall::Wiring` fan-out D-M4-2
/// refuses would be 25 round trips at the 25 agents this surface exists for,
/// and X00's baseline priced that shape at 161 ms
/// (`docs/notes/m4-live-smoke.md` §1.4).
///
/// A wall-clock bound would have been the other way to say this, and it is the
/// shape X04 spent a task taking out of four tests: the same call on this
/// machine read 12 ms and 35 ms an hour apart, and a threshold between them
/// measures the runner rather than the handler.
///
/// `last_line` is empty for the row, and honestly so: no process was started,
/// so no pane has a screen.
#[test]
fn the_whole_reply_is_answered_by_a_core_with_no_runtime_under_it() {
    let dir = TempDir::new("aglist-absorb");
    let ctx = ctx_without_a_runtime(dir.path());
    let mut core = Core::new(ctx, unused_handle());

    let ws = absorb_create(&mut core, "api");
    let pane = *core
        .state()
        .workspace(ws)
        .expect("the workspace was minted")
        .layout()
        .panes()
        .first()
        .expect("a workspace is minted with a root pane");

    // The hub's mirror, arriving the one way it ever arrives: as a command in
    // this actor's own mailbox.
    core.absorb(CoreCommand::Agent(AgentCall::Status {
        pane,
        status: Some(Box::new(AgentSnapshot {
            kind: None,
            state: AgentState::Blocked,
            cause: StatusCause::Screen,
            transition_seq: 7,
            attention: Some(0),
            session_ref: None,
            reason: Some("permission_dialog".to_owned()),
            since: Some(SINCE),
        })),
        attention: vec![pane],
    }));

    let (reply, mut answer) = oneshot::channel();
    core.absorb(CoreCommand::AgentQuery(AgentQueryCall::List {
        params: agent::ListParams::default(),
        reply,
    }));
    let listed = answer
        .try_recv()
        .expect("absorb answers synchronously, on this thread, before it returns")
        .expect("an unscoped list refuses nothing");

    assert_eq!(listed.agents.len(), 1, "{:?}", listed.agents);
    let row = &listed.agents[0];
    assert_eq!(row.pane, pane);
    assert_eq!(row.workspace.id, ws);
    assert_eq!(row.workspace.name.as_deref(), Some("api"));
    assert_eq!(row.status, AgentState::Blocked);
    assert_eq!(row.reason.as_deref(), Some("permission_dialog"));
    assert_eq!(row.since, Some(SINCE));
    assert_eq!(
        row.last_line, "",
        "a pane with no process behind it has no screen, and says so"
    );
    assert_eq!(listed.attention, vec![pane]);
    assert!(
        listed.now >= FLOOR,
        "the reply carries the server's own wall clock: {}",
        listed.now
    );
}

/// The goldens' own instant, so a reader meeting it twice meets one value.
const SINCE: u64 = 1_754_650_000_000;

/// A wall clock later than this is a wall clock, and not a sequence number or a
/// count of seconds: 2023-11-14, comfortably behind anything that can run this.
const FLOOR: u64 = 1_700_000_000_000;

/// Mint a labelled workspace on a `Core` with no runtime, and answer with it.
fn absorb_create(core: &mut Core, label: &str) -> WorkspaceId {
    let (reply, mut answer) = oneshot::channel();
    core.absorb(CoreCommand::Workspace(WorkspaceCall::Create {
        params: workspace::CreateParams {
            label: Some(label.to_owned()),
            focus: true,
            worktree: None,
        },
        reply,
    }));
    answer
        .try_recv()
        .expect("absorb answers before it returns")
        .expect("a create with no worktree is accepted")
        .workspace
}

/// A `CoreHandle` nothing will ever receive on: this `Core` is driven by
/// direct [`Core::absorb`] calls and never runs its mailbox loop.
fn unused_handle() -> CoreHandle {
    let (tx, rx) = mpsc::channel(1);
    std::mem::forget(rx);
    CoreHandle::new(tx)
}

/// A `Ctx` for a `Core` that is never spawned.
fn ctx_without_a_runtime(root: &std::path::Path) -> Ctx {
    let mut ctx = crate::support::ctx_under(root);
    ctx.bus = std::sync::Arc::new(Bus::new(64));
    ctx.session = SessionName::new("aglist").expect("a session name");
    ctx
}
