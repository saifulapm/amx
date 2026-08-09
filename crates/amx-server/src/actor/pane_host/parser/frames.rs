//! What the parser does once the bytes are parsed.
//!
//! The side effects the terminal raised, the frame it publishes, the history
//! rows that left the live grid, and the chunked service of a range somebody
//! asked for. Split out of [`super`] by X02, which found the file 32 lines over
//! the soft budget with X13 still to grow it (`docs/11-m4-plan.md` R-M4-5); the
//! code is T09's and W05's, moved and not changed.
//!
//! The line it splits on is the thread's own loop: [`super`] is the commands
//! that come *in* — drive, resize, export, feed the parser — and this is
//! everything that goes *out*, which is also why the two grow for different
//! reasons.

use std::sync::atomic::Ordering;
use std::time::Instant;

use amx_core::GridGeneration;
use amx_core::platform::WinSize;
use amx_vt::TerminalEvent;
use tracing::{debug, warn};

use super::super::mailbox::HostEvent;
use super::super::{ExportError, PaneExport};
use super::Parser;
use crate::actor::HistoryRows;
use crate::handoff::manifest::{self, PaneManifest};
use crate::history::HistoryEvent;

impl Parser {
    /// Turn the side effects of a parse into pane events.
    pub(super) fn drain_effects(&mut self) {
        for index in 0..self.effects.events().len() {
            let event = match &self.effects.events()[index] {
                TerminalEvent::Bell => Some(HostEvent::Bell),
                TerminalEvent::TitleChanged => match self.terminal.title() {
                    Ok(Some(title)) => Some(HostEvent::Title(title)),
                    Ok(None) => None,
                    Err(err) => {
                        debug!(error = %err, "pane title could not be read");
                        None
                    }
                },
                // The pane's own cwd tracking and clipboard routing are not M0.
                TerminalEvent::PwdChanged | TerminalEvent::ClipboardWrite { .. } => None,
                _ => None,
            };
            if let Some(event) = event {
                self.send(event);
            }
        }
        // After the effects and not among them: no `TerminalEvent` variant
        // announces a mode change, so the mouse mode is *polled* on the one
        // thread that may read the terminal at all. `super::super::mouse` has
        // the cost, which is one FFI getter for a pane that asked for nothing —
        // that is, for very nearly every pane, very nearly always.
        self.observe_mouse();
    }

    /// Report the pane's mouse mode if it moved since the last look.
    ///
    /// A fact, not a transition: it leaves as a [`HostEvent`] the pane actor
    /// turns into a report for `Core` to fold, and nothing publishes an
    /// `Event` for it. `session.state` then answers `PaneState.mouse` out of
    /// that fold, synchronously, with no parser round trip on the query path —
    /// which is the whole of `docs/notes/m4-mouse-path.md` §3, and the same
    /// shape the history window already has.
    pub(super) fn observe_mouse(&mut self) {
        let now = super::super::mouse::mouse_mode(&self.terminal);
        if now == self.mouse {
            return;
        }
        self.mouse = now;
        self.send(HostEvent::Mouse(now));
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
    pub(super) fn resize(&mut self, size: WinSize) -> bool {
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

    /// Freeze the pane: a manifest entry and a duplicate of the master.
    ///
    /// The descriptor is asked for **first**, and that ordering is the whole of
    /// the quiesced-only rule (D-M3-3): the pty actor hands one out in no other
    /// state, so a running pane is refused before a single row is read rather
    /// than after a capture that would already be stale. Nothing else here
    /// knows what state the terminal is in, and nothing else should — the actor
    /// that owns the descriptor is the one that owns the answer.
    pub(super) fn export(&mut self) -> Result<PaneExport, ExportError> {
        let master = self.pty.dup_fd().map_err(ExportError::from)?;
        // A frame first, so the capture describes what the terminal holds now
        // rather than what it held at the last coalescing boundary.
        self.publish();
        let snapshot = self.snapshots.latest();
        let manifest = PaneManifest::capture(manifest::Capture {
            pane: self.pane,
            child: self.child,
            generation: self.current_generation(),
            terminal: &self.terminal,
            tracker: &mut self.history,
            snapshot: &snapshot,
        })?;
        Ok(PaneExport { manifest, master })
    }

    /// Copy the damaged rows into the back buffer and publish it.
    pub(super) fn publish(&mut self) {
        let started = Instant::now();
        if let Err(err) = self.frame() {
            warn!(error = %err, "pane frame could not be published");
            return;
        }
        self.probe.published(started.elapsed());
        self.track_history();
    }

    pub(super) fn frame(&mut self) -> amx_vt::Result<()> {
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

    pub(super) fn current_generation(&self) -> GridGeneration {
        GridGeneration::from_raw(self.generation.load(Ordering::Acquire))
    }

    /// Follow the scrollback across a frame and report what moved.
    pub(super) fn track_history(&mut self) {
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

    /// Serve one chunk of the oldest transfer, replying when its range is done.
    ///
    /// Called once per loop turn, which is the interleaving: a parse queued
    /// behind a large transfer is delayed by one chunk, never the whole range.
    pub(super) fn serve_chunk(&mut self) {
        let Some(transfer) = self.transfers.front_mut() else {
            return;
        };
        let step =
            transfer
                .chunks
                .next_chunk(&mut self.history, &self.terminal, &mut transfer.rows);
        let outcome = match step {
            Some(Ok(_)) if transfer.chunks.more() => return,
            Some(Ok(_)) | None => Ok(()),
            Some(Err(err)) => Err(err),
        };
        let Some(transfer) = self.transfers.pop_front() else {
            return;
        };
        self.probe.served_history();
        let _ = transfer.reply.send(outcome.map(|()| HistoryRows {
            range: transfer.range,
            rows: transfer.rows,
        }));
    }
}
