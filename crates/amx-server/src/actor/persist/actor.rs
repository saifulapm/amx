//! The actor loop: observe dirtiness, debounce it, capture, write.
//!
//! `docs/07-m1-plan.md` §2 in code. Four rules carry it, and each one is a
//! reaction to something that went wrong somewhere else:
//!
//! - **Dirtiness is observed, not pushed.** The actor filters the bus to the
//!   structural events — pane and workspace lifecycle, layout, labels, focus —
//!   and treats a [`Delivery::Gap`] as dirty without re-reading anything, since
//!   a gap can only hide transitions and the capture that follows reads current
//!   state anyway. Nothing anywhere has to remember to notify persistence.
//! - **The debounce has a ceiling.** A save fires after [`QUIET_WINDOW`] of
//!   quiet *or* [`STALENESS_CAP`] after the first unsaved change, whichever
//!   comes first. herdr's trailing-only window (`SESSION_SAVE_DEBOUNCE`) never
//!   fires at all while a session keeps changing; the cap is what makes
//!   staleness bounded instead of merely usually-small.
//! - **Capture happens on `Core`, writing happens here, fsync happens on the
//!   blocking pool.** The capture request goes through the ordinary
//!   [`CoreHandle`] — no back door, the same rule the connection path follows —
//!   and the bytes go to `spawn_blocking`, because `F_FULLFSYNC` on darwin
//!   costs milliseconds and no async worker may spend them. The actor is
//!   sequential, so two saves cannot overlap by construction rather than by
//!   flag.
//! - **Shutdown is receive-only.** After cancellation the actor arms no timer
//!   and sends nothing to any sibling: it drains its own mailbox until it
//!   closes (that is where `Core`'s final capture arrives), writes at most
//!   once, and returns. There is a rare, undiagnosed wedge in the runtime's
//!   `JoinSet` drain (R-M1-2), and a draining task that awaits another draining
//!   task is exactly the shape that could feed it.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use amx_core::{Config, Ctx, Delivery, Event, Subscription};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use super::sidecars::Sidecars;
use super::{Capture, PersistCommand};
use crate::actor::{CoreCommand, CoreHandle, MailboxError, SessionCall};
use crate::persist::io::{SyncAll, Syncs, write_atomic};
use crate::persist::snapshot::encode;
use crate::persist::{PersistError, SNAPSHOT_NAME, Snapshot, sidecar};

/// How long the session must be quiet before a debounced save fires.
///
/// The common path: structural change comes in bursts (a split, a focus move,
/// a layout settle) and half a second of quiet means the burst is over.
pub const QUIET_WINDOW: Duration = Duration::from_millis(500);

/// How long after the *first* unsaved change a save fires regardless.
///
/// The ceiling the quiet window alone does not have. A session that keeps
/// changing forever still saves every five seconds, which is what bounds the
/// loss a `kill -9` can cost (D-M1-7). Not shrunk casually: darwin's
/// `F_FULLFSYNC` is drastically slower than a Linux `fsync` (R-M1-6).
pub const STALENESS_CAP: Duration = Duration::from_secs(5);

/// What one run of the actor did.
///
/// Returned rather than logged: "did that save actually happen?" is the
/// question every persistence test asks, and counting writes at the seam is how
/// it gets answered without reaching into the filesystem for a timestamp.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PersistReport {
    /// Snapshots written to disk.
    pub saves: u64,
    /// Saves that found the session unchanged since the last write and wrote
    /// nothing — a debounce that fired on a change that cancelled itself out.
    pub unchanged: u64,
    /// Scrollback sidecars dumped.
    pub dumps: u64,
    /// Times `[persist] history` was turned off and the sidecars removed.
    pub wipes: u64,
}

/// When the pending change is due to be written.
///
/// Both deadlines are absolute and set at most once per dirty span: `quiet`
/// moves out with every new change, `cap` never does. The save fires at
/// whichever arrives first.
#[derive(Clone, Copy, Debug)]
struct Due {
    quiet: Instant,
    cap: Instant,
}

impl Due {
    /// The deadline the timer is armed to.
    fn at(self) -> Instant {
        self.quiet.min(self.cap)
    }
}

/// What one turn of the loop decided.
///
/// The select assigns one of these and the handler runs after it, rather than
/// inside a branch: a branch body cannot borrow the actor mutably while another
/// branch's future still borrows it.
enum Step {
    /// A mailbox message arrived.
    Command(PersistCommand),
    /// The bus delivered something.
    Delivery(Delivery),
    /// The bus is gone; stop watching it.
    BusClosed,
    /// The configuration changed.
    Configured,
    /// Nobody will publish configuration again; stop watching it.
    ConfigClosed,
    /// The debounce fired.
    Save,
    /// Cancelled, or the mailbox closed.
    Stop,
}

/// The `Persist` actor.
///
/// One per session, built before the runtime spawns it and consumed by
/// [`Persist::run`]. It holds no session state of its own — the snapshot it
/// writes is whatever `Core` hands back — beyond the bookkeeping that decides
/// *when* to write and *which* scrollback has moved since last time.
pub struct Persist {
    ctx: Ctx,
    core: CoreHandle,
    config: watch::Receiver<Config>,
    /// The sync seam every write goes through. Behind an `Arc` because each
    /// write hands it to the blocking pool, and dynamically dispatched so the
    /// actor's type does not grow a parameter every caller would have to name.
    syncs: Arc<dyn Syncs + Send + Sync>,
    /// When the pending change is due, or `None` when nothing is unsaved.
    due: Option<Due>,
    /// A capture that arrived instead of being asked for — `Core`'s final push.
    pending: Option<Capture>,
    /// The snapshot last written, so an unchanged session is not rewritten.
    last: Option<Snapshot>,
    /// Which pane's scrollback has moved since it was last dumped.
    sidecars: Sidecars,
    /// Whether `[persist] history` was on the last time it was looked at, so a
    /// reload can tell which way the toggle moved. Only the falling edge has
    /// work to do, and it has to happen exactly once per fall.
    history: bool,
    report: PersistReport,
}

impl fmt::Debug for Persist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Persist")
            .field("session", &self.ctx.session)
            .field("state_dir", &self.ctx.state_dir)
            .field("due", &self.due)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl Persist {
    /// A `Persist` for the session `ctx` names, saving into its state
    /// directory and capturing through `core`.
    ///
    /// `config` is watched rather than read once: `[persist] history` is a hot
    /// toggle, so every save asks the receiver what it is now.
    #[must_use]
    pub fn new(ctx: Ctx, core: CoreHandle, config: watch::Receiver<Config>) -> Self {
        let history = config.borrow().persist.history;
        Self {
            ctx,
            core,
            config,
            history,
            syncs: Arc::new(SyncAll),
            due: None,
            pending: None,
            last: None,
            sidecars: Sidecars::default(),
            report: PersistReport::default(),
        }
    }

    /// Write through `syncs` instead of `File::sync_all`.
    ///
    /// The seam U02 built for ordering assertions, reachable from here so a
    /// test can also watch *when* the actor writes and on which thread — the
    /// claim that fsync never runs on an async worker is only checkable if
    /// something can stand in the fsync's place.
    #[must_use]
    pub fn with_syncs(mut self, syncs: Arc<dyn Syncs + Send + Sync>) -> Self {
        self.syncs = syncs;
        self
    }

    /// Run the actor until cancellation or a closed mailbox, then drain.
    ///
    /// `events` is a live [`Subscription`]; the caller subscribes so that the
    /// subscription's position is the caller's to choose (restore takes one at
    /// the sequence it applied the snapshot at).
    pub async fn run(
        mut self,
        mut mailbox: mpsc::Receiver<PersistCommand>,
        mut events: Subscription,
    ) -> PersistReport {
        let cancel = self.ctx.cancel.clone();
        let mut watching = true;
        let mut configured = true;
        loop {
            let due = self.due.map(Due::at);
            // Deliberately not `biased`: with a fixed order, a pane flooding
            // the bus would keep the delivery branch ready forever and the
            // save timer would never be reached — herdr's starvation, rebuilt
            // by hand. Random selection is what makes the cap a ceiling.
            let step = tokio::select! {
                () = cancel.cancelled() => Step::Stop,
                command = mailbox.recv() => match command {
                    Some(command) => Step::Command(command),
                    None => Step::Stop,
                },
                delivery = events.recv(), if watching => match delivery {
                    Some(delivery) => Step::Delivery(delivery),
                    None => Step::BusClosed,
                },
                changed = self.config.changed(), if configured => match changed {
                    Ok(()) => Step::Configured,
                    Err(_) => Step::ConfigClosed,
                },
                () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)),
                    if due.is_some() => Step::Save,
            };
            match step {
                Step::Command(command) => self.command(command).await,
                Step::Delivery(delivery) => self.observe(delivery),
                Step::BusClosed => watching = false,
                Step::Configured => self.reconfigure().await,
                Step::ConfigClosed => configured = false,
                Step::Save => {
                    if let Err(err) = self.save().await {
                        tracing::warn!(error = %err, "the debounced save failed");
                    }
                }
                Step::Stop => break,
            }
        }
        self.drain(mailbox).await;
        self.report
    }

    /// Handle one mailbox message while the session is live.
    async fn command(&mut self, command: PersistCommand) {
        match command {
            PersistCommand::Snapshot(snapshot) => {
                self.pending = Some(Capture::without_sidecars(*snapshot));
                self.mark_dirty();
            }
            PersistCommand::Flush { reply } => {
                let _ = reply.send(self.save().await);
            }
        }
    }

    /// Fold one bus delivery into the dirty state.
    fn observe(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Event(envelope) => self.event(&envelope.event),
            // A gap can only hide transitions, and the capture that follows
            // reads current state anyway — so there is nothing to re-read, and
            // the honest reaction is simply to save.
            Delivery::Gap { .. } => self.mark_dirty(),
        }
    }

    /// Fold one event in: structure dirties, history moves a window, the rest
    /// is noise.
    fn event(&mut self, event: &Event) {
        match event {
            Event::PaneCreated { .. }
            | Event::PaneExited { .. }
            | Event::PaneRenamed { .. }
            | Event::PaneResized { .. }
            | Event::WorkspaceCreated { .. }
            | Event::WorkspaceRenamed { .. }
            | Event::WorkspaceClosed { .. }
            | Event::FocusChanged { .. }
            | Event::LayoutChanged { .. } => self.mark_dirty(),
            Event::HistoryCommitted { .. }
            | Event::HistoryEvicted { .. }
            | Event::HistoryInvalidated { .. } => self.sidecars.observe(event),
            // Damage, titles, clients coming and going — and whatever a later
            // version of the enum adds. None of it is the session's shape, and
            // folding per-frame damage in would put the quiet window out of
            // reach of any pane producing output (D-M1-7).
            _ => {}
        }
    }

    /// Apply a configuration change to the scrollback sidecars (D-M1-6).
    ///
    /// The toggle is hot both ways, and the two directions are deliberately not
    /// symmetric. Turning history *on* only arms the debounce: the dump rides
    /// the next save, where it belongs — reading a full pane's scrollback costs
    /// parser time (04 §3) and no config edit should buy itself a burst of it.
    /// Turning it *off* acts now, because what is on disk is the user's
    /// scrollback and a session's worth of secrets does not get to sit there
    /// until the next structural change happens to come along. It is the same
    /// order herdr keeps, for the same reason.
    async fn reconfigure(&mut self) {
        let history = self.config.borrow().persist.history;
        if history == self.history {
            return;
        }
        self.history = history;
        if history {
            self.mark_dirty();
        } else {
            self.wipe_sidecars().await;
        }
    }

    /// Remove every scrollback sidecar this session has written.
    ///
    /// On the blocking pool for the same reason a save is: this is an unlink
    /// per pane plus a directory removal, and the runtime's workers are not
    /// where filesystem latency gets to be discovered. Forgetting what was
    /// dumped is part of the delete rather than an afterthought — a pane whose
    /// sidecar was removed must be dumped again in full if history is ever
    /// turned back on.
    async fn wipe_sidecars(&mut self) {
        self.sidecars.forget_dumps();
        let dir = sidecar::dir(&self.ctx.state_dir);
        let removed = tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            outcome => outcome,
        })
        .await;
        match removed {
            Ok(Ok(())) => self.report.wipes += 1,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "the scrollback sidecars were not removed")
            }
            Err(err) => tracing::warn!(error = %err, "the sidecar wipe panicked"),
        }
    }

    /// Arm the debounce, or move its quiet deadline out if it is already armed.
    fn mark_dirty(&mut self) {
        let now = Instant::now();
        match &mut self.due {
            Some(due) => due.quiet = now + QUIET_WINDOW,
            None => {
                self.due = Some(Due {
                    quiet: now + QUIET_WINDOW,
                    cap: now + STALENESS_CAP,
                });
            }
        }
    }

    /// Capture, dump what moved, write what changed.
    ///
    /// The debounce is disarmed first and never re-armed on failure: a save
    /// that fails is not retried on a timer (herdr's `+250 ms` re-arm is what
    /// backpressure replaces here), it waits for the next change. The failure
    /// itself is returned, so a `Flush` hears about it.
    async fn save(&mut self) -> Result<(), PersistError> {
        self.due = None;
        let capture = match self.pending.take() {
            Some(capture) => capture,
            None => self.capture().await?,
        };
        self.report.dumps += self
            .sidecars
            .dump(&self.ctx.state_dir, &self.syncs, &capture.panes)
            .await;
        self.write(&capture.snapshot).await
    }

    /// Ask `Core` for a snapshot through the ordinary mailbox.
    async fn capture(&self) -> Result<Capture, PersistError> {
        let sidecars = self.config.borrow().persist.history;
        let (reply, answer) = oneshot::channel();
        self.core
            .send(CoreCommand::Session(SessionCall::Capture {
                sidecars,
                reply,
            }))
            .await
            .map_err(unreachable_core)?;
        answer
            .await
            .map_err(|_| unreachable_core(MailboxError::NoReply))
    }

    /// Write `snapshot`, unless it is what is already on disk.
    ///
    /// Serialization happens here and the durable write happens on the blocking
    /// pool: JSON is cheap, fsync is not, and only one of the two may run on an
    /// async worker.
    async fn write(&mut self, snapshot: &Snapshot) -> Result<(), PersistError> {
        if self.last.as_ref() == Some(snapshot) {
            self.report.unchanged += 1;
            return Ok(());
        }
        let bytes = encode(snapshot)?;
        let dir = self.ctx.state_dir.clone();
        let syncs = Shared(Arc::clone(&self.syncs));
        let commit =
            tokio::task::spawn_blocking(move || write_atomic(&dir, SNAPSHOT_NAME, &bytes, &syncs))
                .await
                .map_err(|err| PersistError::Io(io::Error::other(err)))??;
        if let Some(err) = commit.directory_error() {
            // The content is committed and visible; only its survival of a
            // power loss in the next moments is weaker. Worth saying, not
            // worth failing a save that did land.
            tracing::warn!(error = %err, "the snapshot landed but its directory is unsynced");
        }
        self.last = Some(snapshot.clone());
        self.report.saves += 1;
        Ok(())
    }

    /// Shutdown: receive until the mailbox closes, write once, return.
    ///
    /// Nothing here asks anything of another actor. `Core`'s final capture
    /// arrives on this mailbox and then every sender drops, which is what ends
    /// the loop; a dirty flag with no capture behind it writes nothing, because
    /// the only way to get one would be to ask `Core` — and `Core` is draining
    /// too (R-M1-2). Sidecars are skipped for the same reason: they are reads
    /// from pane actors that are already going down.
    async fn drain(&mut self, mut mailbox: mpsc::Receiver<PersistCommand>) {
        let mut waiting = Vec::new();
        while let Some(command) = mailbox.recv().await {
            match command {
                PersistCommand::Snapshot(snapshot) => {
                    self.pending = Some(Capture::without_sidecars(*snapshot));
                }
                PersistCommand::Flush { reply } => waiting.push(reply),
            }
        }

        let outcome = match self.pending.take() {
            Some(capture) => self.write(&capture.snapshot).await,
            None => Ok(()),
        };
        if let Err(err) = &outcome {
            tracing::warn!(error = %err, "the final snapshot could not be written");
        }
        // `PersistError` wraps `io::Error` and so cannot be cloned; a flush
        // that arrived after cancellation still deserves the reason rather
        // than a bare failure, so the message is what gets copied.
        let failed = outcome.err().map(|err| err.to_string());
        for reply in waiting {
            let _ = reply.send(match &failed {
                None => Ok(()),
                Some(message) => Err(PersistError::Io(io::Error::other(message.clone()))),
            });
        }
    }
}

/// `Core` cannot be reached, so there is nothing to capture.
///
/// Reported as I/O rather than invented as a new [`PersistError`] variant: from
/// a caller's side a save that cannot happen is a save that did not happen, and
/// the message says which half was missing.
fn unreachable_core(err: MailboxError) -> PersistError {
    PersistError::Io(io::Error::other(format!("nothing to capture: {err}")))
}

/// Adapts the shared, dynamically dispatched sync seam back to the sized form
/// [`write_atomic`] takes, so a write can hand it to the blocking pool without
/// making every type that names a `Persist` carry a parameter for it.
pub(super) struct Shared(pub(super) Arc<dyn Syncs + Send + Sync>);

impl Syncs for Shared {
    fn sync_file(&self, file: &File, path: &Path) -> io::Result<()> {
        self.0.sync_file(file, path)
    }

    fn sync_dir(&self, dir: &File, path: &Path) -> io::Result<()> {
        self.0.sync_dir(dir, path)
    }
}
