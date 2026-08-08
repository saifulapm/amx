//! The `PaneHost` actor: one pty, one terminal, one published grid.
//!
//! 04 §2 gives the pane actor its scope — "PTY I/O actor thread + VT state +
//! status tracker" — and 04 §3 gives it its one hard invariant:
//!
//! > the parser I/O thread **exclusively owns the libghostty-vt instance**; VT
//! > state is never shared or swapped … At frame boundaries the parser copies
//! > damaged visible rows + cursor into a derived, double-buffered POD cell
//! > snapshot published lock-free for render … Scrollback-dependent reads —
//! > history ranges, persistence snapshots — are served as commands executed on
//! > the parser actor thread, serialized like herdr's pty actor commands.
//! > Readers therefore never contend with the parser on a pane-state mutex.
//!
//! Three pieces implement that, and the boundaries between them are where the
//! invariant lives:
//!
//! - the **pty thread** (T05) owns the master descriptor and the reply-ordering
//!   lock. Its read callback hands bytes to the parser and blocks until the
//!   replies come back, so the ordering guarantee is exactly what it was when
//!   the parse ran inline.
//! - the **parser thread** ([`parser`]) owns the terminal, the render state and
//!   the snapshot buffers. Nothing else can reach them: [`amx_vt::Terminal`] is
//!   `!Sync`, so the compiler rejects a design that shares one.
//! - the **pane actor** ([`actor`]) owns only a mailbox. A command that needs
//!   the terminal is forwarded to the parser *with the caller's reply channel*,
//!   so no reader ever queues behind the actor either.
//!
//! A reader takes [`SnapshotFeed::latest`] and gets an `Arc` to plain data. It
//! holds it for as long as it likes; the parser meanwhile keeps publishing into
//! the other half of the double buffer, and gives that half up rather than wait
//! if a reader still holds it. The only lock on that path is the one inside the
//! `watch` slot, held for the length of a pointer clone by both sides and never
//! across work — it is not the pane-state mutex 04 §3 rules out, and
//! `no_reader_ever_blocks_the_parser_on_a_pane_state_mutex` is the assertion
//! that it does not behave like one.

mod actor;
mod config;
pub mod drive;
mod export;
mod feed;
pub mod keys;
mod mailbox;
mod parser;
mod probe;

use std::os::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc as sync_mpsc;

use amx_core::platform::PtySession;
use amx_core::{GridGeneration, PaneId};
use amx_vt::{RenderState, Snapshots, Terminal, TerminalOptions};
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use self::actor::{Actor, Mailboxes};
use self::config::{pty_config, short};
use self::parser::{Parser, ParserParts};
use crate::actor::{PaneCommand, PaneHandle};
use crate::pty::{PtyActor, PtyActorError, PtyActorHandle};

pub use self::config::PaneHostConfig;
pub use self::drive::{Drive, DriveError, Driven};
pub use self::export::{ExportError, PaneExport, PaneResume};
pub(crate) use self::feed::PublishedFrame;
pub use self::feed::SnapshotFeed;
pub use self::keys::{KeyParseError, KeyStroke};
pub use self::probe::PaneProbe;

/// A running pane: its mailbox, its published frames, and its task.
///
/// The task handle is returned rather than detached — 04 §2 wants nothing
/// detached and everything joined, and the supervisor is the one that knows
/// which `JoinSet` this belongs in. Joining the task also joins the pane's two
/// threads, which it stops on its way out.
#[derive(Debug)]
pub struct PaneHost {
    pane: PaneId,
    handle: PaneHandle,
    frames: SnapshotFeed,
    probe: PaneProbe,
    /// The pty actor, kept for [`PaneHost::kill`]: hanging a pane up must not
    /// depend on mailbox capacity, so it bypasses the command mailbox.
    pty: PtyActorHandle,
    task: JoinHandle<()>,
}

impl PaneHost {
    /// Start a pane host over an already-opened pty session.
    ///
    /// Must be called from inside a tokio runtime: the actor is spawned onto
    /// it, and its handle comes back in the returned [`PaneHost`].
    ///
    /// # Errors
    ///
    /// [`PaneHostError::Terminal`] if libghostty-vt cannot allocate a terminal
    /// or a render state, [`PaneHostError::Pty`] if the pty actor's wake pipe
    /// or thread cannot be created, and [`PaneHostError::Io`] if the parser
    /// thread cannot be spawned.
    pub fn spawn<S>(config: PaneHostConfig, session: S) -> Result<Self, PaneHostError>
    where
        S: PtySession + AsFd + Send + 'static,
    {
        let PaneHostConfig {
            pane,
            bus,
            core,
            size,
            max_scrollback,
            frame_interval,
            mailbox,
            process_tree,
            cancel,
            probe,
            seed,
        } = config;

        let terminal = Terminal::new(TerminalOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback,
        })?;
        let render = RenderState::new()?;
        // Both counters continue where the exporter left them. The frame
        // counter is a reader's "have I seen this?"; the grid generation is 04
        // §4's delta contract. Restarting either at zero on a successor is what
        // `Snapshots::new_at` and this seeding exist to prevent.
        let snapshots = match &seed {
            Some(seed) => Snapshots::new_at(size.cols, size.rows, seed.frame),
            None => Snapshots::new(size.cols, size.rows),
        };
        let first = seed
            .as_ref()
            .map_or(GridGeneration::FIRST, |seed| seed.generation);

        let child = session.child();
        let generation = Arc::new(AtomicU64::new(first.get()));
        let (published, frames) = watch::channel((snapshots.latest(), first));
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (commands_tx, commands_rx) = mpsc::channel(mailbox.max(1));
        let (parser_tx, parser_rx) = sync_mpsc::channel();
        let (done_tx, done_rx) = sync_mpsc::channel();

        let pty_config = pty_config(session, pane, parser_tx.clone(), done_rx, events_tx.clone());
        let (pty, pty_thread) = PtyActor::spawn(pty_config)?;

        let parser = Parser::new(ParserParts {
            pane,
            child,
            terminal,
            render,
            snapshots,
            done: done_tx,
            published,
            events: events_tx,
            pty: pty.clone(),
            probe: probe.clone(),
            frame_interval,
            size,
            generation: Arc::clone(&generation),
            seed,
        });
        let parser_thread = std::thread::Builder::new()
            .name(format!("amx-vt-{}", short(pane)))
            .spawn(move || parser.run(&parser_rx))?;

        let actor = Actor {
            pane,
            bus,
            core,
            pty,
            parser: parser_tx,
            generation: Arc::clone(&generation),
            process_tree,
            cancel,
            threads: vec![pty_thread, parser_thread],
            effects: amx_core::EffectSet::with_capacity(1),
            scheduled: amx_core::Scheduled::new(),
            stopping: false,
        };
        let pty_handle = actor.pty.clone();
        let task = tokio::spawn(actor.run(Mailboxes {
            commands: commands_rx,
            events: events_rx,
            frames: frames.clone(),
        }));

        Ok(Self {
            pane,
            handle: PaneHandle::new(commands_tx),
            frames: SnapshotFeed { frames, generation },
            probe,
            pty: pty_handle,
            task,
        })
    }

    /// Which pane this is.
    #[must_use]
    pub fn pane(&self) -> PaneId {
        self.pane
    }

    /// The pane's mailbox.
    #[must_use]
    pub fn handle(&self) -> &PaneHandle {
        &self.handle
    }

    /// A reader's view of the published frames.
    #[must_use]
    pub fn frames(&self) -> SnapshotFeed {
        self.frames.clone()
    }

    /// The pane's instrumentation counters.
    #[must_use]
    pub fn probe(&self) -> &PaneProbe {
        &self.probe
    }

    /// Hang the pane's terminal up, without going through the mailbox.
    ///
    /// This is the un-droppable half of a close or kill: it cannot be lost to
    /// a full command mailbox, and it unblocks an actor stuck writing to a
    /// stuffed pty. The pty actor stops, the master closes (`SIGHUP` to the
    /// child), and the exit flows back through the ordinary
    /// [`PaneReport::Exited`](crate::actor::PaneReport::Exited) path.
    pub fn kill(&self) {
        self.pty.shutdown();
    }

    /// Hold the pane's terminal still, once everything queued has been written.
    ///
    /// The state a handoff captures in (D-M3-3): the terminal is still owned
    /// and still open, but nothing is read from it or written to it, so it
    /// cannot move under the process taking it over. Input queued afterwards
    /// stays queued and is written by [`PaneHost::resume`] — a quiesce that
    /// dropped a keystroke would be a data-loss bug wearing a state machine's
    /// clothes.
    ///
    /// Bypasses the command mailbox like [`PaneHost::kill`], and blocks for as
    /// long as the drain takes: call it off the runtime's threads.
    ///
    /// # Errors
    ///
    /// Whatever the pty actor answers, including
    /// [`PtyActorError::TimedOut`] for a drain that never finished.
    pub fn quiesce(&self) -> Result<(), PtyActorError> {
        self.pty.quiesce()
    }

    /// Undo a [`PaneHost::quiesce`], writing whatever queued behind it.
    ///
    /// # Errors
    ///
    /// As [`PaneHost::quiesce`].
    pub fn resume(&self) -> Result<(), PtyActorError> {
        self.pty.resume()
    }

    /// The pty actor's own mailbox, for the export path's freeze and thaw.
    ///
    /// [`quiesce`](Self::quiesce) and [`resume`](Self::resume) both block, so
    /// the handoff runs them on the blocking pool — and a `PaneHost` cannot go
    /// there, because `Core` is still serving with it. The handle can: cloning
    /// it gives another way to reach the same actor, not another actor. W06's
    /// orchestrator is the only caller, and it holds one per frozen pane so
    /// that an abort can unfreeze the session without queueing behind `Core`.
    #[must_use]
    pub fn pty(&self) -> &PtyActorHandle {
        &self.pty
    }

    /// Ask the pane to stop and wait for it, threads included.
    ///
    /// # Errors
    ///
    /// Propagates a panic in the pane's task.
    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        let _ = self.handle.send(PaneCommand::Shutdown).await;
        self.task.await
    }

    /// The task handle, for a supervisor that owns the `JoinSet`.
    #[must_use]
    pub fn into_task(self) -> JoinHandle<()> {
        self.task
    }

    /// The one capability a caller keeps after giving the pane away.
    ///
    /// The import assembly's need, and nobody else's (`docs/09-m3-plan.md` §3
    /// step 14): an inherited pane is quiesced *before* it is handed to `Core`,
    /// and the thing that un-quiesces it happens minutes of protocol later,
    /// after `Core` already owns the host. Narrow on purpose — it can resume a
    /// pane and do nothing else — because the reason `PaneHost` keeps its pty
    /// handle private is that quiescing, releasing and duplicating a terminal
    /// belong to whoever owns the pane.
    #[must_use]
    pub fn resumer(&self) -> PaneResume {
        PaneResume::new(self.pane, self.pty.clone())
    }
}

/// A pane host could not be started.
#[derive(Debug, Error)]
pub enum PaneHostError {
    /// libghostty-vt could not give us a terminal or a render state.
    #[error("terminal: {0}")]
    Terminal(#[from] amx_vt::Error),
    /// The pty actor could not be started.
    #[error(transparent)]
    Pty(#[from] PtyActorError),
    /// The parser thread could not be spawned.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
