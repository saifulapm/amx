//! The live client: the loop that joins the terminal to the session socket.
//!
//! On attach the client folds `session.state` into its model, subscribes to the
//! session's event bus from the sequence that snapshot was captured at, binds a
//! grid and a raw input stream for every visible pane, and declares its
//! viewport so the server sizes those panes to this terminal (04 §3). From then
//! on the loop has four inputs — server frames, stdin bytes, `SIGWINCH`, and
//! the resize debounce — and one output: a repainted frame whenever any of them
//! changed something.
//!
//! Calls and their replies interleave with stream frames on one socket, so
//! every read path routes non-control frames through [`crate::stream::apply`]
//! before looking for its reply. The loop stays single-tasked on purpose:
//! nothing here needs concurrency beyond the select, and one task means the
//! model needs no lock.
//!
//! Server-initiated notifications interleave with both, and what they *mean*
//! lives next door in [`super::events`] — this file only makes sure every wake
//! ends by draining them, including the wakes whose real work was a call, so an
//! event that landed mid-call is folded as soon as the reply is in.
//!
//! What the loop *asks the socket for* — the grid, raw and history streams, and
//! the viewport declaration — lives in [`super::binds`], and what it does when
//! the socket ends under it lives in [`super::reconnect`].

use std::io::Write;
use std::os::fd::AsFd;
use std::path::Path;

use amx_core::{Effect, PaneId};
use amx_proto::ClientInfo;
use amx_proto::control::{Call, Method};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{App, AppError, RESIZE_DEBOUNCE};
use crate::input::InputEvent;
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
    /// model, subscribe to the event bus, bind streams for the visible panes,
    /// and declare the viewport.
    ///
    /// The snapshot comes *before* the subscription and hands it its own
    /// capture sequence, which is 04 §2's contract used exactly as written:
    /// state as of `seq`, deliveries from `seq + 1`. Subscribing first would
    /// replay transitions the snapshot already describes; subscribing later
    /// without the sequence would lose the ones in between.
    pub async fn attach(
        socket: &Path,
        fd: Fd,
        out: W,
        client: ClientInfo,
    ) -> Result<Self, AppError> {
        let stream = crate::net::connect(socket).await?;
        let (session, welcome) = Session::attach(stream, client.clone(), true, None).await?;
        let term = TerminalGuard::enter(fd, out)?;
        let mut app = Self::assemble(session, term)?;
        // Recorded before the first call: this is what makes the client
        // redialable at all, and the `SessionId` here is what a later
        // `Welcome` is compared against to tell "the session continued" from
        // "a different server answers this path" (`super::reconnect`).
        app.origin = Some(super::reconnect::Origin::new(
            socket.to_path_buf(),
            client,
            welcome.session,
        ));
        app.consumed(welcome.seq);
        let seq = app.sync_state().await?;
        app.subscribe_events(Some(seq)).await?;
        app.report_viewport().await?;
        app.consumed(seq);
        Ok(app)
    }

    /// Ask the host terminal for mouse reporting, if the user turned it on.
    ///
    /// Off by default and for a measured reason — `[client] mouse`, and
    /// `amx_core::config::DEFAULT_MOUSE` is where the reason is written down.
    /// Called by whoever read the configuration, after [`Self::attach`] has
    /// taken the terminal, in the same breath as the bindings: `attach` cannot
    /// do it itself because it does not know where the configuration lives, and
    /// this is the same seam `Input::set_bindings` already uses for the same
    /// reason.
    ///
    /// Releasing it is not a second call. The guard resets what it set on
    /// normal exit, on a panic unwind and from the `SIGTERM` arm, which is what
    /// makes leaving amx leave the terminal as it was found.
    pub fn set_mouse_tracking(&mut self, on: bool) {
        if on {
            self.term.request_mouse();
        }
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
        // Taken, not borrowed back: `run` consumes the app, so the buffer it
        // shares with `step_frame` has no second reader after this point.
        let mut frame = std::mem::take(&mut self.frame);
        self.repaint();
        flush(self.writer.bytes()).map_err(AppError::from_io)?;

        loop {
            enum Wake {
                Stdin(usize),
                Frame(amx_proto::FrameHeader),
                /// The socket ended under the read. Not the end of the client:
                /// a handoff retires the gateway and the successor binds the
                /// same path a moment later (`super::reconnect`).
                Lost(crate::net::NetError),
                Winch,
                Settle,
                /// The agents view's refresh window elapsed (D15, R-M4-7).
                Agents,
                Gone,
            }
            let resizing = self.pending_resize.is_some();
            let boarding = self.agents_open();
            // Every arm resolves to a `Wake` without touching `self` beyond
            // the one disjoint field its future borrows, so the handling
            // below runs with the borrows already released.
            //
            // Every arm must also be cancel-safe, because every other arm
            // routinely wins the race and drops it: a keystroke arriving mid
            // frame cancels the read, and a read that had left its progress on
            // the dropped future would resume in the middle of a payload. That
            // is why `read_frame_into` keeps its progress in the session.
            let wake = tokio::select! {
                read = stdin.read(&mut input) => match read {
                    Ok(n @ 1..) => Wake::Stdin(n),
                    // End of input is a detach too: whatever was driving this
                    // terminal has gone, and the panes are not its to take.
                    _ => Wake::Gone,
                },
                header = self.session.read_frame_into(&mut frame) => match header {
                    Ok(header) => Wake::Frame(header),
                    Err(err) => Wake::Lost(err),
                },
                signal = sigwinch.recv() => match signal {
                    Some(()) => Wake::Winch,
                    None => Wake::Gone,
                },
                () = tokio::time::sleep(RESIZE_DEBOUNCE), if resizing => Wake::Settle,
                // The one clock in this loop. It runs only while the board is
                // up, so a client that never opens it wakes exactly as often as
                // it did before — and while it is up, the detail lines and the
                // ages move on a session where nothing else is happening, which
                // no other wake would give them.
                () = tokio::time::sleep(super::AGENTS_REFRESH), if boarding => Wake::Agents,
            };

            // The whole round is fallible in one place so a transport failure
            // anywhere in it — the read, a call the input arm made, the drain
            // behind that call — reaches the same recovery. A failure that is
            // not the transport propagates out of `recover` unchanged.
            let round: Result<Flow, AppError> = async {
                match wake {
                    Wake::Stdin(n) => {
                        // The borrow checker cannot split `input` from `self`
                        // across the await inside; a copy of a keystroke is
                        // cheap.
                        let bytes = input[..n].to_vec();
                        if self.handle_bytes(&bytes).await? == Flow::Detach {
                            return Ok(Flow::Detach);
                        }
                    }
                    Wake::Frame(header) => {
                        let applied = stream::apply(
                            &mut self.model,
                            &mut self.caches,
                            &self.bindings,
                            header,
                            &frame,
                        );
                        self.absorb(effect_of(applied));
                    }
                    Wake::Lost(err) => return Err(err.into()),
                    Wake::Winch => {
                        let size = self.term.size()?;
                        self.note_resize(size);
                    }
                    Wake::Settle => self.settle_resize_wired().await?,
                    Wake::Agents => self.refresh_agents().await?,
                    Wake::Gone => return Ok(Flow::Detach),
                }

                // After every wake, not only after a frame: a control call made
                // by the input arm reads frames of its own, and any
                // notification that arrived behind its reply is sitting in the
                // session's queue now.
                self.drain_events().await?;
                self.fetch_wanted_history().await?;
                // Last, and after every wake: under D14's narrow projection
                // the pane this terminal shows is the pane the server sizes to
                // it, and focus can move without any call this client resyncs
                // after — a numeric jump, a pane chosen in the picker, another
                // client's `pane.focus`. A wide client stops at one comparison
                // (`super::binds`).
                self.redeclare_shown_pane().await?;
                Ok(Flow::Continue)
            }
            .await;

            match round {
                Ok(Flow::Detach) => break,
                Ok(Flow::Continue) => {}
                Err(err) => {
                    self.recover(err).await?;
                    // The reattach left a repaint owed and the frame buffer
                    // holding whatever the dead socket last handed over; the
                    // next round reads the successor's first frame.
                    frame.clear();
                }
            }

            if self.frame_due() {
                self.repaint();
                flush(self.writer.bytes()).map_err(AppError::from_io)?;
            }
            // Outside the frame, and outside the question of whether one was
            // owed: OSC 52 and OSC 9 are bytes for the host terminal, not cells
            // in this client's own picture, and a round that produced one
            // without changing the screen still has to send it.
            if !self.emit.is_empty() {
                flush(&self.emit).map_err(AppError::from_io)?;
                self.emit.clear();
            }
        }
        Ok(())
    }

    /// Answer a failed round: reattach if it was the transport, propagate
    /// otherwise.
    ///
    /// The two conditions are separate on purpose. A refused call, a frame this
    /// build cannot read or a state reply it cannot decode would say the same
    /// thing on a fresh connection, so retrying would only hide it; and a client
    /// with no socket behind it (the ssh bridge's socketpair) has nowhere to
    /// redial, so it keeps M2's behavior — the loop ends and the terminal comes
    /// back.
    async fn recover(&mut self, err: AppError) -> Result<(), AppError> {
        let lost = matches!(&err, AppError::Net(net) if net.is_transport());
        if !lost || !self.can_reconnect() {
            return Err(err);
        }
        self.reconnect().await?;
        Ok(())
    }

    /// Read exactly one server frame, apply it, and fold whatever notifications
    /// came with it.
    ///
    /// [`Self::run`]'s `Frame` arm, minus the select — for a caller that owns
    /// its own loop, and for a test that wants to step the wire without a
    /// terminal and a stdin behind it. It awaits until a frame arrives, so a
    /// caller waits on the server rather than on a clock.
    pub async fn step_frame(&mut self) -> Result<(), AppError> {
        // Borrowed out and put back rather than allocated: a caller stepping
        // the wire frame by frame is on the same hot path `run` is.
        let mut frame = std::mem::take(&mut self.frame);
        let header = self.session.read_frame_into(&mut frame).await;
        let applied = header.map(|header| {
            stream::apply(
                &mut self.model,
                &mut self.caches,
                &self.bindings,
                header,
                &frame,
            )
        });
        self.frame = frame;
        self.absorb(effect_of(applied?));
        self.drain_events().await
    }

    /// Feed stdin bytes through the modal layer and act on every consequence.
    pub async fn handle_bytes(&mut self, bytes: &[u8]) -> Result<Flow, AppError> {
        let picker_was_open = self.picker_open();
        let board_was_open = self.agents_open();
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

        // A picker that just opened lists whatever the model held — which,
        // with no event subscription in M0, may be stale. Re-sync and rebuild
        // it so a workspace another client created a moment ago is choosable.
        if !picker_was_open && self.picker_open() {
            self.sync_state().await?;
            let effect = self.open_picker();
            self.absorb(effect);
        }

        // A board that just opened is drawn from the last reply it held and
        // corrected here, rather than waiting up to a quarter second for its own
        // window to come round. `refresh_agents` is a no-op inside the window,
        // so this cannot make the second call R-M4-7 forbids.
        if !board_was_open && self.agents_open() {
            self.refresh_agents().await?;
        }
        // Seam 5's client half, after every read: the board's key table records
        // which pane it wants peeked and this is what binds or releases the
        // stream (`super::agents`). A read that touched neither costs one
        // comparison.
        self.settle_peek().await?;

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
            self.absorb(Effect::Full);
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
            effects,
            ..
        } = self;
        session
            .call_with(method, params, |header, payload| {
                effects.absorb(effect_of(stream::apply(
                    model, caches, bindings, header, payload,
                )));
            })
            .await
    }
}

/// What one applied stream frame invalidated.
///
/// The whole of the mapping, and the reason the client can tell an off-screen
/// delta from an on-screen one: both kinds of frame name the pane they landed
/// in — a grid delta and a history chunk alike, the second because copy mode
/// draws that pane's cache in that pane's rect.
const fn effect_of(applied: Applied) -> Effect {
    match applied {
        Applied::Grid(pane) | Applied::History { pane, .. } => Effect::PaneDamage(pane),
        Applied::Nothing => Effect::Nothing,
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
        | Method::WorkspaceSwitch
        // `agent.next` moves the session's focus, and across workspaces when
        // the head of the attention queue is in another one — which is the
        // whole point of one key for "handle the next one" (03). The
        // `FocusChanged` it publishes says which *pane*; it does not say that
        // this terminal should change what it is showing, because the identical
        // event is published when some other client switches workspace
        // (`super::events` explains why that one is not followed). So the
        // client that asked re-reads state, whose `focused_workspace` is the
        // answer, and binds what the new workspace shows.
        | Method::AgentNext => true,
        // M2's other rows change a pane's *contents* or its agent's status,
        // never the layout tree — `agent.start` is the one that mints a pane,
        // and it does so through the same `pane.split` path, whose
        // `pane_created` every client hears (`super::events`).
        Method::Ping
        | Method::SessionState
        | Method::SessionReport
        | Method::StreamBind
        | Method::PaneHistory
        | Method::ClientViewport
        | Method::AgentReport
        | Method::AgentStart
        | Method::AgentPrompt
        | Method::AgentExplain
        | Method::Wait
        | Method::EventsSubscribe
        | Method::PaneSendText
        | Method::PaneSendKeys
        | Method::PaneRun
        | Method::PaneRead
        | Method::PaneWaitOutput
        // A read-only projection: `agent.list` answers a question and moves
        // nothing, so the mirror has nothing to re-sync. X02 planted the arm
        // because the match is exhaustive over the table; X14 and X16 are the
        // readers.
        | Method::AgentList
        // The successor's `pane_created` and `layout_changed` are what a
        // reconnecting client folds; the call that asked for the swap does not
        // survive long enough to re-read anything.
        | Method::SessionHandoff => false,
    }
}
