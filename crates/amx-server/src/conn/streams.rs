//! Per-connection stream bindings: the `stream.bind` machinery.
//!
//! 04 §4: a binary stream is "bound by a control call" and then speaks only
//! its own kind. Binding is connection-scoped state — the channel byte means
//! something only on the connection that bound it — so it lives here, beside
//! the reader and writer it configures, not in the `Core`:
//!
//! - a **pane grid** bind spawns T11's [`pump`](crate::damage::pump) onto the
//!   connection's task tracker: the pane's published frames flow through a
//!   [`GridStream`](crate::damage::GridStream)'s dirty set into this
//!   connection's priority writer, keyframe first;
//! - a **raw pane I/O** bind registers the channel with the reader's
//!   [`StreamCaps`](super::reader::StreamCaps), which is what allows inbound
//!   frames on it at all, and records where their bytes go;
//! - a **history** bind records the channel `pane.history` chunks ride out on.
//!
//! Everything spawned here lands in a handle list the connection task joins
//! on its way out — nothing detached, everything joined (04 §2); the bindings
//! die with the connection, and stream ids are never reused within one (a
//! late frame for a closed stream must read as stale, not land on whatever
//! took the channel next).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use amx_core::PaneId;
use amx_proto::control::stream::BindReply;
use amx_proto::frame::DEFAULT_STREAM_FRAME;
use amx_proto::rpc::RpcError;
use amx_proto::stream::{FlowControl, StreamId, StreamKind};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::reader::StreamCaps;
use super::writer::Outbound;
use crate::actor::{PaneHandle, PaneWiring};
use crate::damage::{GridStream, GridStreamConfig, pump};

/// Depth of a stream's flow-control mailbox.
const FLOW_DEPTH: usize = 8;

/// Where frames arriving on a bound inbound channel go.
#[derive(Clone, Debug)]
pub struct RawRoute {
    /// The pane the channel was bound for.
    pub pane: PaneId,
    /// Its command mailbox.
    pub handle: PaneHandle,
}

/// A bound history stream: the channel `pane.history` chunks ride out on.
#[derive(Clone, Debug)]
pub struct HistoryRoute {
    /// The outbound channel byte.
    pub channel: u8,
    /// The pane's command mailbox, for the range read itself.
    pub handle: PaneHandle,
}

#[derive(Debug, Default)]
struct Bindings {
    /// The next stream id to issue. Starts past the control channel and never
    /// goes back: ids are not reused on one connection.
    next: u16,
    /// Inbound raw routes by channel byte.
    raw: HashMap<u8, RawRoute>,
    /// Outbound history routes by pane.
    history: HashMap<PaneId, HistoryRoute>,
    /// Flow-control senders by stream, for bound grid pumps.
    flows: HashMap<StreamId, mpsc::Sender<FlowControl>>,
    /// Every pump this connection spawned, joined at connection end.
    pumps: Vec<JoinHandle<crate::damage::DamageStats>>,
}

/// One connection's stream bindings, shared between its reader, its dispatch
/// handlers and the tasks it spawns.
#[derive(Clone, Debug)]
pub struct ConnStreams {
    caps: Arc<Mutex<StreamCaps>>,
    bindings: Arc<Mutex<Bindings>>,
    outbound: Outbound,
    cancel: CancellationToken,
}

impl ConnStreams {
    /// Stream state for one connection: frames go out through `outbound`,
    /// and every pump this state spawns stops with `cancel`.
    #[must_use]
    pub fn new(outbound: Outbound, cancel: CancellationToken) -> Self {
        Self {
            caps: Arc::new(Mutex::new(StreamCaps::default())),
            bindings: Arc::new(Mutex::new(Bindings {
                next: 1,
                ..Bindings::default()
            })),
            outbound,
            cancel,
        }
    }

    /// The inbound cap table, for the connection's reader.
    #[must_use]
    pub fn caps(&self) -> Arc<Mutex<StreamCaps>> {
        Arc::clone(&self.caps)
    }

    /// The connection's outbound writer handle.
    #[must_use]
    pub fn outbound(&self) -> &Outbound {
        &self.outbound
    }

    /// Bind one stream, wiring whatever its kind needs.
    ///
    /// # Errors
    ///
    /// A typed refusal for the reserved graphics kind, for a connection that
    /// has exhausted its channel bytes, and for a pump that cannot start.
    pub fn bind(&self, kind: StreamKind, wiring: &PaneWiring) -> Result<BindReply, RpcError> {
        let stream = self.allocate()?;
        // `allocate` only returns ids with a channel byte.
        let Some(channel) = stream.channel() else {
            return Err(no_channels());
        };
        match kind {
            StreamKind::PaneGrid { pane } => {
                let grid = GridStream::new(GridStreamConfig::new(pane, stream))
                    .map_err(|err| RpcError::new(RpcError::INTERNAL_ERROR, err.to_string()))?;
                let (flow_tx, flow_rx) = mpsc::channel(FLOW_DEPTH);
                self.lock_bindings().flows.insert(stream, flow_tx);
                let frames = wiring.frames.clone();
                let out = self.outbound.clone();
                let cancel = self.cancel.clone();
                let task =
                    tokio::spawn(async move { pump(grid, frames, out, flow_rx, cancel).await });
                self.lock_bindings().pumps.push(task);
            }
            StreamKind::RawPaneIo { pane } => {
                self.lock_caps().bind(channel, DEFAULT_STREAM_FRAME);
                self.lock_bindings().raw.insert(
                    channel,
                    RawRoute {
                        pane,
                        handle: wiring.handle.clone(),
                    },
                );
            }
            StreamKind::History { pane } => {
                self.lock_bindings().history.insert(
                    pane,
                    HistoryRoute {
                        channel,
                        handle: wiring.handle.clone(),
                    },
                );
            }
            StreamKind::Graphics { .. } => {
                return Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    "the graphics stream kind is reserved and not carried in M0",
                ));
            }
        }
        Ok(BindReply {
            stream,
            channel,
            max_frame: DEFAULT_STREAM_FRAME,
        })
    }

    /// The raw route bound to `channel`, if any.
    #[must_use]
    pub fn raw_route(&self, channel: u8) -> Option<RawRoute> {
        self.lock_bindings().raw.get(&channel).cloned()
    }

    /// The history route bound for `pane`, if any.
    #[must_use]
    pub fn history_route(&self, pane: PaneId) -> Option<HistoryRoute> {
        self.lock_bindings().history.get(&pane).cloned()
    }

    /// Route a flow-control signal to the stream it names.
    pub async fn control(&self, flow: FlowControl) {
        let stream = match flow {
            FlowControl::StreamCap { stream, .. }
            | FlowControl::Pause { stream }
            | FlowControl::Resume { stream }
            | FlowControl::Resync { stream } => stream,
        };
        let sender = self.lock_bindings().flows.get(&stream).cloned();
        if let Some(sender) = sender {
            let _ = sender.send(flow).await;
        }
    }

    /// Stop every pump and wait for each one — the connection task's last act.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let pumps = std::mem::take(&mut self.lock_bindings().pumps);
        for pump in pumps {
            let _ = pump.await;
        }
    }

    fn allocate(&self) -> Result<StreamId, RpcError> {
        let mut bindings = self.lock_bindings();
        let id = bindings.next;
        if id > u16::from(u8::MAX) {
            return Err(no_channels());
        }
        bindings.next += 1;
        Ok(StreamId::new(id))
    }

    fn lock_bindings(&self) -> MutexGuard<'_, Bindings> {
        // Every critical section is a map access; a panic cannot happen inside
        // one, so poisoning carries no torn state.
        self.bindings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_caps(&self) -> MutexGuard<'_, StreamCaps> {
        // Same shape as `lock_bindings`: plain array writes only.
        self.caps.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn no_channels() -> RpcError {
    RpcError::new(
        RpcError::INTERNAL_ERROR,
        "this connection has no stream channels left; reconnect to rebind",
    )
}
