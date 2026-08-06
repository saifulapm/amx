//! The parser thread: the exclusive owner of one pane's terminal.
//!
//! 04 §3 is the whole specification of this file. The libghostty-vt instance,
//! its render state and its snapshot buffers live here and nowhere else, which
//! the type system already insists on ([`Terminal`] is `Send` and not `Sync`).
//! Everything anyone else wants from the terminal — a frame, a history range —
//! is a [`ParserCommand`] executed on this thread, so no reader ever reaches
//! for a lock the parser holds.
//!
//! ## Why parsing is a blocking round trip
//!
//! The pty actor's read callback runs on the pty thread, under T05's
//! `response_order` lock, and the replies it produces have to be queued before
//! any out-of-band reply handed in afterwards. The terminal that produces those
//! replies is on *this* thread, so the read callback sends the bytes here and
//! blocks until the parse is done and the replies come back. The lock is
//! therefore held for exactly as long as it would have been if the parse ran
//! inline, and the ordering guarantee is unchanged.
//!
//! The bytes travel in a [`Scratch`] buffer that is handed back with the
//! replies and reused, so a steady stream of output allocates nothing.
//!
//! This thread never takes the `response_order` lock itself: a reply produced
//! outside a parse (the in-band size report a resize can emit) travels with the
//! resize through [`PtyActorHandle::resize`], which does not take that lock.
//! Taking it here would deadlock against a read callback already waiting on us.
//!
//! ## Row identity
//!
//! Row ids, the eviction floor and invalidation are [`HistoryTracker`]'s
//! (`crate::history`), observed here once per published frame because that is
//! where the terminal is. This file holds no model of its own: it hands the
//! tracker the terminal, forwards what the tracker found, and serves history
//! ranges through it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::time::{Duration, Instant};

use amx_core::platform::WinSize;
use amx_core::{GridGeneration, InvalidationCause, RowHash, RowId, RowRange};
use amx_vt::{Effect as VtEffect, Effects, RenderState, SnapshotRef, Snapshots, Terminal};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, warn};

use super::PublishedFrame;
use super::probe::PaneProbe;
use crate::actor::{HistoryError, HistoryRows};
use crate::history::{HistoryEvent, HistoryTracker};
use crate::pty::{ChildExit, PtyActorHandle};

/// How long the parser parks when it has no frame pending.
///
/// A fallback for a wake that never arrived, exactly as the pty actor's idle
/// timeout is: every path that makes the terminal dirty sends a command.
const IDLE_TICK: Duration = Duration::from_millis(250);

/// The buffer that shuttles between the pty thread and the parser thread.
///
/// One per pane, handed over with the bytes and handed back with the replies,
/// so the hot path is two moves and no allocation.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    /// Bytes read off the pty.
    pub(super) input: Vec<u8>,
    /// What the terminal wants written back, concatenated in order.
    pub(super) replies: Vec<u8>,
}

/// What the parser thread is asked to do.
///
/// Serialized like herdr's pty actor commands: one queue, one thread, so a
/// history read and a parse can never interleave inside the terminal.
pub(super) enum ParserCommand {
    /// Feed these bytes to the terminal and hand the buffer back with the
    /// replies. The sender blocks until it comes back.
    Parse(Box<Scratch>),
    /// Resize the grid, then the pty.
    Resize {
        /// New size.
        size: WinSize,
    },
    /// Publish a frame now and answer with it.
    Snapshot(oneshot::Sender<SnapshotRef>),
    /// Read a committed range of history.
    History {
        /// The rows wanted.
        range: RowRange,
        /// Where they go.
        reply: oneshot::Sender<Result<HistoryRows, HistoryError>>,
    },
    /// Stop the thread.
    Stop,
}

/// What the parser thread tells the pane actor.
///
/// Sent on an unbounded channel on purpose: the pane actor can be awaiting a
/// pty round trip when one of these is produced, and a parser that blocked on a
/// full mailbox would deadlock against it. The rate is bounded by the frame
/// interval, not by output volume.
#[derive(Debug)]
pub(super) enum HostEvent {
    /// The application set the title.
    Title(String),
    /// The application rang the bell.
    Bell,
    /// The grid was resized and its generation bumped.
    ///
    /// Produced here rather than in the pane actor so the generation moves on
    /// the same thread that publishes frames: a bump made anywhere else could
    /// land between a frame and the generation a reader pairs it with.
    Resized {
        /// New row count.
        rows: u16,
        /// New column count.
        cols: u16,
        /// The generation the resize minted.
        generation: GridGeneration,
    },
    /// Rows were committed to history.
    Committed {
        /// The rows, in order.
        range: RowRange,
        /// Content hashes for the tail of `range` (04 §3).
        hashes: Vec<RowHash>,
    },
    /// Cached history at or beyond `from_row` is no longer valid.
    Invalidated {
        /// First invalid row.
        from_row: RowId,
        /// Why.
        cause: InvalidationCause,
    },
    /// The eviction floor advanced.
    Evicted {
        /// Oldest row still fetchable.
        oldest_row: RowId,
    },
    /// The child process ended.
    Exited(ChildExit),
}

/// The parser thread's own state. Nothing here is shared.
pub(super) struct Parser {
    terminal: Terminal,
    render: RenderState,
    snapshots: Snapshots,
    effects: Effects,
    /// Where a parsed buffer goes back to the pty thread.
    done: sync_mpsc::Sender<Box<Scratch>>,
    /// The published snapshot slot. Readers clone the `Arc` out of it and hold
    /// that; the borrow is a pointer copy, and no reader ever holds it across
    /// work of its own.
    published: watch::Sender<PublishedFrame>,
    events: mpsc::UnboundedSender<HostEvent>,
    pty: PtyActorHandle,
    probe: PaneProbe,
    frame_interval: Duration,
    /// The pane's grid generation (04 §4). This thread is the only writer:
    /// the bump happens when the resize is applied to the terminal, so every
    /// frame published after it carries the geometry the generation names.
    generation: Arc<AtomicU64>,
    /// Row ids, the eviction floor and invalidation (04 §3).
    history: HistoryTracker,
    /// What the last observation found, reused so a frame allocates nothing
    /// more than the hashes it announces.
    found: Vec<HistoryEvent>,
}

impl Parser {
    /// Assemble the parser. The terminal moves onto its thread with it.
    pub(super) fn new(parts: ParserParts) -> Self {
        let ParserParts {
            terminal,
            render,
            snapshots,
            done,
            published,
            events,
            pty,
            probe,
            frame_interval,
            size,
            generation,
        } = parts;
        Self {
            terminal,
            render,
            snapshots,
            effects: Effects::new(),
            done,
            published,
            events,
            pty,
            probe,
            frame_interval,
            generation,
            history: HistoryTracker::new(size.cols, size.rows),
            found: Vec::new(),
        }
    }

    /// Serve commands until told to stop or until every sender is gone.
    pub(super) fn run(mut self, commands: &sync_mpsc::Receiver<ParserCommand>) {
        self.probe.parser_started();
        let mut deadline: Option<Instant> = None;
        loop {
            let waited = match deadline {
                Some(at) => commands.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => commands.recv_timeout(IDLE_TICK),
            };
            match waited {
                Ok(ParserCommand::Stop) => break,
                Ok(command) => {
                    if self.handle(command) {
                        // Coalesce: everything that arrives before the deadline
                        // lands in the same frame.
                        deadline.get_or_insert_with(|| Instant::now() + self.frame_interval);
                    }
                }
                Err(sync_mpsc::RecvTimeoutError::Timeout) => {}
                Err(sync_mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if deadline.is_some_and(|at| Instant::now() >= at) {
                deadline = None;
                self.publish();
            }
        }
    }

    /// Handle one command; the answer is whether the grid changed.
    fn handle(&mut self, command: ParserCommand) -> bool {
        match command {
            ParserCommand::Parse(scratch) => self.parse(scratch),
            ParserCommand::Resize { size } => self.resize(size),
            ParserCommand::Snapshot(reply) => {
                self.publish();
                self.probe.served_snapshot();
                let _ = reply.send(self.snapshots.latest());
                false
            }
            ParserCommand::History { range, reply } => {
                let rows = self.history(range);
                self.probe.served_history();
                let _ = reply.send(rows);
                false
            }
            // Handled by the caller, which stops the loop rather than looping
            // once more to publish a frame nobody will read.
            ParserCommand::Stop => false,
        }
    }

    /// Feed one chunk of output to the terminal and hand the buffer back.
    fn parse(&mut self, mut scratch: Box<Scratch>) -> bool {
        self.effects.clear();
        self.terminal.write(&scratch.input, &mut self.effects);
        scratch.replies.clear();
        scratch.replies.extend_from_slice(self.effects.replies());
        // Back to the pty thread first: it is holding `response_order` and
        // waiting, and the events below are ours to deal with in our own time.
        let _ = self.done.send(scratch);
        self.probe.parsed();
        self.drain_effects();
        true
    }

    /// Turn the side effects of a parse into pane events.
    fn drain_effects(&mut self) {
        for index in 0..self.effects.events().len() {
            let event = match &self.effects.events()[index] {
                VtEffect::Bell => Some(HostEvent::Bell),
                VtEffect::TitleChanged => match self.terminal.title() {
                    Ok(Some(title)) => Some(HostEvent::Title(title)),
                    Ok(None) => None,
                    Err(err) => {
                        debug!(error = %err, "pane title could not be read");
                        None
                    }
                },
                // The pane's own cwd tracking and clipboard routing are not M0.
                VtEffect::PwdChanged | VtEffect::ClipboardWrite { .. } => None,
                _ => None,
            };
            if let Some(event) = event {
                self.send(event);
            }
        }
    }

    /// Resize the grid, bump the generation, then resize the pty the child
    /// sees.
    ///
    /// This order is the contract: the terminal reflows first, so the frame
    /// published after the `SIGWINCH` already describes the new geometry. The
    /// generation bump happens here and only here, exactly once per applied
    /// resize — 04 §4 makes the generation the thing a client compares
    /// against to decide a delta is uninterpretable, so bumping it twice for
    /// one resize would cost every client a keyframe. A resize the terminal
    /// refused bumps nothing.
    fn resize(&mut self, size: WinSize) -> bool {
        if let Err(err) = self.terminal.resize(size.cols, size.rows) {
            warn!(error = %err, "pane terminal resize failed");
            return false;
        }
        let generation = self.current_generation().next();
        self.generation.store(generation.get(), Ordering::Release);
        // No responses travel with the resize: libghostty-vt emits the in-band
        // size report through the write-pty callback, and amx-vt only binds
        // that callback's sink inside `Terminal::write`, so a reply produced
        // here would have nowhere to land. Noted for T06.
        if let Err(err) = self.pty.resize(size, Vec::new()) {
            warn!(error = %err, "pane pty resize failed");
        }
        self.send(HostEvent::Resized {
            rows: size.rows,
            cols: size.cols,
            generation,
        });
        // What the resize did to history — a width reflow invalidates every id,
        // a height change moves rows between history and the live grid and
        // invalidates nothing (04 §3) — is the tracker's to work out from the
        // terminal itself, at the next frame boundary.
        true
    }

    /// Copy the damaged rows into the back buffer and publish it.
    fn publish(&mut self) {
        let started = Instant::now();
        if let Err(err) = self.frame() {
            warn!(error = %err, "pane frame could not be published");
            return;
        }
        self.probe.published(started.elapsed());
        self.track_history();
    }

    fn frame(&mut self) -> amx_vt::Result<()> {
        self.render.update(&mut self.terminal)?;
        self.snapshots.publish(&mut self.render)?;
        // `send_replace` rather than `send`: a pane with no reader attached is
        // normal, and publication must not care. The generation rides in the
        // slot with the frame it describes — this thread is the only bumper,
        // so the pair is consistent by construction.
        self.published
            .send_replace((self.snapshots.latest(), self.current_generation()));
        Ok(())
    }

    fn current_generation(&self) -> GridGeneration {
        GridGeneration::from_raw(self.generation.load(Ordering::Acquire))
    }

    /// Follow the scrollback across a frame and report what moved.
    fn track_history(&mut self) {
        self.found.clear();
        self.history.observe(&mut self.terminal, &mut self.found);
        for event in self.found.drain(..) {
            let event = match event {
                HistoryEvent::Committed(committed) => HostEvent::Committed {
                    range: committed.range,
                    hashes: committed.hashes,
                },
                HistoryEvent::Invalidated { from_row, cause } => {
                    HostEvent::Invalidated { from_row, cause }
                }
                HistoryEvent::Evicted { oldest_row } => HostEvent::Evicted { oldest_row },
            };
            // Inlined rather than `self.send`: `found` is borrowed by the
            // drain, and the borrow checker is right that it would be a second
            // borrow of `self`.
            let _ = self.events.send(event);
        }
    }

    /// Read a committed range out of the scrollback.
    ///
    /// Chunked through the history module, which re-checks the range against
    /// the eviction floor per chunk and refuses what the scrollback has thrown
    /// away rather than faking it (04 §3).
    fn history(&mut self, range: RowRange) -> Result<HistoryRows, HistoryError> {
        self.history.read(&self.terminal, range)
    }

    fn send(&self, event: HostEvent) {
        // The pane actor going away is how a pane shuts down, not an error.
        let _ = self.events.send(event);
    }
}

/// Everything [`Parser::new`] needs, so the constructor stays inside the
/// argument budget.
pub(super) struct ParserParts {
    pub(super) terminal: Terminal,
    pub(super) render: RenderState,
    pub(super) snapshots: Snapshots,
    pub(super) done: sync_mpsc::Sender<Box<Scratch>>,
    pub(super) published: watch::Sender<PublishedFrame>,
    pub(super) events: mpsc::UnboundedSender<HostEvent>,
    pub(super) pty: PtyActorHandle,
    pub(super) probe: PaneProbe,
    pub(super) frame_interval: Duration,
    pub(super) size: WinSize,
    pub(super) generation: Arc<AtomicU64>,
}
