//! What this client asks the session to open for it, and what it sends back.
//!
//! Three kinds of stream and one rule each ([`crate::stream`] is the table they
//! land in):
//!
//! - **grid**, one per visible pane, opened by [`App::bind_visible`] whenever a
//!   state fold finds a pane on screen with nothing bound. This is the one that
//!   carries a claim: a re-bind presents the generation this client holds, when
//!   it holds a complete grid at all, and gets delta-only for its trouble
//!   ([`super::reconnect`] is where that judgement is made).
//! - **raw**, one per pane this terminal has typed into, opened lazily by
//!   [`App::forward`] on the first keystroke.
//! - **history**, one per pane copy mode has looked past the top of, opened by
//!   [`App::fetch_wanted_history`] and immediately spent on a `pane.history`
//!   call.
//!
//! All three are per *connection*: the bindings die with the socket, and a
//! reattach rebuilds the table from nothing rather than trimming it, because the
//! channel numbers are the server's to hand out and a successor's have no
//! relation to the ones a dead table holds.
//!
//! # Task ownership
//!
//! **W09** split this out of [`super::wired`], which the reconnect loop had
//! pushed past the 500-line soft budget for the second time (R-M3-7 named the
//! first split; this is the responsibility that was left over once the loop and
//! the redial were each in a file of their own).

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Effect, GridGeneration, PaneId};
use amx_proto::control::{Method, client as client_proto, stream as stream_proto};
use amx_proto::stream::{RawDirection, RawPaneIo, StreamKind};

use super::{App, AppError};

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Write `bytes` into `pane`'s PTY over its bound raw stream, binding one
    /// on first use.
    pub(super) async fn forward(&mut self, pane: PaneId, bytes: &[u8]) -> Result<(), AppError> {
        let channel = match self.bindings.raw_channel(pane) {
            Some(channel) => channel,
            None => match self.bind(StreamKind::RawPaneIo { pane }).await? {
                Some(reply) => {
                    self.bindings.bind_raw(pane, reply.channel);
                    reply.channel
                }
                // The pane is gone (or was never live); its input has nowhere
                // to go, which is not this client's failure.
                None => return Ok(()),
            },
        };
        let mut payload = Vec::with_capacity(bytes.len() + 17);
        RawPaneIo {
            pane,
            direction: RawDirection::ToPane,
            bytes,
        }
        .encode(&mut payload);
        self.session.write_stream(channel, &payload).await?;
        Ok(())
    }

    /// Bind a grid stream for every visible pane that has none yet.
    pub(super) async fn bind_visible(&mut self) -> Result<(), AppError> {
        let visible: Vec<PaneId> = self
            .model
            .focused_workspace()
            .map(|ws| ws.layout.panes())
            .unwrap_or_default();
        for pane in visible {
            if self.bindings.has_grid(pane) {
                continue;
            }
            // Presented on the call rather than left to the hello's block,
            // which the server would also honor: an explicit `generation`
            // always wins, and saying it here makes every re-bind say the same
            // thing whether it is this connection's first for the pane or its
            // fifth. Absent for a pane this client cannot vouch for, which
            // opens with a `First` keyframe exactly as every bind did before
            // M3 — [`super::reconnect`] is where that judgement is made.
            let generation = self
                .model
                .pane(pane)
                .filter(|grid| grid.complete())
                .map(crate::model::PaneGrid::generation);
            if let Some(reply) = self
                .bind_at(StreamKind::PaneGrid { pane }, generation)
                .await?
            {
                self.bindings.bind_grid(pane, reply.channel);
            }
        }
        Ok(())
    }

    /// Bind one stream with nothing to claim about it.
    ///
    /// What every non-grid stream binds with, and what a grid stream binds with
    /// on a first attach.
    async fn bind(
        &mut self,
        kind: StreamKind,
    ) -> Result<Option<stream_proto::BindReply>, AppError> {
        self.bind_at(kind, None).await
    }

    /// Bind one stream, recording its inbound cap.
    ///
    /// `Ok(None)` when the server refuses the bind itself (the pane died
    /// between the state snapshot and now) — a race, not a failure.
    async fn bind_at(
        &mut self,
        kind: StreamKind,
        generation: Option<GridGeneration>,
    ) -> Result<Option<stream_proto::BindReply>, AppError> {
        let params = serde_json::to_value(stream_proto::BindParams { kind, generation })
            .map_err(|_| AppError::BadState("unencodable bind"))?;
        let value = match self.call(Method::StreamBind.wire_name(), params).await {
            Ok(value) => value,
            Err(crate::net::NetError::Call(_)) => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let reply: stream_proto::BindReply =
            serde_json::from_value(value).map_err(|_| AppError::BadState("stream.bind reply"))?;
        self.session.bind_channel(reply.channel, reply.max_frame);
        Ok(Some(reply))
    }

    /// Declare this client's size and visible panes.
    pub async fn report_viewport(&mut self) -> Result<(), AppError> {
        let panes = self
            .model
            .focused_workspace()
            .map(|ws| ws.layout.panes())
            .unwrap_or_default();
        let params = serde_json::to_value(client_proto::Viewport {
            rows: self.model.term.h,
            cols: self.model.term.w,
            panes,
        })
        .map_err(|_| AppError::BadState("unencodable viewport"))?;
        let _ = self
            .call(Method::ClientViewport.wire_name(), params)
            .await?;
        Ok(())
    }

    /// Turn copy mode's cache misses into `pane.history` calls.
    pub(super) async fn fetch_wanted_history(&mut self) -> Result<(), AppError> {
        if self.wanted_history.is_empty() {
            return Ok(());
        }
        let wanted = std::mem::take(&mut self.wanted_history);
        for (pane, range) in wanted {
            if !self.bindings.has_history(pane) {
                match self.bind(StreamKind::History { pane }).await? {
                    Some(reply) => self.bindings.bind_history(pane, reply.channel),
                    None => continue,
                }
            }
            let params = serde_json::to_value(stream_proto::HistoryParams {
                pane,
                first: range.first,
                last: range.last,
                request: next_request(),
            })
            .map_err(|_| AppError::BadState("unencodable history request"))?;
            match self.call(Method::PaneHistory.wire_name(), params).await {
                // Chunks landed via `call`'s frame routing, into this pane's
                // scrollback cache — which is what copy mode is drawing over
                // it, so the damage is the pane's.
                Ok(_) => self.absorb(Effect::PaneDamage(pane)),
                // A range the pane can no longer serve — evicted while the
                // request was in flight. Copy mode renders it unavailable.
                Err(crate::net::NetError::Call(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }
}

/// A process-unique history request token.
fn next_request() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
