//! One client connection: negotiate, decode, dispatch, write.
//!
//! A connection is a single task. It owns both halves of the socket and runs
//! its reader and its writer as two futures inside that one task, so the
//! gateway's `JoinSet` accounting is one entry per connected client and there
//! is no second task that can outlive it. Whichever half finishes first ends
//! the connection: a disconnected peer stops the reader, a broken socket stops
//! the writer, and cancellation stops both.
//!
//! Order of business on a fresh socket:
//!
//! 1. read the opening [`Hello`](amx_proto::Hello) frame;
//! 2. ask the `Core` who it is and where the event stream has got to;
//! 3. answer with a [`Welcome`](amx_proto::Welcome) carrying the negotiated
//!    version, the feature intersection and that sequence number;
//! 4. serve frames until the peer goes away.
//!
//! Step 2 goes through the ordinary `ping` mailbox call rather than a private
//! back door, so the identity in the welcome is the same identity every later
//! reply carries.

pub mod events;
pub mod negotiate;
pub mod reader;
pub mod streams;
pub mod writer;

use std::sync::Arc;

use amx_core::{ClientId, Ctx, Event};
use amx_proto::control::{Dispatch, session};
use amx_proto::error::{FrameError, NegotiationError};
use amx_proto::rpc::RpcError;
use thiserror::Error;
use tokio::net::UnixStream;

use crate::actor::{CoreCommand, CoreHandle, SessionCall, StatusView};
use crate::conn::reader::Reader;
use crate::conn::writer::{OutboundError, WriteError};
use crate::dispatch::Router;

/// A connection ended for a reason that is not a clean disconnect.
#[derive(Debug, Error)]
pub enum ConnError {
    /// The socket failed.
    #[error("connection i/o failed: {0}")]
    Io(#[from] std::io::Error),
    /// The peer went away in the middle of a frame.
    ///
    /// Distinct from a clean disconnect on purpose: a frame boundary is where
    /// a client is allowed to leave, and anywhere else means the peer or the
    /// transport broke.
    #[error("peer disconnected mid-frame")]
    Truncated,
    /// The framing layer refused a frame.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// Hello/Welcome did not produce a usable session.
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
    /// The first frame was not a hello.
    #[error("first frame was not a hello")]
    NotAHello,
    /// The peer closed before saying hello.
    #[error("peer closed before saying hello")]
    ClosedBeforeHello,
    /// A frame's payload was not the shape its channel requires.
    #[error("malformed control frame: {0}")]
    Malformed(&'static str),
    /// A message this server produced could not be serialized.
    #[error("failed to encode an outgoing message")]
    Encode,
    /// A frame could not be queued for writing.
    #[error(transparent)]
    Outbound(#[from] OutboundError),
    /// The writer failed.
    #[error(transparent)]
    Write(#[from] WriteError),
    /// A call the connection itself made on the `Core` failed.
    #[error("core call failed: {}", .0.message)]
    Core(RpcError),
}

impl ConnError {
    /// Classify a read failure: end-of-file part way through a frame is a torn
    /// frame, anything else is transport failure.
    #[must_use]
    pub fn truncated(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            Self::Truncated
        } else {
            Self::Io(err)
        }
    }
}

/// Serve one connected client until it disconnects or the session shuts down.
///
/// `status` is the [`StatusView`] `AgentHub` writes and wait predicates read
/// (`docs/08-m2-plan.md` §3). It arrives as a parameter rather than being
/// fetched, for the same reason the hub is *handed* a `SnapshotFeed`: a
/// connection asking an actor for shared state is a round trip it would then
/// have to repeat on every evaluation, and a wait predicate has to read live
/// state synchronously.
pub async fn serve(
    stream: UnixStream,
    ctx: Ctx,
    core: CoreHandle,
    status: StatusView,
) -> Result<(), ConnError> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = Reader::new(read_half);
    let mut router = Router::new(core);

    let hello = negotiate::read_hello(&mut reader).await?;
    // An attach to a session with no workspaces seeds its first one, and the
    // reply is awaited *before* the welcome is written: by the time the
    // client can render anything there is a workspace with a live shell for
    // it to show, never an empty session. A failed seed (no spawnable shell)
    // is logged and the attach proceeds — the client gets the same empty
    // session it would have seen before, not a refused connection.
    if hello.attach {
        let seeded = router
            .call(|reply| CoreCommand::Session(SessionCall::Attached { reply }))
            .await;
        if let Err(err) = seeded {
            tracing::warn!(error = %err.message, "could not seed the first workspace");
        }
    }
    let identity = Dispatch::ping(&mut router, session::PingParams {})
        .await
        .map_err(ConnError::Core)?;
    let welcome = hello.accept(
        identity.server,
        &negotiate::supported_features(),
        identity.seq,
        identity.session,
    )?;
    negotiate::write_welcome(&mut write_half, &welcome).await?;

    // The gateway is the only authority on client lifecycle, so it is the one
    // publisher of these two transitions — the same "one publisher per
    // transition" rule that keeps the `Core` the sole publisher of state-tree
    // events (04 §2).
    let client = ClientId::new_v4();
    ctx.bus.publish(Event::ClientAttached { client });
    tracing::debug!(%client, proto = welcome.proto, "client attached");

    let (outbound, queue) = writer::channel();
    // Stream bindings are connection state: the pumps spawned for bound grid
    // streams write into this connection's priority writer and die with it.
    let streams = streams::ConnStreams::new(outbound.clone(), ctx.cancel.child_token());
    reader.share_caps(streams.caps());
    router.attach_streams(streams.clone());
    // So is the event subscription, and so are the long polls: cursors are
    // per-connection state, which is why there is nothing to persist and
    // nothing to leak (D-M2-5).
    let events = events::ConnEvents::new(
        Arc::clone(&ctx.bus),
        status,
        outbound.clone(),
        ctx.cancel.child_token(),
    );
    router.attach_events(events.clone());

    let outcome = tokio::select! {
        result = reader::run(&mut reader, &mut router, &streams, &events, &outbound, &ctx.cancel) => result,
        result = writer::run(write_half, queue, ctx.cancel.clone()) => {
            result.map(|_report| ()).map_err(ConnError::from)
        }
    };

    // Joined before the detach is published: no pump and no long poll outlives
    // its connection.
    events.shutdown().await;
    streams.shutdown().await;
    ctx.bus.publish(Event::ClientDetached { client });
    tracing::debug!(%client, "client detached");
    outcome
}
