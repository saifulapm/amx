//! The pane's tokio actor: the mailbox end of a pane.
//!
//! The actor owns no terminal state at all. It routes: commands that need the
//! terminal become [`ParserCommand`]s carrying the caller's own reply channel,
//! so the parser answers the caller directly and the actor never waits on it.
//! That is not a style choice — an actor that awaited the parser while the
//! parser waited on the actor's mailbox would deadlock through the pty thread.
//!
//! Every handler returns an [`Effect`] (04 §2). The loop folds a batch of them
//! through an [`EffectSet`] and reports once per batch, so a pane under heavy
//! output produces one damage notification per frame rather than one per
//! message.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::thread::JoinHandle;

use amx_core::platform::{ProcessTree, WinSize};
use amx_core::{Bus, Effect, EffectSet, Event, GridGeneration, Level, PaneId, RowRange, Scheduled};
use amx_vt::SnapshotRef;
use bytes::Bytes;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::drive::{Drive, DriveError, Driven};
use super::mailbox::{HostEvent, ParserCommand};
use super::{ExportError, PaneExport, PublishedFrame};
use crate::actor::{CoreCommand, CoreHandle, HistoryError, HistoryRows, PaneCommand, PaneReport};
use crate::pty::{ChildExit, PtyActorHandle};

/// What one loop iteration decided.
enum Step {
    /// Fold this in and keep going.
    Effect(Effect),
    /// Nothing happened worth folding.
    Idle,
    /// Stop the actor.
    Stop,
}

/// The receiving ends the actor loop owns.
///
/// Kept out of [`Actor`] so that a `select!` arm can borrow one of them while
/// its handler borrows the actor.
pub(super) struct Mailboxes {
    pub(super) commands: mpsc::Receiver<PaneCommand>,
    pub(super) events: mpsc::UnboundedReceiver<HostEvent>,
    pub(super) frames: watch::Receiver<PublishedFrame>,
}

/// The pane actor.
pub(super) struct Actor {
    pub(super) pane: PaneId,
    pub(super) bus: Arc<Bus>,
    pub(super) core: Option<CoreHandle>,
    pub(super) pty: PtyActorHandle,
    pub(super) parser: sync_mpsc::Sender<ParserCommand>,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) process_tree: Arc<dyn ProcessTree>,
    pub(super) cancel: CancellationToken,
    pub(super) threads: Vec<JoinHandle<()>>,
    pub(super) effects: EffectSet,
    pub(super) scheduled: Scheduled,
    pub(super) stopping: bool,
}

impl Actor {
    /// Serve the mailbox until told to stop, then take the pane down with it.
    pub(super) async fn run(mut self, mut boxes: Mailboxes) {
        loop {
            let step = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Step::Stop,
                command = boxes.commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => Step::Stop,
                },
                event = boxes.events.recv() => match event {
                    Some(event) => Step::Effect(self.event(event).await),
                    None => Step::Idle,
                },
                changed = boxes.frames.changed() => match changed {
                    Ok(()) => Step::Effect(Effect::PaneDamage(self.pane)),
                    // The parser is gone, which only happens on the way down.
                    Err(_) => Step::Stop,
                },
            };
            let mut stop = !self.absorb(step);
            // Everything already queued joins the same batch, so a burst of
            // output costs one damage report rather than one per message.
            while !stop {
                let queued = match boxes.events.try_recv() {
                    Ok(event) => Step::Effect(self.event(event).await),
                    Err(_) => match boxes.commands.try_recv() {
                        Ok(command) => self.command(command).await,
                        Err(_) => break,
                    },
                };
                stop = !self.absorb(queued);
            }
            self.dispatch().await;
            if stop || self.stopping {
                break;
            }
        }
        self.shutdown().await;
    }

    /// Fold one step in; the answer is whether the loop continues.
    fn absorb(&mut self, step: Step) -> bool {
        match step {
            Step::Effect(effect) => {
                self.effects.absorb(effect);
                true
            }
            Step::Idle => true,
            Step::Stop => false,
        }
    }

    /// Turn a folded batch into the output it schedules.
    async fn dispatch(&mut self) {
        self.effects.drain_into(&mut self.scheduled);
        if self.scheduled.is_empty() {
            return;
        }
        if self.scheduled.level() >= Level::PaneDamage {
            let generation = self.generation();
            self.bus.publish(Event::PaneDamage {
                pane: self.pane,
                generation,
            });
            // `try_send`, not `send`: damage is the one report produced at
            // frame rate, and a pane that blocked on a saturated `Core` here
            // would stop serving its own mailbox — one half of a
            // Core↔PaneHost deadlock. Dropping is safe for damage alone: the
            // dirty state stays accumulated and the next frame's report
            // covers it.
            if let Some(core) = &self.core {
                let _ = core.try_send(CoreCommand::PaneReport {
                    pane: self.pane,
                    report: PaneReport::Damage { generation },
                });
            }
        }
    }

    // ------------------------------------------------------------- commands

    async fn command(&mut self, command: PaneCommand) -> Step {
        match command {
            PaneCommand::Write(bytes) => Step::Effect(self.write(bytes).await),
            PaneCommand::Seed(bytes) => Step::Effect(self.seed(bytes)),
            PaneCommand::Resize { rows, cols } => Step::Effect(self.resize(rows, cols).await),
            PaneCommand::TakeSnapshot(reply) => Step::Effect(self.snapshot(reply)),
            PaneCommand::HistoryRange { range, reply } => Step::Effect(self.history(range, reply)),
            PaneCommand::Drive { what, reply } => Step::Effect(self.drive(what, reply)),
            PaneCommand::ForegroundCwd(reply) => Step::Effect(self.foreground_cwd(reply).await),
            PaneCommand::ExportHandoff { reply } => Step::Effect(self.export(reply)),
            PaneCommand::Kill => Step::Effect(self.kill().await),
            PaneCommand::Shutdown => Step::Stop,
        }
    }

    /// Input goes straight to the pty; the grid changes only once the child
    /// answers, so this schedules nothing by itself.
    async fn write(&mut self, bytes: Bytes) -> Effect {
        if let Err(err) = self.pty.write_input(bytes).await {
            debug!(error = %err, "pane input was not queued");
        }
        Effect::Nothing
    }

    /// Restored scrollback goes to the parser thread, which owns the terminal
    /// — never to the pty, which would hand it to the child as *input*.
    fn seed(&mut self, bytes: Vec<u8>) -> Effect {
        let _ = self.parser.send(ParserCommand::Seed(bytes));
        Effect::Nothing
    }

    /// Forward a resize to the parser thread, which owns the terminal.
    ///
    /// The generation bump and the `PaneResized` event come back from the
    /// parser as [`HostEvent::Resized`] once the terminal has actually
    /// reflowed: bumping here would let a reader pair the new generation with
    /// a frame published before the resize was applied.
    async fn resize(&mut self, rows: u16, cols: u16) -> Effect {
        let _ = self.parser.send(ParserCommand::Resize {
            size: WinSize { rows, cols },
        });
        Effect::Nothing
    }

    /// A frame is a command on the parser thread, answered to the caller.
    fn snapshot(&mut self, reply: oneshot::Sender<SnapshotRef>) -> Effect {
        let _ = self.parser.send(ParserCommand::Snapshot(reply));
        Effect::Nothing
    }

    /// So is a history range (04 §3) — never a lock over the pane's state.
    fn history(
        &mut self,
        range: RowRange,
        reply: oneshot::Sender<Result<HistoryRows, HistoryError>>,
    ) -> Effect {
        if let Err(err) = self.parser.send(ParserCommand::History { range, reply })
            && let ParserCommand::History { reply, .. } = err.0
        {
            let _ = reply.send(Err(HistoryError::Terminal("pane is stopping".into())));
        }
        Effect::Nothing
    }

    /// Driven input (04 §8) is a parser-thread command for the same reason a
    /// history range is: the answer depends on state only that thread owns.
    ///
    /// Forwarded with the caller's own reply channel, so the parser answers the
    /// connection directly and this actor never waits for it.
    fn drive(&mut self, what: Drive, reply: oneshot::Sender<Result<Driven, DriveError>>) -> Effect {
        if let Err(err) = self.parser.send(ParserCommand::Drive { what, reply })
            && let ParserCommand::Drive { reply, .. } = err.0
        {
            let _ = reply.send(Err(DriveError::Gone));
        }
        Effect::Nothing
    }

    /// Freezing the pane for a handoff is a parser-thread read like the others
    /// (D-M3-4): the grid, the modes and the scrollback all live there.
    ///
    /// Forwarded with the caller's reply channel, so the parser answers the
    /// orchestrator directly and this actor never waits on a capture.
    fn export(&mut self, reply: oneshot::Sender<Result<PaneExport, ExportError>>) -> Effect {
        if let Err(err) = self.parser.send(ParserCommand::Export(reply))
            && let ParserCommand::Export(reply) = err.0
        {
            let _ = reply.send(Err(ExportError::Gone));
        }
        Effect::Nothing
    }

    /// The foreground process's cwd, or `None` when it cannot be read.
    ///
    /// Both halves are blocking syscalls on another thread's terminal, so they
    /// run on the blocking pool and are joined rather than detached.
    async fn foreground_cwd(&mut self, reply: oneshot::Sender<Option<PathBuf>>) -> Effect {
        let pty = self.pty.clone();
        let tree = Arc::clone(&self.process_tree);
        let cwd = tokio::task::spawn_blocking(move || {
            let group = pty.foreground_group().ok()?;
            tree.cwd(group).ok()
        })
        .await
        .ok()
        .flatten();
        let _ = reply.send(cwd);
        Effect::Nothing
    }

    /// Hang the child's terminal up.
    ///
    /// Releasing the pty actor drops the master, and what a closing terminal
    /// owes the session on it is `SIGHUP`. The exit arrives as a report like
    /// any other, which is what stops the actor.
    async fn kill(&mut self) -> Effect {
        let pty = self.pty.clone();
        if let Ok(Err(err)) = tokio::task::spawn_blocking(move || pty.release()).await {
            debug!(error = %err, "pane pty could not be released");
        }
        Effect::Nothing
    }

    // --------------------------------------------------------------- events

    async fn event(&mut self, event: HostEvent) -> Effect {
        match event {
            // Published and not reported: `Core` folds nothing from a title, so
            // the report would be a blocking send into a bounded mailbox for a
            // fact the bus already carries. See [`PaneReport`].
            HostEvent::Title(title) => {
                self.bus.publish(Event::PaneTitle {
                    pane: self.pane,
                    title,
                });
            }
            HostEvent::Bell => self.report(PaneReport::Bell).await,
            HostEvent::Resized {
                rows,
                cols,
                generation,
            } => {
                self.bus.publish(Event::PaneResized {
                    pane: self.pane,
                    rows,
                    cols,
                    generation,
                });
            }
            HostEvent::Committed { range, hashes } => {
                self.bus.publish(Event::HistoryCommitted {
                    pane: self.pane,
                    range,
                });
                self.report(PaneReport::Committed { range, hashes }).await;
            }
            HostEvent::Invalidated { from_row, cause } => {
                self.bus.publish(Event::HistoryInvalidated {
                    pane: self.pane,
                    from_row,
                    cause,
                });
                self.report(PaneReport::Invalidated { from_row, cause })
                    .await;
            }
            HostEvent::Evicted { oldest_row } => {
                self.bus.publish(Event::HistoryEvicted {
                    pane: self.pane,
                    oldest_row,
                });
                self.report(PaneReport::Evicted { oldest_row }).await;
            }
            HostEvent::Exited(exit) => {
                let status = match exit {
                    ChildExit::Code(status) => Some(status),
                    ChildExit::Signalled | ChildExit::Unknown => None,
                };
                self.bus.publish(Event::PaneExited {
                    pane: self.pane,
                    status,
                });
                self.report(PaneReport::Exited { status }).await;
                self.stopping = true;
            }
        }
        Effect::Nothing
    }

    // ------------------------------------------------------------- plumbing

    fn generation(&self) -> GridGeneration {
        GridGeneration::from_raw(self.generation.load(Ordering::Acquire))
    }

    /// Hand a fact to the `Core`, if there is one listening yet.
    async fn report(&self, report: PaneReport) {
        let Some(core) = &self.core else { return };
        let _ = core
            .send(CoreCommand::PaneReport {
                pane: self.pane,
                report,
            })
            .await;
    }

    /// Stop both threads and join them: nothing detached, everything joined.
    async fn shutdown(mut self) {
        let _ = self.parser.send(ParserCommand::Stop);
        self.pty.shutdown();
        let threads = std::mem::take(&mut self.threads);
        if threads.is_empty() {
            return;
        }
        let joined = tokio::task::spawn_blocking(move || {
            for thread in threads {
                let _ = thread.join();
            }
        })
        .await;
        if joined.is_err() {
            debug!("pane threads were not joined");
        }
    }
}
