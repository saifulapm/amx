//! One client's view of one pane: the accumulated set, and the drain.
//!
//! Split out of [`crate::damage`] by the module budget; the mechanism and the
//! reasoning behind it are documented there.

use amx_core::{GridGeneration, PaneId};
use amx_proto::stream::{Cursor, FlowControl, Priority, StreamId};
use amx_vt::Snapshot;

use super::DamageError;
use super::codec;
use super::dirty::DirtySet;
use super::encode::Encoder;
use super::keyframe::{KeyframePolicy, KeyframeReason};
use crate::conn::writer::{OutFrame, Outbound};

/// What one client's grid stream did with the damage it was given.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DamageStats {
    /// Published frames folded into the dirty set.
    pub absorbed: u64,
    /// Absorbs that found damage already pending — coalescing, counted.
    pub coalesced: u64,
    /// Publications this stream never saw, and marked the grid for.
    pub skipped: u64,
    /// Keyframes sent.
    pub keyframes: u64,
    /// Deltas sent.
    pub deltas: u64,
    /// Cursor-only messages sent.
    pub cursors: u64,
    /// Deltas that stopped at the frame cap with rows still owed.
    pub partials: u64,
    /// Flushes declined because the writer had not drained.
    pub stalls: u64,
    /// Timer wakeups taken while something was owed.
    ///
    /// The pump's drain-retry timer, counted: a paused stream or one whose
    /// keyframe cannot fit its cap must not show up here — its wakeups come
    /// from flow signals and new frames, never from spinning on the clock.
    pub retries: u64,
    /// Payload bytes queued.
    pub bytes: u64,
}

impl DamageStats {
    /// Frames of every kind queued for the client.
    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.keyframes + self.deltas + self.cursors
    }
}

/// What kind of message a flush produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SentKind {
    /// A full visible grid, for the stated reason.
    Keyframe(KeyframeReason),
    /// An incremental update.
    Delta,
    /// The cursor and nothing else.
    Cursor,
}

/// One message this stream put on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sent {
    /// Keyframe, delta or cursor.
    pub kind: SentKind,
    /// The generation the cells were read at.
    pub generation: GridGeneration,
    /// Payload bytes.
    pub bytes: usize,
    /// Rows the message carried.
    pub rows: u16,
    /// Whether it emptied the dirty set.
    pub complete: bool,
}

/// Everything one client's view of one pane needs.
#[derive(Clone, Copy, Debug)]
pub struct GridStreamConfig {
    /// The pane being watched.
    pub pane: PaneId,
    /// The bound stream.
    pub stream: StreamId,
    /// The frame cap negotiated for it.
    pub max_frame: u32,
    /// When a delta gives way to a keyframe.
    pub policy: KeyframePolicy,
    /// The pane's live grid generation at bind time.
    pub live: GridGeneration,
    /// The generation the client says it already holds for the pane, when the
    /// bind is a re-bind after a reconnect (`docs/09-m3-plan.md` D-M3-7).
    ///
    /// `None` is a fresh bind and opens with a [`KeyframeReason::First`]
    /// keyframe, which is what every bind did before this field existed. Equal
    /// to `live` opens delta-only. Anything else is stale, and opens with a
    /// [`KeyframeReason::Generation`] keyframe.
    pub resume: Option<GridGeneration>,
}

impl GridStreamConfig {
    /// A config at the default frame cap and keyframe policy, for a fresh
    /// bind of a pane whose generation nobody has looked up.
    #[must_use]
    pub fn new(pane: PaneId, stream: StreamId) -> Self {
        Self {
            pane,
            stream,
            max_frame: amx_proto::frame::DEFAULT_STREAM_FRAME,
            policy: KeyframePolicy::default(),
            live: GridGeneration::FIRST,
            resume: None,
        }
    }

    /// The same config for a client re-binding a pane it already holds cells
    /// for at `held`, against the pane's `live` generation.
    #[must_use]
    pub fn resuming(mut self, live: GridGeneration, held: Option<GridGeneration>) -> Self {
        self.live = live;
        self.resume = held;
        self
    }

    /// Why this stream opens as it does: `None` when the client's generation
    /// makes a keyframe unnecessary.
    fn opening_keyframe(&self) -> Option<KeyframeReason> {
        match self.resume {
            Some(held) if held == self.live => None,
            Some(_) => Some(KeyframeReason::Generation),
            None => Some(KeyframeReason::First),
        }
    }
}

/// One client's accumulated damage for one visible pane.
#[derive(Debug)]
pub struct GridStream {
    pane: PaneId,
    stream: StreamId,
    channel: u8,
    max_frame: u32,
    policy: KeyframePolicy,
    dirty: DirtySet,
    encoder: Encoder,
    /// The generation the accumulated set belongs to.
    generation: GridGeneration,
    /// Set while a keyframe is owed, with the reason it is owed.
    keyframe: Option<KeyframeReason>,
    /// Set on a stream that opened delta-only and has not yet seen a frame.
    ///
    /// The client's cells *are* this stream's starting state, so the first
    /// publication is adopted rather than treated as news; see
    /// [`Self::absorb`].
    resumed: bool,
    paused: bool,
    /// Publication counter of the last snapshot folded in.
    seen: Option<u64>,
    /// The cursor as the client last saw it.
    cursor: Option<Cursor>,
    stats: DamageStats,
}

impl GridStream {
    /// Start a stream that owes its client whatever its config says it owes.
    ///
    /// A fresh bind owes a [`First`](KeyframeReason::First) keyframe, as every
    /// bind has since M0. A re-bind presenting the generation the pane is
    /// already at owes nothing — it opens delta-only, and the first
    /// publication is adopted rather than repainted. A re-bind presenting any
    /// other generation owes a [`Generation`](KeyframeReason::Generation)
    /// keyframe, which is 04 §6's "keyframes for stale grids".
    ///
    /// # Errors
    ///
    /// [`DamageError::Unbindable`] if the stream id has no frame channel —
    /// [`StreamId::channel`] runs out past 255, which is a protocol-visible
    /// condition and not something to truncate silently.
    pub fn new(config: GridStreamConfig) -> Result<Self, DamageError> {
        let channel = config.stream.channel().ok_or(DamageError::Unbindable {
            stream: config.stream,
        })?;
        let keyframe = config.opening_keyframe();
        Ok(Self {
            pane: config.pane,
            stream: config.stream,
            channel,
            max_frame: config.max_frame,
            policy: config.policy,
            dirty: DirtySet::default(),
            encoder: Encoder::new(),
            generation: config.live,
            keyframe,
            resumed: keyframe.is_none(),
            paused: false,
            seen: None,
            cursor: None,
            stats: DamageStats::default(),
        })
    }

    /// The pane this stream carries.
    #[must_use]
    pub const fn pane(&self) -> PaneId {
        self.pane
    }

    /// The bound stream id.
    #[must_use]
    pub const fn stream(&self) -> StreamId {
        self.stream
    }

    /// What this stream has done so far.
    #[must_use]
    pub const fn stats(&self) -> DamageStats {
        self.stats
    }

    /// The accumulated dirty set.
    #[must_use]
    pub const fn dirty(&self) -> &DirtySet {
        &self.dirty
    }

    /// Whether a keyframe is owed, and why.
    #[must_use]
    pub const fn owed_keyframe(&self) -> Option<KeyframeReason> {
        self.keyframe
    }

    /// Whether the client is owed anything at all.
    #[must_use]
    pub const fn owes(&self) -> bool {
        self.keyframe.is_some() || !self.dirty.is_empty()
    }

    /// Whether the client has paused this stream.
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        self.paused
    }

    /// Bytes this stream's bookkeeping and scratch hold on the heap.
    #[must_use]
    pub fn bookkeeping_bytes(&self) -> usize {
        self.dirty.accounted_bytes() + self.encoder.accounted_bytes()
    }

    /// An upper bound on the bytes held on this client's behalf for this pane:
    /// bookkeeping, scratch, and whatever the writer has not put on the wire.
    ///
    /// The writer reports queue depth rather than queued bytes, so the queued
    /// part is counted at the frame cap — an over-estimate, which is the right
    /// direction for a bound.
    #[must_use]
    pub fn accounted_bytes(&self, out: &Outbound) -> usize {
        self.bookkeeping_bytes() + out.depth(Priority::Grid) * self.max_frame as usize
    }

    /// Fold a published frame's damage into the accumulated set.
    ///
    /// Cheap by construction: a handful of bit sets, whatever the frame
    /// contained. Nothing is read out of the snapshot's cells here — that
    /// happens in [`Self::flush`], at send time.
    pub fn absorb(&mut self, snapshot: &Snapshot, generation: GridGeneration) {
        if self.resumed {
            self.resumed = false;
            // The first frame a delta-only stream sees is the grid its client
            // is already looking at, so it seeds the shape and the publication
            // counter and marks nothing: reshaping from zero would read as a
            // resize and a whole-grid mark would cross the keyframe threshold,
            // both of which would repaint a screen that is already right. If
            // the generation moved between the bind and this frame the client
            // is stale after all, and the ordinary path below says so.
            if generation == self.generation {
                self.dirty.reshape(snapshot.rows(), snapshot.cols());
                self.seen = Some(snapshot.generation());
                self.stats.absorbed += 1;
                return;
            }
            // Recorded before the reshape below can claim the reason: the
            // generation is why this client's cells stopped being usable, and
            // the resize is only how.
            self.need(KeyframeReason::Generation);
        }
        if self.dirty.reshape(snapshot.rows(), snapshot.cols()) {
            self.need(KeyframeReason::Resize);
        }
        if generation != self.generation {
            self.generation = generation;
            self.need(KeyframeReason::Generation);
        }

        let published = snapshot.generation();
        let pending = self.owes();
        match self.seen {
            Some(last) if last == published => return,
            Some(last) if published != last.wrapping_add(1) => {
                // Frames we never saw carried damage lists we will never see.
                // The published grid is still complete, so the whole grid is
                // the only honest description of what might have changed.
                self.stats.skipped += 1;
                self.dirty.mark_all();
            }
            Some(_) => {
                for &row in snapshot.damage() {
                    self.dirty.mark(row);
                }
            }
            None => self.dirty.mark_all(),
        }
        self.seen = Some(published);
        self.stats.absorbed += 1;
        if pending {
            // Damage was already waiting when this frame arrived: the two are
            // now one set rather than two deltas. That is the coalescing.
            self.stats.coalesced += 1;
        }
        if self.policy.crossed(self.dirty.marked(), self.dirty.rows()) {
            self.need(KeyframeReason::Threshold);
        }
    }

    /// Record one drain-retry timer wakeup.
    pub fn note_retry(&mut self) {
        self.stats.retries += 1;
    }

    /// Apply a flow-control signal from the client.
    ///
    /// A requested cap below [`MIN_STREAM_FRAME`](super::MIN_STREAM_FRAME) is
    /// clamped to it: a keyframe is indivisible, so a cap of, say, zero would
    /// make every future keyframe unbuildable and the stream permanently
    /// unrecoverable — a floor is part of the protocol, not a courtesy.
    pub fn control(&mut self, flow: FlowControl) {
        match flow {
            FlowControl::StreamCap { stream, max_frame } if stream == self.stream => {
                self.max_frame = max_frame.max(super::MIN_STREAM_FRAME);
            }
            FlowControl::Pause { stream } if stream == self.stream => self.paused = true,
            FlowControl::Resume { stream } if stream == self.stream => self.paused = false,
            FlowControl::Resync { stream } if stream == self.stream => {
                self.need(KeyframeReason::Resync);
            }
            _ => {}
        }
    }

    /// Build and queue one message, if the client is owed one and the writer
    /// has drained.
    ///
    /// `snapshot` is the authoritative grid *now*: whatever cells go out are
    /// read from it during this call, never from anything captured when the
    /// damage happened.
    ///
    /// Nothing is cleared until the frame is accepted, so a refused send leaves
    /// the dirty set exactly as it was and the next drain sends it.
    ///
    /// # Errors
    ///
    /// [`DamageError::FrameTooLarge`] if the message does not fit the stream's
    /// cap, and [`DamageError::Outbound`] if the writer refuses the frame.
    pub fn flush(
        &mut self,
        snapshot: &Snapshot,
        out: &Outbound,
    ) -> Result<Option<Sent>, DamageError> {
        if self.paused {
            return Ok(None);
        }
        if out.depth(Priority::Grid) != 0 {
            self.stats.stalls += 1;
            return Ok(None);
        }

        let cap = self.max_frame as usize;
        let cursor = codec::cursor_of(snapshot.cursor());
        let kind = if let Some(reason) = self.keyframe {
            self.encoder.reset(self.generation, snapshot, cap)?;
            SentKind::Keyframe(reason)
        } else if !self.dirty.is_empty() {
            self.encoder
                .delta(self.generation, snapshot, &self.dirty, cap)?;
            SentKind::Delta
        } else if self.cursor == Some(cursor) {
            return Ok(None);
        } else {
            self.encoder.cursor(snapshot);
            SentKind::Cursor
        };

        // Zero-copy hand-off: the encoder's buffer is split and frozen, and
        // reclaimed for the next message once the writer drops this frame.
        let payload = self.encoder.take_payload();
        let bytes = payload.len();
        out.send(OutFrame::stream(
            self.channel,
            Priority::Grid,
            self.max_frame,
            payload,
        )?)?;

        // Accepted: only now is anything forgotten.
        match kind {
            SentKind::Keyframe(_) => {
                self.dirty.clear();
                self.keyframe = None;
                self.stats.keyframes += 1;
            }
            SentKind::Delta => {
                for row in self.encoder.covered() {
                    self.dirty.unmark(*row);
                }
                self.stats.deltas += 1;
                if !self.encoder.complete() {
                    self.stats.partials += 1;
                }
            }
            SentKind::Cursor => self.stats.cursors += 1,
        }
        self.cursor = Some(cursor);
        self.stats.bytes += bytes as u64;

        // Rows are bounded by the grid height.
        let rows = u16::try_from(self.encoder.covered().len()).unwrap_or(u16::MAX);
        Ok(Some(Sent {
            kind,
            generation: self.generation,
            bytes,
            rows,
            complete: self.encoder.complete(),
        }))
    }

    /// Record that a keyframe is owed, keeping the first reason recorded.
    ///
    /// All five reasons produce the same message; the first one is kept because
    /// it is the one that explains why the client's grid stopped being usable.
    fn need(&mut self, reason: KeyframeReason) {
        if self.keyframe.is_none() {
            self.keyframe = Some(reason);
        }
    }
}
