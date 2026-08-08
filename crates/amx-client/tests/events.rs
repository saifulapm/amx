//! V14 acceptance: the client hears the session's events instead of polling
//! for them.
//!
//! Everything here that can be driven over the wire is driven over the wire.
//! `support::Server` is a real `Core` and a real `Gateway` on a real socket, so
//! `events.subscribe`, the server's delivery pump, the framing and the client's
//! notification queue are all the shipped ones; the tests publish onto the
//! session's own bus and then read what the client does about it. The bus is
//! the honest injection point — it is where `AgentHub` publishes from, and the
//! hub itself is not assembled by this harness.
//!
//! What cannot go over the wire says why in place: a delivery carrying an event
//! tag no build knows cannot be published through a typed bus, and neither can
//! a notification naming a method that does not exist.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::time::Duration;

use amx_client::app::{App, Folded};
use amx_client::input::{InputEvent, PREFIX};
use amx_client::model::WorkspaceModel;
use amx_core::agent::{AgentState, StatusCause};
use amx_core::{Delivery, Direction, Envelope, Event, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::{Call, Method};
use amx_proto::rpc::Notification;
use serde_json::json;

/// How long a test waits for the client to fold something before calling it a
/// failure. Generous, and never on the green path: the client wakes on a frame,
/// so a passing run spends microseconds here.
const DEADLINE: Duration = Duration::from_secs(10);

type TestApp = App<std::fs::File, Vec<u8>>;

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx-events-test".to_owned(),
        version: "0.0.0".to_owned(),
        term: None,
    }
}

/// One attention event, with the identity block D15 added and X06 fills.
///
/// A helper rather than a literal at each publish: what this suite asserts is
/// what the client's *model* does with a queue change, and the names ride the
/// event for a notifier's benefit rather than this client's.
fn enqueued(pane: PaneId) -> Event {
    Event::AttentionEnqueued {
        pane,
        workspace: None,
        name: None,
        reason: None,
        since: None,
    }
}

/// The other half of [`enqueued`].
fn dequeued(pane: PaneId) -> Event {
    Event::AttentionDequeued {
        pane,
        workspace: None,
        name: None,
        reason: None,
        since: None,
    }
}

/// Attach to a real session, subscribed, with the seeded workspace folded.
///
/// The pty master comes back with the app: the slave moved into the terminal
/// guard is unusable the moment nothing holds the master open, so the caller
/// has to keep it alive for the length of the test.
async fn attached(tag: &'static str) -> (support::Server, TestApp, std::fs::File) {
    let server = support::Server::start(tag).await;
    let pty = support::open_pty();
    let app = App::attach(server.socket(), pty.slave, Vec::new(), client_info())
        .await
        .expect("attach to the real server over the real socket");
    (server, app, pty.master)
}

/// Step the wire until `done` holds, failing on the deadline.
///
/// A condition plus a deadline whose expiry is the failure — never a nap. Each
/// step blocks on the socket, so this makes no progress the server did not.
async fn pump_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) {
    let stepped = tokio::time::timeout(DEADLINE, async {
        while !done(app) {
            app.step_frame().await.expect("read one server frame");
        }
    })
    .await;
    assert!(
        stepped.is_ok(),
        "the client never folded what the session published"
    );
}

/// The pane the seeded workspace put the client's focus on.
fn focused(app: &mut TestApp) -> PaneId {
    app.focused_pane()
        .expect("the attach folded the seeded workspace's pane")
}

/// One delivery, shaped exactly as the server's pump encodes it.
fn delivery(delivery: &Delivery) -> Notification {
    Notification::new(
        "event",
        Some(serde_json::to_value(delivery).expect("encode a delivery")),
    )
}

/// One event as a delivery notification.
fn published(seq: u64, event: Event) -> Notification {
    delivery(&Delivery::Event(Envelope { seq, event }))
}

#[tokio::test]
async fn client_subscribes_and_folds_status_events_without_polling() {
    let (server, mut app, _master) = attached("evst").await;
    let pane = focused(&mut app);
    let polls = app.resyncs;

    server.ctx.bus.publish(Event::AgentStatus {
        pane,
        from: Some(AgentState::Working),
        to: AgentState::Blocked,
        cause: StatusCause::Hook,
    });

    pump_until(&mut app, |app| {
        app.model()
            .pane_agent(pane)
            .is_some_and(|agent| agent.state == AgentState::Blocked)
    })
    .await;

    let agent = app.model().pane_agent(pane).expect("the folded status");
    assert_eq!(agent.cause, StatusCause::Hook);
    assert!(
        agent.transition_seq > 0,
        "the status carries the sequence of the transition that produced it"
    );
    // The whole point of the subscription: the status arrived without the
    // client asking for it. One `session.state` happened, at attach, and no
    // other.
    assert_eq!(
        app.resyncs, polls,
        "the client re-read session.state instead of listening"
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn gap_notification_triggers_one_state_resync() {
    let (server, mut app, _master) = attached("evgp").await;
    let pane = focused(&mut app);

    // Advance the bus well past its 64-deep replay buffer, so a subscription
    // resuming from sequence 1 cannot be served out of it. 04 §2: a subscriber
    // the buffer has lapped is told so explicitly, in `gap{from,to}` — and
    // `events.subscribe`'s contract gives an `after_seq` already dropped the
    // same answer rather than an error.
    for _ in 0..256 {
        server.ctx.bus.publish(Event::AgentStatus {
            pane,
            from: None,
            to: AgentState::Working,
            cause: StatusCause::Screen,
        });
    }

    let polls = app.resyncs;
    app.subscribe_events(Some(1))
        .await
        .expect("re-subscribe from a sequence the replay buffer has dropped");

    pump_until(&mut app, |app| app.resyncs > polls).await;
    assert_eq!(
        app.resyncs,
        polls + 1,
        "a gap is re-read state, exactly once — the events are gone, the state \
         they described is not"
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn enqueue_notification_emits_osc_9_into_the_host_terminal() {
    let (server, mut app, _master) = attached("evos").await;
    let pane = focused(&mut app);
    app.model().set_pane_label(pane, Some("dev".to_owned()));

    server.ctx.bus.publish(enqueued(pane));
    pump_until(&mut app, |app| !app.emitted().is_empty()).await;

    let emitted = String::from_utf8(app.emitted().to_vec()).expect("the escape is valid utf-8");
    assert!(
        emitted.starts_with("\u{1b}]9;") && emitted.ends_with('\u{7}'),
        "an enqueue writes one BEL-terminated OSC 9 to the host terminal: \
         {emitted:?}"
    );
    assert!(
        emitted.contains("dev"),
        "the notification names the agent by its label: {emitted:?}"
    );
    assert_eq!(app.model().attention(), [pane]);

    // One notification per *enqueue*: a re-block notifies again, and a
    // duplicate enqueue — which is what the overlap after a resync produces —
    // notifies not at all.
    let written = app.emitted().len();
    server.ctx.bus.publish(dequeued(pane));
    server.ctx.bus.publish(enqueued(pane));
    server.ctx.bus.publish(enqueued(pane));
    // A marker behind the three: deliveries are ordered, so folding this one
    // proves the three before it were folded too.
    server.ctx.bus.publish(Event::AgentStatus {
        pane,
        from: None,
        to: AgentState::Idle,
        cause: StatusCause::Screen,
    });
    pump_until(&mut app, |app| {
        app.model()
            .pane_agent(pane)
            .is_some_and(|agent| agent.state == AgentState::Idle)
    })
    .await;
    assert_eq!(
        app.emitted().len(),
        written * 2,
        "the re-block notified once and the duplicate enqueue not at all"
    );
    assert_eq!(app.model().attention(), [pane]);

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn next_attention_key_calls_agent_next_and_focus_follows() {
    let (server, mut app, _master) = attached("evnx").await;

    // A two-pane mirror, so "focus moved" is a change this client can actually
    // represent: a focus event naming a pane outside the layout is a mirror
    // that has fallen behind, not a focus move.
    let workspace = WorkspaceId::new_v4();
    let here = PaneId::new_v4();
    let there = PaneId::new_v4();
    let mut layout = BspLayout::with_root(here);
    layout
        .split(here, Direction::Right, there, 0.5)
        .expect("split the mirrored layout");
    app.adopt_workspace(
        workspace,
        WorkspaceModel {
            label: Some("agents".to_owned()),
            layout,
        },
    );
    assert_eq!(app.focused_pane(), Some(here));

    // Prefix + `a`: one key for "handle the next one" (03).
    let mut calls = Vec::new();
    app.handle_input(&[PREFIX, b'a'], &mut |event| {
        if let InputEvent::Call(call) = event {
            calls.push(call);
        }
    });
    assert_eq!(calls.len(), 1, "one key, one call: {calls:?}");
    assert_eq!(calls[0].method(), Method::AgentNext);
    assert!(matches!(calls[0], Call::AgentNext(_)));

    // The key asks; the server moves focus and says so with `FocusChanged`,
    // which is the path a focus change from any other client arrives by too.
    let folded = app.apply_notification(&published(
        1,
        Event::FocusChanged {
            workspace,
            pane: Some(there),
        },
    ));
    assert_eq!(folded, Folded::Applied);
    assert_eq!(
        app.focused_pane(),
        Some(there),
        "focus follows the server's own event"
    );

    // A focus change *in another workspace* is recorded and not followed: the
    // identical event is what a second client's `workspace.switch` publishes,
    // and this terminal is not that client's to move (`tests/adversarial.rs`
    // pins it). Crossing workspaces is `agent.next`'s own reply's job.
    let elsewhere = WorkspaceId::new_v4();
    let folded = app.apply_notification(&published(
        2,
        Event::FocusChanged {
            workspace: elsewhere,
            pane: Some(PaneId::new_v4()),
        },
    ));
    assert_eq!(folded, Folded::Nothing);
    assert_eq!(
        app.model().focused_workspace_id(),
        Some(workspace),
        "this client is still showing what it was showing"
    );

    drop(app);
    server.shutdown().await;
}

#[tokio::test]
async fn a_pane_another_connection_created_reaches_this_screen() {
    let (server, mut app, _master) = attached("evsp").await;
    let pane = focused(&mut app);
    assert_eq!(panes_shown(&mut app), vec![pane]);

    // A second connection to the same session, splitting the pane this client
    // is showing. This is `amx agent start` run from another terminal: it mints
    // its pane through `pane.split`, and nothing about it passes through the
    // attached client — which is why the client can only learn of it from the
    // event.
    let other_pty = support::open_pty();
    let mut other = App::attach(server.socket(), other_pty.slave, Vec::new(), client_info())
        .await
        .expect("a second client on the same session");
    other
        .handle_bytes(&[PREFIX, b'v'])
        .await
        .expect("split from the other connection");
    let split = other
        .model()
        .focused_workspace()
        .expect("the other client's mirror")
        .layout
        .panes();
    assert_eq!(split.len(), 2, "the split happened: {split:?}");

    pump_until(&mut app, |app| panes_shown(app).len() == 2).await;
    assert_eq!(
        panes_shown(&mut app),
        split,
        "this client shows the same two panes the session holds"
    );

    drop(other);
    drop(app);
    server.shutdown().await;
}

/// The panes this client would paint, in layout order.
fn panes_shown(app: &mut TestApp) -> Vec<PaneId> {
    app.model()
        .focused_workspace()
        .map(|ws| ws.layout.panes())
        .unwrap_or_default()
}

#[tokio::test]
async fn unknown_event_tags_are_ignored_not_fatal() {
    let (server, mut app, _master) = attached("evsk").await;
    let pane = focused(&mut app);

    // An event tag from a server newer than this build. `Event` is
    // `#[non_exhaustive]` exactly so this can happen, and the contract is that
    // it costs a skipped line rather than a dead client.
    let future = Notification::new(
        "event",
        Some(json!({
            "delivery": "event",
            "seq": 7,
            "event": { "event": "agent_reticulated", "pane": pane },
        })),
    );
    assert_eq!(app.apply_notification(&future), Folded::Nothing);

    // So does a notification naming a method this build has never heard of.
    let stranger = Notification::new("session.telepathy", Some(json!({ "pane": pane })));
    assert_eq!(app.apply_notification(&stranger), Folded::Nothing);

    // And a delivery kind it cannot name.
    let unknown_delivery = Notification::new("event", Some(json!({ "delivery": "smoke_signal" })));
    assert_eq!(app.apply_notification(&unknown_delivery), Folded::Nothing);

    // A known event this client has no model for is ignored just as quietly,
    // and is not a gap: `session.state` after a layout call still covers it.
    assert_eq!(
        app.apply_notification(&published(
            8,
            Event::WorkspaceCreated {
                workspace: WorkspaceId::new_v4()
            }
        )),
        Folded::Nothing,
    );

    // The stream survives all four: the next event this build *does* know still
    // lands, over the real wire.
    server.ctx.bus.publish(Event::AgentStatus {
        pane,
        from: None,
        to: AgentState::Idle,
        cause: StatusCause::Screen,
    });
    pump_until(&mut app, |app| app.model().pane_agent(pane).is_some()).await;
    assert_eq!(
        app.model().pane_agent(pane).map(|agent| agent.state),
        Some(AgentState::Idle),
    );

    drop(app);
    server.shutdown().await;
}
