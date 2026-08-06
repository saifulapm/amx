//! Damage coalescing, keyframes and flow control (04 §4).
//!
//! > **Flow control is part of protocol v1, not a later optimization.** Damage
//! > deltas are incremental — unlike herdr's full-frame diffs they cannot be
//! > dropped without corrupting the client grid — so:
//! >
//! > - Per client, per visible pane, the server keeps an accumulated
//! >   dirty-region set (rects + grid generation), not a queue of deltas. When
//! >   the client's writer is not ready, new damage coalesces into that set; on
//! >   drain the server emits one delta built from the current authoritative
//! >   grid. Cost is O(dirty bookkeeping) per client — cell contents are always
//! >   read from the single authoritative grid at send time.
//! > - When accumulated damage crosses a threshold (or on reconnect/resync),
//! >   the server sends a **keyframe** (`grid.reset` + full visible grid).
//!
//! [`GridStream`] is one client's view of one pane, and it is the whole
//! mechanism:
//!
//! - [`DirtySet`] is the accumulated set. It is a row bitmap sized by the pane,
//!   so a client that stalls for a minute under `cat /dev/urandom` costs the
//!   same three words it cost when it was keeping up. This is the bound, and it
//!   is structural rather than enforced by a cap somewhere.
//! - [`Encoder`] reads cells out of the snapshot it is handed *at send time*.
//!   Nothing about a delta is decided when the damage happens except which rows
//!   to look at.
//! - [`KeyframePolicy`] decides when a delta stops being worth trusting.
//!
//! ## When is the writer "not ready"?
//!
//! When anything is still queued in the writer's grid class
//! ([`Priority::Grid`](amx_proto::stream::Priority::Grid)). That is a
//! connection-wide question rather than a
//! per-stream one, and deliberately so: T10's writer drains the grid class
//! strictly, as one queue, so "this client is keeping up" is a property of the
//! client and not of which pane it is watching. A stream therefore has at most
//! one frame queued ahead of it, and everything that happens meanwhile lands in
//! the dirty set instead — which is the sentence from 04 §4, implemented
//! literally.
//!
//! ## Missed publications
//!
//! [`amx_vt::Snapshot::damage`] means "rows that changed since the previously
//! *published* frame", and [`SnapshotFeed`] is a `watch` slot: a reader that
//! falls behind sees the newest frame and never the ones in between. Their
//! damage lists would be lost, so [`GridStream::absorb`] checks the snapshot's
//! publication counter for continuity and marks the whole grid when it finds a
//! gap. Every published snapshot is a *complete* copy of the visible grid, so
//! nothing is lost — only the shortcut is.
//!
//! ## The wire codec lives here
//!
//! [`GridMessage::encode`](amx_proto::stream::GridMessage::encode) is still
//! `todo!()` in `amx-proto`, and its `decode` counterpart cannot be implemented
//! as declared: it promises a borrowed `&'a [DamageRect]` out of a `&'a [u8]`,
//! which no safe code can produce from an arbitrarily-aligned little-endian
//! payload. So the layout lives in [`codec`], [`cell`] and [`decode`], next to
//! the encoder that produces it — the same choice
//! [`history::pack`](crate::history::pack) already made for history rows. When
//! `amx-proto` grows a real codec it should be this one, with `decode` taking
//! the rect buffer as an argument.

pub mod cell;
pub mod codec;
pub mod decode;
pub mod dirty;
pub mod encode;
pub mod keyframe;
pub mod stream;

use std::time::Duration;

use amx_proto::stream::{FlowControl, StreamId};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use self::dirty::DirtySet;
pub use self::encode::Encoder;
pub use self::keyframe::{KeyframePolicy, KeyframeReason};
pub use self::stream::{DamageStats, GridStream, GridStreamConfig, Sent, SentKind};

use crate::actor::SnapshotFeed;
use crate::conn::writer::{Outbound, OutboundError};

/// How long a stalled stream waits before trying the writer again.
///
/// Nothing wakes a stream when the writer drains — the writer's `Notify` is for
/// producers waiting for *room*, and a grid stream never waits for room — so a
/// stream that declined to send re-checks on a timer. Short enough to be
/// invisible next to the pane's own frame interval, long enough that a stalled
/// client is not a spin loop.
pub const DRAIN_RETRY: Duration = Duration::from_millis(4);

/// A grid stream could not produce or queue a frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum DamageError {
    /// The message does not fit the stream's negotiated frame cap.
    ///
    /// A delta splits across frames rather than hitting this; it is reachable
    /// for a keyframe, which is indivisible, and for a single row wider than
    /// the whole cap. Both mean the cap is too small for the pane.
    #[error("a {kind} of {len} bytes exceeds the stream's frame cap of {cap}")]
    FrameTooLarge {
        /// What was being built.
        kind: &'static str,
        /// Bytes it needed.
        len: usize,
        /// The cap that applies.
        cap: usize,
    },
    /// The writer refused the frame.
    #[error(transparent)]
    Outbound(#[from] OutboundError),
    /// The stream id has no channel byte left to bind.
    #[error("stream {stream:?} has no frame channel")]
    Unbindable {
        /// The stream that could not be bound.
        stream: StreamId,
    },
}

/// Drive one client's grid stream until the pane, the client or the session
/// ends, and report what it did.
///
/// The loop is: absorb whatever the pane published, try to drain, then wait for
/// the next thing that could change either — a new frame, a flow-control
/// signal, or the writer having had time to drain a frame this stream declined
/// to add to.
pub async fn pump(
    mut stream: GridStream,
    mut frames: SnapshotFeed,
    out: Outbound,
    mut flow: mpsc::Receiver<FlowControl>,
    cancel: CancellationToken,
) -> DamageStats {
    let mut flow_open = true;
    loop {
        let snapshot = frames.latest();
        stream.absorb(&snapshot, frames.generation());
        match stream.flush(&snapshot, &out) {
            Ok(_) => {}
            Err(DamageError::Outbound(OutboundError::Closed)) => break,
            Err(error) => {
                tracing::warn!(pane = %stream.pane(), %error, "grid stream frame dropped");
            }
        }
        drop(snapshot);

        let retry = stream.owes();
        tokio::select! {
            () = cancel.cancelled() => break,
            fresh = frames.changed() => {
                if !fresh {
                    break;
                }
            }
            signal = flow.recv(), if flow_open => match signal {
                Some(signal) => stream.control(signal),
                None => flow_open = false,
            },
            () = sleep_if(retry) => {}
        }
    }
    stream.stats()
}

/// Sleep for the retry interval, or never.
async fn sleep_if(retry: bool) {
    if retry {
        tokio::time::sleep(DRAIN_RETRY).await;
    } else {
        std::future::pending().await
    }
}
