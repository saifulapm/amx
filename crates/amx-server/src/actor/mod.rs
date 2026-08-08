//! Mailbox types: what the actors say to each other.
//!
//! Two directions and two actors: the `Core` sends [`PaneCommand`] to a
//! `PaneHost` and receives [`PaneReport`] back; connection tasks send
//! [`CoreCommand`] to the `Core`. Every command that needs an answer carries
//! its own `oneshot` sender, so there is no correlation table to keep in sync
//! and no reply that can arrive for a request nobody is waiting on.
//!
//! The vocabulary itself lives in two siblings — [`panes`] for the `Core` ↔
//! `PaneHost` direction, [`calls`] for what a connection asks of the `Core` —
//! and both are re-exported here, so the split W03 made for the module budget
//! (`docs/09-m3-plan.md` R-M3-7) is invisible to every caller. What stays in
//! this file is the plumbing the two share: the reply alias, the two mailbox
//! handles, and the one error a closed mailbox produces.
//!
//! The other two actors keep their vocabulary in their own modules and
//! re-export it here: [`persist`] for the snapshot mailbox, and [`agent`] for
//! `AgentHub`'s — its two directions, its handle, and the [`StatusView`] wait
//! predicates read live state from. The hub's loop lives in [`agent_hub`].

pub mod pane_host;

pub mod agent;
pub mod agent_hub;
pub mod calls;
pub mod core;
pub mod gateway;
pub mod panes;
pub mod persist;

use amx_proto::rpc::RpcError;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

pub use agent::{
    AGENT_MAILBOX, AgentCall, AgentCommand, AgentHandle, SpawnedIdentity, StatusUpdate, StatusView,
};
pub use calls::{
    ClientCall, CoreCommand, PaneCall, PaneWiring, SessionCall, StreamCall, WorkspaceCall,
};
pub use pane_host::{
    Drive, DriveError, Driven, ExportError, KeyParseError, KeyStroke, PaneExport, PaneHost,
    PaneHostConfig, PaneHostError, PaneProbe, SnapshotFeed,
};
pub use panes::{HistoryError, HistoryRows, PaneCommand, PaneReport};
pub use persist::{Capture, PersistCommand, PersistHandle};

/// A reply channel for a command that answers.
pub type Reply<T> = oneshot::Sender<Result<T, RpcError>>;

/// A handle on one pane actor's mailbox.
#[derive(Clone, Debug)]
pub struct PaneHandle {
    tx: mpsc::Sender<PaneCommand>,
}

impl PaneHandle {
    /// Wrap a mailbox sender.
    #[must_use]
    pub fn new(tx: mpsc::Sender<PaneCommand>) -> Self {
        Self { tx }
    }

    /// Send a command, waiting for mailbox capacity.
    ///
    /// Bounded mailboxes are the backpressure: a pane that cannot keep up slows
    /// its sender rather than growing an unbounded queue behind it.
    pub async fn send(&self, command: PaneCommand) -> Result<(), MailboxError> {
        self.tx.send(command).await.map_err(|_| MailboxError::Gone)
    }

    /// Send a command without waiting for mailbox capacity.
    ///
    /// For callers that cannot `.await` — `Core::absorb` folds a batch
    /// synchronously (04 §2) — and for which a full mailbox is not worth
    /// blocking over: a close or a kill is rare next to the traffic that would
    /// fill 256 slots, so losing one to backpressure is an acceptable and
    /// logged trade rather than a reason to make the whole fold async.
    pub fn try_send(&self, command: PaneCommand) -> Result<(), MailboxError> {
        self.tx.try_send(command).map_err(|_| MailboxError::Gone)
    }

    /// Whether the actor is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A handle on the `Core` actor's mailbox.
#[derive(Clone, Debug)]
pub struct CoreHandle {
    tx: mpsc::Sender<CoreCommand>,
}

impl CoreHandle {
    /// Wrap a mailbox sender.
    #[must_use]
    pub fn new(tx: mpsc::Sender<CoreCommand>) -> Self {
        Self { tx }
    }

    /// Send a command, waiting for mailbox capacity.
    pub async fn send(&self, command: CoreCommand) -> Result<(), MailboxError> {
        self.tx.send(command).await.map_err(|_| MailboxError::Gone)
    }

    /// Send a command without waiting for mailbox capacity.
    ///
    /// For the reports a pane produces at frame rate: a pane actor that
    /// waited on a saturated `Core` here would stop serving its own mailbox,
    /// which is one half of a `Core`↔`PaneHost` deadlock. Callers use this
    /// only for facts that are safe to drop and re-derive (damage), never for
    /// one-shot facts like an exit.
    pub fn try_send(&self, command: CoreCommand) -> Result<(), MailboxError> {
        self.tx.try_send(command).map_err(|_| MailboxError::Gone)
    }

    /// Whether the actor is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A mailbox could not be used.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum MailboxError {
    /// The actor has stopped and its mailbox is closed.
    #[error("actor is gone")]
    Gone,
    /// The actor stopped before answering.
    #[error("actor dropped the reply channel")]
    NoReply,
}
