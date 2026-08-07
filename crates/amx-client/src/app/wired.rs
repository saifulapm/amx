//! The live client: the loop that joins the terminal to the session socket.
//!
//! On attach the client folds `session.state` into its model, binds a grid
//! and a raw input stream for every visible pane, and declares its viewport
//! so the server sizes those panes to this terminal (04 §3). From then on the
//! loop has four inputs — server frames, stdin bytes, `SIGWINCH`, and the
//! resize debounce — and one output: a repainted frame whenever any of them
//! changed something.
//!
//! Calls and their replies interleave with stream frames on one socket, so
//! every read path routes non-control frames through [`crate::stream::apply`]
//! before looking for its reply. The loop stays single-tasked on purpose:
//! nothing here needs concurrency beyond the select, and one task means the
//! model needs no lock.

use std::io::Write;
use std::os::fd::AsFd;
use std::path::Path;

use amx_core::{PaneId, RowRange, WorkspaceId};
use amx_proto::ClientInfo;
use amx_proto::control::{Call, Method, client as client_proto, session, stream as stream_proto};
use amx_proto::stream::{RawDirection, RawPaneIo, StreamKind};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{App, AppError, RESIZE_DEBOUNCE};
use crate::input::InputEvent;
use crate::model::WorkspaceModel;
use crate::net::Session;
use crate::stream::{self, Applied};
use crate::term::{Sigwinch, TerminalGuard};

/// What one round of the loop asks the caller to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    /// Keep looping.
    Continue,
    /// The user detached (prefix `d`, or stdin closed).
    Detach,
}

/// One owned input consequence, buffered so the async half can act on it
/// after the borrow-heavy decode has finished.
enum OwnedEvent {
    Forward { pane: PaneId, bytes: Vec<u8> },
    Call(Call),
    Detach,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Connect to `socket`, negotiate, take `fd` into raw mode (writing the
    /// alt-screen sequence to `out`), fold the session's state into a fresh
    /// model, bind streams for the visible panes, and declare the viewport.
    pub async fn attach(
        socket: &Path,
        fd: Fd,
        out: W,
        client: ClientInfo,
    ) -> Result<Self, AppError> {
        let stream = crate::net::connect(socket).await?;
        let (session, _welcome) = Session::attach(stream, client, true, None).await?;
        let term = TerminalGuard::enter(fd, out)?;
        let mut app = Self::assemble(session, term)?;
        app.sync_state().await?;
        app.report_viewport().await?;
        Ok(app)
    }

    /// Run until the user detaches or the server goes away, flushing every
    /// repaint through `flush`.
    pub async fn run(
        mut self,
        mut sigwinch: Sigwinch,
        mut stdin: impl tokio::io::AsyncRead + Unpin,
        mut flush: impl FnMut(&[u8]) -> std::io::Result<()>,
    ) -> Result<(), AppError> {
        let mut input = [0_u8; 4096];
        let mut frame = Vec::new();
        self.repaint();
        flush(self.writer.bytes()).map_err(AppError::from_io)?;

        loop {
            enum Wake {
                Stdin(usize),
                Frame(amx_proto::FrameHeader),
                Winch,
                Settle,
                Gone,
            }
            let resizing = self.pending_resize.is_some();
            // Every arm resolves to a `Wake` without touching `self` beyond
            // the one disjoint field its future borrows, so the handling
            // below runs with the borrows already released.
            let wake = tokio::select! {
                read = stdin.read(&mut input) => match read {
                    Ok(n @ 1..) => Wake::Stdin(n),
                    // End of input is a detach too: whatever was driving this
                    // terminal has gone, and the panes are not its to take.
                    _ => Wake::Gone,
                },
                header = self.session.read_frame_into(&mut frame) => match header {
                    Ok(header) => Wake::Frame(header),
                    Err(err) => return Err(err.into()),
                },
                signal = sigwinch.recv() => match signal {
                    Some(()) => Wake::Winch,
                    None => Wake::Gone,
                },
                () = tokio::time::sleep(RESIZE_DEBOUNCE), if resizing => Wake::Settle,
            };

            match wake {
                Wake::Stdin(n) => {
                    // The borrow checker cannot split `input` from `self`
                    // across the await inside; a copy of a keystroke is cheap.
                    let bytes = input[..n].to_vec();
                    if self.handle_bytes(&bytes).await? == Flow::Detach {
                        break;
                    }
                }
                Wake::Frame(header) => {
                    if stream::apply(
                        &mut self.model,
                        &mut self.caches,
                        &self.bindings,
                        header,
                        &frame,
                    ) != Applied::Nothing
                    {
                        self.dirty = true;
                    }
                }
                Wake::Winch => {
                    let size = self.term.size()?;
                    self.note_resize(size);
                }
                Wake::Settle => self.settle_resize_wired().await?,
                Wake::Gone => break,
            }

            self.fetch_wanted_history().await?;
            if self.dirty {
                self.dirty = false;
                self.repaint();
                flush(self.writer.bytes()).map_err(AppError::from_io)?;
                if !self.emit.is_empty() {
                    flush(&self.emit).map_err(AppError::from_io)?;
                    self.emit.clear();
                }
            }
        }
        Ok(())
    }

    /// Feed stdin bytes through the modal layer and act on every consequence.
    pub async fn handle_bytes(&mut self, bytes: &[u8]) -> Result<Flow, AppError> {
        let picker_was_open = self.picker_open();
        let mut events: Vec<OwnedEvent> = Vec::new();
        self.handle_input(bytes, &mut |event| {
            events.push(match event {
                InputEvent::Forward { pane, bytes } => OwnedEvent::Forward {
                    pane,
                    bytes: bytes.to_vec(),
                },
                InputEvent::Call(call) => OwnedEvent::Call(call),
                InputEvent::Detach => OwnedEvent::Detach,
            });
        });
        self.dirty = true;

        // A picker that just opened lists whatever the model held — which,
        // with no event subscription in M0, may be stale. Re-sync and rebuild
        // it so a workspace another client created a moment ago is choosable.
        if !picker_was_open && self.picker_open() {
            self.sync_state().await?;
            self.open_picker();
        }

        let mut resync = false;
        for event in events {
            match event {
                OwnedEvent::Forward { pane, bytes } => self.forward(pane, &bytes).await?,
                OwnedEvent::Call(call) => {
                    let method = call.method();
                    let params = call
                        .params()
                        .map_err(|_| AppError::BadState("unencodable call"))?;
                    match self.call(method.wire_name(), params).await {
                        Ok(_) => resync |= mutates_layout(method),
                        // A refused verb (splitting a pane that just died,
                        // say) must not take the client down with it.
                        Err(crate::net::NetError::Call(_)) => {}
                        Err(err) => return Err(err.into()),
                    }
                }
                OwnedEvent::Detach => return Ok(Flow::Detach),
            }
        }
        if resync {
            self.sync_state().await?;
        }
        Ok(Flow::Continue)
    }

    /// Write `bytes` into `pane`'s PTY over its bound raw stream, binding one
    /// on first use.
    async fn forward(&mut self, pane: PaneId, bytes: &[u8]) -> Result<(), AppError> {
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

    /// Fold a fresh `session.state` into the model and bind whatever the
    /// focused workspace newly shows.
    pub async fn sync_state(&mut self) -> Result<(), AppError> {
        let value = self
            .call(Method::SessionState.wire_name(), serde_json::json!({}))
            .await?;
        let state: session::StateReply =
            serde_json::from_value(value).map_err(|_| AppError::BadState("session.state reply"))?;

        let workspaces: Vec<WorkspaceId> = state.workspaces.iter().map(|ws| ws.workspace).collect();
        self.model.retain_workspaces(|id| workspaces.contains(&id));
        let panes: Vec<PaneId> = state.panes.iter().map(|pane| pane.pane).collect();
        self.model.retain_panes(|id| panes.contains(&id));
        self.caches.retain(|id, _| panes.contains(id));

        for ws in &state.workspaces {
            self.model.set_workspace(
                ws.workspace,
                WorkspaceModel {
                    label: ws.label.clone(),
                    layout: ws.layout.clone(),
                },
            );
            if let Some(focus) = ws.focus {
                self.focus.insert(ws.workspace, focus);
            }
        }
        if let Some(focused) = state.focused_workspace {
            self.model.focus_workspace(focused);
        }
        for pane in &state.panes {
            let cache = self.caches.entry(pane.pane).or_default();
            let known = cache.head().get();
            let head = pane.history_head.get();
            if head < known {
                cache.invalidate(pane.history_head);
            } else if head > known {
                cache.commit(RowRange::new(
                    amx_core::RowId::from_raw(known),
                    amx_core::RowId::from_raw(head - 1),
                ));
            }
            cache.evict(pane.history_floor);
        }

        self.bind_visible().await?;
        self.layout_dirty = true;
        self.dirty = true;
        Ok(())
    }

    /// Bind a grid stream for every visible pane that has none yet.
    async fn bind_visible(&mut self) -> Result<(), AppError> {
        let visible: Vec<PaneId> = self
            .model
            .focused_workspace()
            .map(|ws| ws.layout.panes())
            .unwrap_or_default();
        for pane in visible {
            if self.bindings.has_grid(pane) {
                continue;
            }
            if let Some(reply) = self.bind(StreamKind::PaneGrid { pane }).await? {
                self.bindings.bind_grid(pane, reply.channel);
            }
        }
        Ok(())
    }

    /// Bind one stream, recording its inbound cap.
    ///
    /// `Ok(None)` when the server refuses the bind itself (the pane died
    /// between the state snapshot and now) — a race, not a failure.
    async fn bind(
        &mut self,
        kind: StreamKind,
    ) -> Result<Option<stream_proto::BindReply>, AppError> {
        let params = serde_json::to_value(stream_proto::BindParams { kind })
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

    /// Act on the pending resize: repaint locally, then tell the server so it
    /// re-sizes the panes this client drives (04 §3 — active client rule).
    async fn settle_resize_wired(&mut self) -> Result<(), AppError> {
        let mut settled = false;
        self.settle_resize(&mut |_| settled = true);
        if settled {
            self.report_viewport().await?;
            // The active client resized: everything it cached about row
            // widths may reflow server-side. Dropping the caches is the
            // conservative reading of the width-reflow invalidation this
            // client cannot yet hear as an event.
            self.caches.clear();
            self.dirty = true;
        }
        Ok(())
    }

    /// Turn copy mode's cache misses into `pane.history` calls.
    async fn fetch_wanted_history(&mut self) -> Result<(), AppError> {
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
                // Chunks landed via `call`'s frame routing; repaint follows.
                Ok(_) => self.dirty = true,
                // A range the pane can no longer serve — evicted while the
                // request was in flight. Copy mode renders it unavailable.
                Err(crate::net::NetError::Call(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// Make one call, routing interleaved stream frames into the model.
    pub(super) async fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, crate::net::NetError> {
        let Self {
            session,
            model,
            caches,
            bindings,
            dirty,
            ..
        } = self;
        session
            .call_with(method, params, |header, payload| {
                if stream::apply(model, caches, bindings, header, payload) != Applied::Nothing {
                    *dirty = true;
                }
            })
            .await
    }
}

impl AppError {
    fn from_io(err: std::io::Error) -> Self {
        Self::Net(crate::net::NetError::Io(err))
    }
}

/// Whether a successful `method` may have changed the layout tree or focus —
/// the calls after which the mirror re-syncs from the server.
fn mutates_layout(method: Method) -> bool {
    match method {
        Method::PaneSplit
        | Method::PaneClose
        | Method::PaneZoom
        | Method::PaneSwap
        | Method::PaneMove
        | Method::PaneResize
        | Method::PaneFocus
        | Method::PaneRename
        | Method::WorkspaceCreate
        | Method::WorkspaceRename
        | Method::WorkspaceKill
        | Method::WorkspaceSwitch => true,
        Method::Ping
        | Method::SessionState
        | Method::SessionReport
        | Method::StreamBind
        | Method::PaneHistory
        | Method::ClientViewport => false,
    }
}

/// A process-unique history request token.
fn next_request() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
