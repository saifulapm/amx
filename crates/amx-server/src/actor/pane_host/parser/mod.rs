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
//!
//! ## Driven input
//!
//! `pane.send_text`/`send_keys`/`run` are commands here too, for the reasons
//! [`super::drive`] states: the encoding and the bracketing are questions only
//! this terminal can answer, and queueing the bytes from this thread is what
//! keeps them ordered against query replies.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc as sync_mpsc;
use std::time::{Duration, Instant};

use amx_core::platform::{ProcessId, WinSize};
use amx_core::{PaneId, RowId, RowRange};
use amx_vt::{Effects, RenderState, Snapshots, Terminal};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::warn;

use super::PublishedFrame;
use super::drive::Driver;
use super::mailbox::{HostEvent, ParserCommand, Scratch};
use super::probe::PaneProbe;
use crate::actor::{HistoryError, HistoryRows};
use crate::handoff::manifest::PaneSeed;
use crate::history::{CHUNK_ROWS, HistoryChunks, HistoryEvent, HistoryTracker};
use crate::pty::PtyActorHandle;

/// How long the parser parks when it has no frame pending.
///
/// A fallback for a wake that never arrived, exactly as the pty actor's idle
/// timeout is: every path that makes the terminal dirty sends a command.
const IDLE_TICK: Duration = Duration::from_millis(250);

/// How many line feeds one adoption will spend pushing replayed rows off the
/// top of the screen.
///
/// The cursor starts somewhere inside the grid and every row of it may hold
/// replayed text, so two grid heights is the ceiling and the loop below stops
/// the moment the scrollback has swallowed what was carried. A bound rather
/// than a trust: a terminal that refused to scroll would otherwise spin here.
const ADOPT_FEED_LIMIT: u32 = 2;

/// The parser thread's own state. Nothing here is shared.
pub(super) struct Parser {
    pane: PaneId,
    child: ProcessId,
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
    /// History transfers in progress, served one chunk per loop turn.
    transfers: VecDeque<Transfer>,
    /// The encoder and buffer driven input is built with (04 §8), idle until
    /// something drives this pane.
    driver: Driver,
}

mod frames;

/// One chunked history read in flight, its reply held until the range is done.
///
/// Held as parser state rather than drained inside one command: the pty thread
/// blocks in `done.recv()` while a parse waits its turn, so a transfer served
/// whole would stall the read callback — and the published frame — for the
/// whole range. One chunk per loop turn (~a quarter millisecond, [`CHUNK_ROWS`]
/// at the measured ~3 µs a row) is the yield point 04 §4's chunking promises.
struct Transfer {
    range: RowRange,
    chunks: HistoryChunks,
    rows: Vec<u8>,
    reply: oneshot::Sender<Result<HistoryRows, HistoryError>>,
}

impl Parser {
    /// Assemble the parser. The terminal moves onto its thread with it.
    ///
    /// A pane rebuilt from a manifest is seeded here, before the thread is even
    /// spawned, so the first frame anyone can publish already carries the
    /// screen the exporter froze rather than a blank one.
    pub(super) fn new(parts: ParserParts) -> Self {
        let ParserParts {
            pane,
            child,
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
            seed,
        } = parts;
        let mut parser = Self {
            pane,
            child,
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
            transfers: VecDeque::new(),
            driver: Driver::default(),
        };
        if let Some(seed) = seed {
            parser.adopt(&seed);
        }
        parser
    }

    /// Rebuild an inherited pane from its manifest entry: rows, grid, modes.
    ///
    /// The order is D-M3-4's, and the step between the first two is the one
    /// that cannot be expressed as bytes. Replaying the rows leaves the newest
    /// of them sitting on the screen, where the grid paint is about to
    /// overwrite them — so they are fed off the top first, until the scrollback
    /// holds everything the manifest carried and the screen is blank. That
    /// blank screen is also what lets the paint skip cursor-addressed columns
    /// it has nothing to say about; `handoff::grid` has why clearing one
    /// instead would be worse.
    ///
    /// The floor is *derived*, not taken from the manifest: what a resumed
    /// tracker must get right is the head, because that is where the next
    /// committed row id comes from, and the head is the floor plus whatever
    /// actually landed in this terminal's scrollback. Rows the replay could not
    /// fit are therefore below the floor — evicted, in the ordinary sense the
    /// eviction floor already means, never renumbered.
    fn adopt(&mut self, seed: &PaneSeed) {
        self.write_quietly(&seed.rows);
        let mut fed = 0;
        while self.scrollback() < seed.carried && fed < ADOPT_FEED_LIMIT * u32::from(seed.size.rows)
        {
            self.write_quietly(b"\n");
            fed += 1;
        }
        let landed = self.scrollback().min(seed.carried);
        let floor = RowId::from_raw(seed.head.get().saturating_sub(landed));
        self.history = HistoryTracker::resume(seed.size.cols, seed.size.rows, seed.head, floor);
        self.write_quietly(&seed.screen);
        self.write_quietly(&seed.modes);
        // Publish rather than wait for a frame boundary: an inherited pane may
        // produce no output at all for minutes, and the first reader must not
        // see an empty grid until it does. `track_history` is deliberately not
        // called — the tracker's adopting first look belongs to the first
        // ordinary observation, with the terminal already whole.
        if let Err(err) = self.frame() {
            warn!(error = %err, "inherited pane frame could not be published");
        }
    }

    /// Rows of scrollback the terminal is holding, or zero if it will not say.
    fn scrollback(&self) -> u64 {
        self.terminal.scrollback_rows().unwrap_or(0) as u64
    }

    /// Write bytes that came from a manifest, answering nobody.
    ///
    /// The same rule [`Parser::seed`] follows: a reply belongs to the program
    /// that provoked it, and these bytes were provoked by an upgrade.
    fn write_quietly(&mut self, bytes: &[u8]) {
        self.effects.clear();
        self.terminal.write(bytes, &mut self.effects);
        self.effects.clear();
    }

    /// Serve commands until told to stop or until every sender is gone.
    pub(super) fn run(mut self, commands: &sync_mpsc::Receiver<ParserCommand>) {
        self.probe.parser_started();
        let mut deadline: Option<Instant> = None;
        loop {
            let waited = if self.transfers.is_empty() {
                match deadline {
                    Some(at) => commands.recv_timeout(at.saturating_duration_since(Instant::now())),
                    None => commands.recv_timeout(IDLE_TICK),
                }
            } else {
                // A transfer is in progress: take whatever has already queued
                // without blocking, so a parse or a publish waits for at most
                // one chunk, and the chunk below is served either way.
                match commands.try_recv() {
                    Ok(command) => Ok(command),
                    Err(sync_mpsc::TryRecvError::Empty) => {
                        Err(sync_mpsc::RecvTimeoutError::Timeout)
                    }
                    Err(sync_mpsc::TryRecvError::Disconnected) => {
                        Err(sync_mpsc::RecvTimeoutError::Disconnected)
                    }
                }
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
            self.serve_chunk();
        }
    }

    /// Handle one command; the answer is whether the grid changed.
    fn handle(&mut self, command: ParserCommand) -> bool {
        match command {
            ParserCommand::Parse(scratch) => self.parse(scratch),
            ParserCommand::Seed(bytes) => self.seed(&bytes),
            ParserCommand::Resize { size } => self.resize(size),
            ParserCommand::Snapshot(reply) => {
                self.publish();
                self.probe.served_snapshot();
                let _ = reply.send(self.snapshots.latest());
                false
            }
            ParserCommand::History { range, reply } => {
                // Chunked through the history module, which re-checks the
                // range against the eviction floor per chunk and refuses what
                // the scrollback has thrown away rather than faking it (04 §3).
                match self.history.chunked(range, CHUNK_ROWS) {
                    Ok(chunks) => self.transfers.push_back(Transfer {
                        range,
                        chunks,
                        rows: Vec::new(),
                        reply,
                    }),
                    Err(err) => {
                        self.probe.served_history();
                        let _ = reply.send(Err(err));
                    }
                }
                false
            }
            ParserCommand::Export(reply) => {
                let _ = reply.send(self.export());
                false
            }
            ParserCommand::Drive { what, reply } => {
                // The write is queued from this thread, which is what orders it
                // behind the replies of every parse already served (04 §3), and
                // the grid does not change until the child answers — so this
                // schedules no frame of its own.
                let _ = reply.send(self.driver.perform(what, &self.terminal, &self.pty));
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

    /// Write restored scrollback into the terminal.
    ///
    /// Whatever the terminal wants to reply is dropped rather than queued: a
    /// reply belongs to the program that provoked it, and these bytes came off
    /// disk. Their side effects are dropped for the same reason — a bell saved
    /// yesterday must not ring today. Everything else about them is ordinary
    /// output, so the frame this dirties is published like any other.
    fn seed(&mut self, bytes: &[u8]) -> bool {
        self.write_quietly(bytes);
        true
    }

    fn send(&self, event: HostEvent) {
        // The pane actor going away is how a pane shuts down, not an error.
        let _ = self.events.send(event);
    }
}

/// Everything [`Parser::new`] needs, so the constructor stays inside the
/// argument budget.
pub(super) struct ParserParts {
    pub(super) pane: PaneId,
    pub(super) child: ProcessId,
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
    /// What an inherited pane is rebuilt from, absent for a fresh one.
    pub(super) seed: Option<PaneSeed>,
}
