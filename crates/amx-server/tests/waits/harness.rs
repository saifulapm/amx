//! A session whose bus, status view and socket the test holds all three of.
//!
//! Assembled here rather than through `support::Server` for three reasons the
//! suite needs and that harness cannot give:
//!
//! - the **replay capacity is a parameter**. A gap is what happens when a
//!   subscriber falls behind a bounded buffer, so a suite that has to produce
//!   one on purpose sizes the buffer rather than publishing a million events.
//! - the **[`StatusView`] is reachable**. `AgentHub` is V08's and lands in the
//!   same wave; until it does, the view the gateway hands every connection is
//!   written here instead, through the same [`StatusView::commit`] the hub will
//!   use — including its write-before-publish ordering, which is the property
//!   half these tests are about.
//! - the **shell is pinned to `/bin/sh`**. A session on the defaults spawns the
//!   developer's own login shell behind every seeded pane, which is slow, rude,
//!   and (for a zsh interrupted during startup) leaves a stale lock on their
//!   real history file. The same reasoning `tests/drive.rs` records.

#![allow(dead_code, reason = "each test uses a subset of the harness")]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use amx_core::agent::{AgentSnapshot, AgentState, StatusCause};
use amx_core::{Bus, Config, Ctx, Delivery, Event, PaneId, Scheduled, Seq};
use amx_proto::rpc::{Notification, RequestId};
use amx_proto::{FrameHeader, Request, Response, RpcOutcome};
use amx_server::actor::CoreHandle;
use amx_server::actor::core::Core;
use amx_server::actor::gateway::Gateway;
use amx_server::actor::{StatusUpdate, StatusView};
use amx_server::runtime::Runtime;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, watch};

use crate::support::{PATIENCE, TempDir, ctx_under, read_frame};

/// A session on its own socket, with everything a wait reads held by the test.
pub struct Rig {
    /// The session context — its bus is the one every wait subscribes to.
    pub ctx: Ctx,
    /// The view every connection's wait predicates read.
    pub status: StatusView,
    runtime: Runtime,
    _dir: TempDir,
    /// Kept alive so the `Core`'s receiver keeps reading the pinned shell.
    _config: watch::Sender<Config>,
}

impl Rig {
    /// Start a session whose bus replays `replay` events.
    pub async fn start(tag: &str, replay: usize) -> Self {
        let dir = TempDir::new(tag);
        let mut ctx = ctx_under(dir.path());
        ctx.bus = Arc::new(Bus::new(replay));
        let mut runtime = Runtime::new(ctx.clone());

        let mut pinned = Config::default();
        pinned.terminal.shell = Some("/bin/sh".to_owned());
        let (config_tx, config_rx) = watch::channel(pinned);

        let (core_tx, core_rx) = mpsc::channel(64);
        let mut core = Core::new(ctx.clone(), CoreHandle::new(core_tx.clone()));
        core.set_config(config_rx);
        runtime.spawn("core", async move {
            let _ = core.run(core_rx, |_: &Scheduled| {}).await;
        });

        let gateway =
            Gateway::bind(ctx.clone(), CoreHandle::new(core_tx)).expect("bind the session socket");
        let status = gateway.status_view();
        runtime.spawn("gateway", async move {
            let _ = gateway.run().await;
        });

        Self {
            ctx,
            status,
            runtime,
            _dir: dir,
            _config: config_tx,
        }
    }

    /// Where the session listens.
    pub fn socket(&self) -> &Path {
        &self.ctx.socket
    }

    /// Connect a wire client and complete the handshake.
    ///
    /// `attach` seeds an empty session with its first workspace, which is how
    /// a test gets a pane to name without splitting one.
    pub async fn wire(&self, attach: bool) -> Wire {
        let mut wire = Wire::connect(self.socket()).await;
        wire.hello(attach).await;
        wire
    }

    /// The bus head right now.
    pub fn head(&self) -> Seq {
        self.ctx.bus.head()
    }

    /// Publish `count` events nothing under test cares about.
    ///
    /// Synchronous and tight on purpose: on a current-thread runtime nothing
    /// else can run while this loop does, so a burst larger than the replay
    /// buffer leaves every live subscriber lapped — which is the only way to
    /// put a transition *inside* a gap rather than merely near one.
    pub fn flood(&self, pane: PaneId, count: usize) {
        for n in 0..count {
            self.ctx.bus.publish(Event::PaneTitle {
                pane,
                title: format!("filler {n}"),
            });
        }
    }

    /// Move a pane's agent status, publishing the event that describes it.
    ///
    /// Exactly what `AgentHub` will do: [`StatusView::commit`] writes the view
    /// and then publishes, in that order and with no way to ask for the other
    /// one.
    pub fn set_status(
        &self,
        pane: PaneId,
        from: Option<AgentState>,
        to: AgentState,
    ) -> Option<Seq> {
        self.commit(
            pane,
            to,
            vec![Event::AgentStatus {
                pane,
                from,
                to,
                cause: StatusCause::Hook,
            }],
        )
    }

    /// Move a pane's agent status **without** publishing anything.
    ///
    /// Not something the hub does — it is how this suite proves that a wait
    /// completes from *state*, since a transition with no event of its own can
    /// only ever be found by re-evaluating.
    pub fn set_status_silently(&self, pane: PaneId, to: AgentState) {
        self.commit(pane, to, Vec::new());
    }

    fn commit(&self, pane: PaneId, to: AgentState, events: Vec<Event>) -> Option<Seq> {
        self.status.commit(
            &self.ctx.bus,
            StatusUpdate {
                pane,
                status: Some(AgentSnapshot {
                    kind: None,
                    state: to,
                    cause: StatusCause::Hook,
                    transition_seq: self.ctx.bus.head() + 1,
                    attention: None,
                    session_ref: None,
                }),
                attention: if to.wants_attention() {
                    vec![pane]
                } else {
                    Vec::new()
                },
                events,
            },
        )
    }

    /// Cancel the session and join every task.
    pub async fn stop(self) {
        self.ctx.cancel.cancel();
        self.runtime.shutdown().await;
    }
}

/// A frame-level client that tells replies and notifications apart.
///
/// `support::Client` reads the next frame and calls it the reply, which stops
/// being true the moment a connection is subscribed: a delivery can arrive
/// between a request and its answer, and both halves of this suite depend on
/// neither one eating the other.
pub struct Wire {
    stream: UnixStream,
    next: u64,
    /// Deliveries that arrived while a reply was being waited for.
    deliveries: VecDeque<Delivery>,
}

impl Wire {
    /// Connect to a session socket.
    pub async fn connect(socket: &Path) -> Self {
        Self {
            stream: UnixStream::connect(socket).await.expect("connect"),
            next: 1,
            deliveries: VecDeque::new(),
        }
    }

    async fn hello(&mut self, attach: bool) {
        let hello = amx_proto::Hello {
            proto: amx_proto::version::window(),
            features: std::collections::BTreeSet::new(),
            client: amx_proto::ClientInfo {
                name: "amx-test".to_owned(),
                version: "0.0.0".to_owned(),
                term: None,
            },
            attach,
            resume: None,
        };
        self.send(&serde_json::to_vec(&hello).expect("encode hello"))
            .await;
        let (header, _payload) = read_frame(&mut self.stream).await;
        assert!(header.is_control(), "the welcome must be a control frame");
    }

    /// Send a request and answer with the id it went out under.
    pub async fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        let request = Request::new(RequestId::Number(id), method, Some(params));
        self.send(&serde_json::to_vec(&request).expect("encode request"))
            .await;
        id
    }

    /// Read frames until the reply to `id` arrives, buffering deliveries.
    pub async fn reply(&mut self, id: u64) -> Response {
        loop {
            let (header, payload) = read_frame(&mut self.stream).await;
            assert!(header.is_control(), "only control frames are expected here");
            let value: Value = serde_json::from_slice(&payload).expect("a JSON frame");
            if value.get("id").is_none() {
                self.absorb(&value);
                continue;
            }
            let response: Response = serde_json::from_value(value).expect("decode response");
            if response.id == RequestId::Number(id) {
                return response;
            }
        }
    }

    /// Send a request, read its reply and unwrap the result.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params).await;
        match self.reply(id).await.outcome {
            RpcOutcome::Result(value) => value,
            RpcOutcome::Error(err) => panic!("{method} failed: {} ({})", err.message, err.code),
        }
    }

    /// Send a request expecting it to fail, and answer with the error.
    pub async fn call_err(&mut self, method: &str, params: Value) -> amx_proto::RpcError {
        let id = self.request(method, params).await;
        match self.reply(id).await.outcome {
            RpcOutcome::Error(err) => err,
            RpcOutcome::Result(value) => panic!("{method} unexpectedly succeeded: {value}"),
        }
    }

    /// The next event delivery, waiting for one if none is buffered.
    pub async fn delivery(&mut self) -> Delivery {
        loop {
            if let Some(delivery) = self.deliveries.pop_front() {
                return delivery;
            }
            let (header, payload) = read_frame(&mut self.stream).await;
            assert!(header.is_control(), "a delivery is a control frame");
            let value: Value = serde_json::from_slice(&payload).expect("a JSON frame");
            assert!(
                value.get("id").is_none(),
                "a reply arrived where a delivery was expected: {value}"
            );
            self.absorb(&value);
        }
    }

    /// Whether a reply for `id` has arrived within `PATIENCE`.
    pub async fn replied_within(&mut self, id: u64, patience: std::time::Duration) -> bool {
        tokio::time::timeout(patience, self.reply(id)).await.is_ok()
    }

    /// Subscribe this connection to the bus and answer with the reply's seq.
    pub async fn subscribe(&mut self, after_seq: Option<Seq>) -> Seq {
        let params = match after_seq {
            Some(seq) => json!({ "after_seq": seq }),
            None => json!({}),
        };
        let reply = self.call("events.subscribe", params).await;
        reply["seq"]
            .as_u64()
            .expect("the subscribe reply carries a seq")
    }

    /// The pane the seeded workspace was given.
    pub async fn seeded_pane(&mut self) -> PaneId {
        let state = self.call("session.state", json!({})).await;
        state["panes"][0]["pane"]
            .as_str()
            .expect("the seeded workspace has a pane")
            .parse()
            .expect("a pane id")
    }

    fn absorb(&mut self, value: &Value) {
        let notification: Notification =
            serde_json::from_value(value.clone()).expect("decode notification");
        assert_eq!(
            notification.method,
            amx_proto::control::wait::EVENT_METHOD,
            "the only notification this build sends is an event delivery",
        );
        let params = notification.params.expect("a delivery carries params");
        let delivery: Delivery = serde_json::from_value(params).expect("decode delivery");
        self.deliveries.push_back(delivery);
    }

    async fn send(&mut self, payload: &[u8]) {
        let header = FrameHeader::new(payload.len() as u32, amx_proto::frame::CONTROL_CHANNEL);
        self.stream
            .write_all(&header.encode())
            .await
            .expect("write a header");
        self.stream.write_all(payload).await.expect("write a body");
    }
}

/// How long a wait that must not hang is given before the test fails.
pub const WAIT_PATIENCE: std::time::Duration = PATIENCE;
