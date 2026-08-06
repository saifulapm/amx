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
//! ## The wire codec lives in `amx-proto`
//!
//! The byte layout is `amx_proto::stream::{codec, cell, grid}`, shared with
//! the client; [`codec`] here is only the adapter from `amx-vt`'s snapshot
//! types into that wire vocabulary.

pub mod codec;
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

/// The smallest frame cap a client may negotiate on a grid stream.
///
/// A keyframe is indivisible, so a cap below what one costs turns the stream
/// into a retry loop that can never succeed; `max_frame: 0` would spin at the
/// retry interval forever. Caps below this floor are clamped to it at
/// [`GridStream::control`] time. The value matches what a default 24×80 grid's
/// keyframe comfortably fits in; a grid too large even for a client's clamped
/// cap is a terminal stream error, not a retry.
pub const MIN_STREAM_FRAME: u32 = 64 * 1024;

/// How many consecutive keyframes may fail to fit before the stream is torn
/// down instead of retried.
///
/// The count only moves on an owed keyframe: a keyframe cannot split, so a
/// cap that cannot carry one will never carry one unless the client raises it
/// — each new publication retries once, and a raise arriving in between
/// resets the count via the successful flush.
pub const KEYFRAME_CAP_STRIKES: u32 = 3;

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
    let mut cap_strikes: u32 = 0;
    loop {
        // One read for both halves: the snapshot and the generation it was
        // published under travel in the same slot, so a resize landing
        // between two separate reads can never pair one publication's cells
        // with another's generation.
        let (snapshot, generation) = frames.frame();
        stream.absorb(&snapshot, generation);
        match stream.flush(&snapshot, &out) {
            Ok(_) => cap_strikes = 0,
            Err(DamageError::Outbound(OutboundError::Closed)) => break,
            Err(error @ DamageError::FrameTooLarge { .. }) if stream.owed_keyframe().is_some() => {
                // A keyframe cannot split, so retrying against the same cap
                // can never succeed. Give the client a bounded chance to
                // raise it, then treat the stream as broken rather than
                // retrying at the drain interval forever.
                cap_strikes += 1;
                if cap_strikes >= KEYFRAME_CAP_STRIKES {
                    tracing::warn!(
                        pane = %stream.pane(),
                        %error,
                        "grid stream cap cannot carry a keyframe; closing the stream"
                    );
                    break;
                }
                tracing::warn!(pane = %stream.pane(), %error, "grid stream keyframe dropped");
            }
            Err(error) => {
                tracing::warn!(pane = %stream.pane(), %error, "grid stream frame dropped");
            }
        }
        drop(snapshot);

        // The timer retry exists for one case: the writer had not drained. A
        // paused stream's flush is a no-op until a flow signal arrives, and
        // an unfittable keyframe stays unfittable until the cap changes —
        // arming the timer for either is a busy loop dressed as patience.
        let retry = stream.owes() && !stream.is_paused() && cap_strikes == 0;
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
            () = sleep_if(retry) => stream.note_retry(),
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
