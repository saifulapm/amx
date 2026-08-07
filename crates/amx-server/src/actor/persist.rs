//! The `Persist` actor's mailbox: what anything says to persistence.
//!
//! The fourth actor of 04 §2's table — "debounced snapshot capture, fsynced
//! writes, restore". Almost nothing talks to it: dirtiness is *observed* off
//! the event bus rather than pushed, so `Core` carries no "remember to tell
//! persistence" call sites and forgetting to publish is already impossible.
//! What is left is this mailbox, and it has exactly two messages.
//!
//! # Task ownership
//!
//! U01 froze the mailbox and the handle; **U05** grows the actor loop here
//! (`docs/07-m1-plan.md` §2 and §4): the bus subscription, the 500 ms-quiet /
//! 5 s-cap debounce, the capture request through the ordinary [`CoreHandle`],
//! `spawn_blocking` writes, sidecar dumps, and the shutdown drain.

use amx_core::PaneId;
use tokio::sync::{mpsc, oneshot};

use super::{MailboxError, PaneHandle};
use crate::persist::{PersistError, Snapshot};

/// What the `Persist` actor is asked to do.
#[derive(Debug)]
pub enum PersistCommand {
    /// A capture pushed in, rather than requested out.
    ///
    /// Sent exactly once in normal operation: by `Core` on its shutdown path,
    /// built from stored state only, because the panes are about to be killed
    /// and probing them would mean waiting on actors that are already going
    /// down. Boxed because the mailbox's size is set by its largest variant
    /// and a whole session's snapshot is not a message size worth paying on
    /// every slot.
    Snapshot(Box<Snapshot>),
    /// Save now, and say when it is on disk.
    ///
    /// The deterministic handle the crash suite and the actor's own tests use
    /// instead of waiting out the debounce — a test that slept for the quiet
    /// window would be pacing against the wall clock, which the rig forbids.
    Flush {
        /// Where the outcome of the write goes.
        reply: oneshot::Sender<Result<(), PersistError>>,
    },
}

/// One capture: what `Core` hands back for `SessionCall::Capture`.
///
/// Capture happens on `Core` (it owns the state), writing happens on `Persist`,
/// and fsync happens on the blocking pool — this type is the first of those
/// two seams. The pane handles ride along so a sidecar dump can read history
/// through `PaneCommand::HistoryRange`, which executes on the parser thread
/// (04 §3); they are absent when sidecars are off, so the dump path costs
/// nothing when nobody opted in.
#[derive(Debug)]
pub struct Capture {
    /// The session, as it will be written.
    pub snapshot: Box<Snapshot>,
    /// A handle per live pane, for sidecar dumps. Empty when history is off.
    pub panes: Vec<(PaneId, PaneHandle)>,
}

impl Capture {
    /// A capture of `snapshot` with no pane handles.
    #[must_use]
    pub fn without_sidecars(snapshot: Snapshot) -> Self {
        Self {
            snapshot: Box::new(snapshot),
            panes: Vec::new(),
        }
    }
}

/// A handle on the `Persist` actor's mailbox.
///
/// Shaped like [`CoreHandle`](super::CoreHandle) on purpose: the same bounded
/// mailbox, the same two send modes, the same `Gone` when the actor has
/// stopped.
#[derive(Clone, Debug)]
pub struct PersistHandle {
    tx: mpsc::Sender<PersistCommand>,
}

impl PersistHandle {
    /// Wrap a mailbox sender.
    #[must_use]
    pub fn new(tx: mpsc::Sender<PersistCommand>) -> Self {
        Self { tx }
    }

    /// Send a command, waiting for mailbox capacity.
    pub async fn send(&self, command: PersistCommand) -> Result<(), MailboxError> {
        self.tx.send(command).await.map_err(|_| MailboxError::Gone)
    }

    /// Send a command without waiting for mailbox capacity.
    ///
    /// This is what `Core`'s shutdown break uses for its final capture push:
    /// blocking there would let a full persistence mailbox wedge `Core`'s own
    /// drain, and no task on the shutdown path may wait on a sibling
    /// (R-M1-2). A dropped final capture costs the last few seconds of
    /// changes, which the debounced save has already bounded.
    pub fn try_send(&self, command: PersistCommand) -> Result<(), MailboxError> {
        self.tx.try_send(command).map_err(|_| MailboxError::Gone)
    }

    /// Force a save and wait for it to reach the disk.
    ///
    /// # Errors
    ///
    /// [`MailboxError::Gone`] if the actor has stopped, [`MailboxError::NoReply`]
    /// if it stopped mid-write. The write's own failure comes back inside the
    /// `Ok`, since a refused save is an answer, not a lost message.
    pub async fn flush(&self) -> Result<Result<(), PersistError>, MailboxError> {
        let (tx, rx) = oneshot::channel();
        self.send(PersistCommand::Flush { reply: tx }).await?;
        rx.await.map_err(|_| MailboxError::NoReply)
    }

    /// Whether the actor is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}
