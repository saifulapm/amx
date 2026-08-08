//! The knobs a pane host is started with, and the pty actor built from them.
//!
//! Split out of [`super`] by X02, which found that file at the soft budget with
//! X13 still to add the mouse-mode report to it (`docs/11-m4-plan.md` R-M4-5);
//! the code is T05's and T09's, moved and not changed.
//!
//! Two things sit here together because they are the same thing at two
//! distances: [`PaneHostConfig`] is what a caller asks for, and [`pty_config`]
//! is the actor configuration that request becomes — including the read
//! callback, which is the seam between a pane's two threads.

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc as sync_mpsc;
use std::time::Duration;

use amx_core::platform::{ProcessTree, WinSize};
use amx_core::{Bus, PaneId};
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::mailbox::{HostEvent, ParserCommand, Scratch};
use super::probe::PaneProbe;
use crate::actor::CoreHandle;
use crate::handoff::manifest::PaneSeed;
use crate::platform::UnixProcessTree;
use crate::pty::PtyActorConfig;

/// How long output coalesces before a frame is published.
const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(8);

/// How many commands may queue for one pane before its sender waits.
const DEFAULT_MAILBOX: usize = 256;

/// How much memory a pane's scrollback may hold.
///
/// Bytes, not rows: the vendored library's `max_scrollback` is a memory bound
/// whatever its header says, and the rows it buys move with the pane's width
/// (`docs/notes/scrollback-identity.md`).
const DEFAULT_SCROLLBACK: usize = 4 * 1024 * 1024;

/// Everything one pane host needs to start.
pub struct PaneHostConfig {
    /// Which pane this is. Every event and report carries it.
    pub pane: PaneId,
    /// The session event bus.
    pub bus: Arc<Bus>,
    /// The `Core`'s mailbox, when there is one to report to.
    pub core: Option<CoreHandle>,
    /// Initial grid size. The pty was opened at this size too.
    pub size: WinSize,
    /// How much memory, in bytes, the scrollback may hold.
    pub max_scrollback: usize,
    /// How long output coalesces before a frame is published.
    pub frame_interval: Duration,
    /// Command mailbox depth.
    pub mailbox: usize,
    /// How the pane reads the process tree behind it.
    pub process_tree: Arc<dyn ProcessTree>,
    /// The shutdown signal.
    pub cancel: CancellationToken,
    /// Counters for the threading invariants.
    pub probe: PaneProbe,
    /// What an inherited pane is rebuilt from (D-M3-4), absent for a fresh one.
    ///
    /// Present exactly when the pty session handed to
    /// [`PaneHost::spawn`] is one that crossed a handoff: the seed carries the
    /// bytes that put the screen and the scrollback back, and the three
    /// counters — row-id head, grid generation, frame counter — a successor
    /// continues rather than restarts.
    pub seed: Option<PaneSeed>,
}

impl PaneHostConfig {
    /// A config with the defaults, for one pane on one bus.
    #[must_use]
    pub fn new(pane: PaneId, bus: Arc<Bus>, size: WinSize) -> Self {
        Self {
            pane,
            bus,
            core: None,
            size,
            max_scrollback: DEFAULT_SCROLLBACK,
            frame_interval: DEFAULT_FRAME_INTERVAL,
            mailbox: DEFAULT_MAILBOX,
            process_tree: Arc::new(UnixProcessTree),
            cancel: CancellationToken::new(),
            probe: PaneProbe::new(),
            seed: None,
        }
    }
}

impl fmt::Debug for PaneHostConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneHostConfig")
            .field("pane", &self.pane)
            .field("size", &self.size)
            .field("max_scrollback", &self.max_scrollback)
            .field("frame_interval", &self.frame_interval)
            .field("mailbox", &self.mailbox)
            .finish_non_exhaustive()
    }
}

/// Build the pty actor's config, read callback included.
///
/// The callback is the seam between the two threads: it fills the shuttle
/// buffer, blocks while the parser owns it, and pushes whatever came back into
/// the reply queue T05 is holding open for it.
pub(super) fn pty_config<S>(
    session: S,
    pane: PaneId,
    parser: sync_mpsc::Sender<ParserCommand>,
    done: sync_mpsc::Receiver<Box<Scratch>>,
    events: mpsc::UnboundedSender<HostEvent>,
) -> PtyActorConfig<S> {
    let mut scratch = Some(Box::new(Scratch::default()));
    let on_read = move |bytes: &[u8], responses: &mut Vec<Bytes>| {
        // `None` means the parser already went away with the buffer; the pane
        // is on its way down and the bytes have nowhere to go.
        let Some(mut buffer) = scratch.take() else {
            return;
        };
        buffer.input.clear();
        buffer.input.extend_from_slice(bytes);
        buffer.replies.clear();
        if parser.send(ParserCommand::Parse(buffer)).is_err() {
            return;
        }
        let Ok(buffer) = done.recv() else {
            return;
        };
        if !buffer.replies.is_empty() {
            responses.push(Bytes::copy_from_slice(&buffer.replies));
        }
        scratch = Some(buffer);
    };

    let mut config = PtyActorConfig::new(session, Box::new(on_read));
    config.name = format!("amx-pty-{}", short(pane));
    config.on_exit = Some(Box::new(move |exit| {
        let _ = events.send(HostEvent::Exited(exit));
    }));
    config
}

/// The head of a pane id, for a thread name.
///
/// Six characters: Linux truncates a thread name at 15 bytes, and the prefixes
/// here are eight, so anything longer would be cut off mid-identifier.
pub(super) fn short(pane: PaneId) -> String {
    pane.to_string().chars().take(6).collect()
}
