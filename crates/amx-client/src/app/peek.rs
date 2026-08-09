//! D15's peek region: one selected pane, live, read-only.
//!
//! `Space` on a row of the agents view shows that agent's pane as it is
//! painting, in a region reserved beside the list, without attaching to it and
//! without sending it a byte. Everything the region needs already exists: the
//! grid stream carries a pane's cells to whoever bound one, and `stream.bind`
//! resolves a pane with no visibility check, so a client may bind a pane in a
//! workspace it is not drawing (`docs/11-m4-plan.md` §5, X15's dependency note;
//! X12's outcome settles the other half — `Viewport.panes` is a *sizing*
//! declaration and never a traffic filter, so a bind outside the declaration
//! stays legitimate).
//!
//! # The stream is the whole of the difficulty
//!
//! A peek is a subscription, and a surface that opens one per selection and
//! never closes one leaks quietly: the cells keep arriving, the pane keeps
//! costing this client bandwidth, and nothing on screen says so. Three rules
//! answer it, and [`App::peek_live`] is what makes them observable:
//!
//! - **At most one peek-owned stream is live.** Moving the selection releases
//!   the old one *before* it opens the new one, so the count is one at every
//!   instant rather than one after a tidy-up.
//! - **A stream is released, not dropped.** The protocol has no `stream.unbind`
//!   — bindings die with the connection (`amx-server/src/conn/streams.rs`) — so
//!   the release is [`FlowControl::Pause`], which is what that signal is for:
//!   "stop sending on this stream … damage accumulates into the per-client
//!   dirty set while paused; it is never queued as a backlog of deltas"
//!   (`amx-proto/src/stream/mod.rs`). The server's half is already built and
//!   already pinned — `crates/amx-server/tests/flow_control.rs`'s
//!   `a_paused_stream_with_pending_damage_sleeps_until_a_flow_signal` and the
//!   pause arm of `resync_request_emits_a_keyframe`. A
//!   re-peek of the same pane resumes the stream it already has rather than
//!   binding a second one, so the streams this connection holds are bounded by
//!   the panes it has ever peeked and not by how often the selection moved.
//! - **A borrowed stream is never released.** The projection binds a grid
//!   stream for every pane of the focused workspace ([`super::binds`]), and
//!   peeking one of those opens nothing. Pausing on the way out would then
//!   freeze a pane the user is looking at, which is why ownership is recorded
//!   rather than inferred from the binding table.
//!
//! The mirror image is the same rule read backwards: a pane whose peek-owned
//! stream was paused can become visible later — the user switches to its
//! workspace — and [`App::bind_visible`](super::App::bind_visible) finds it
//! already bound and would otherwise skip it forever. It resumes it instead.
//!
//! # What the region is, and what draws it
//!
//! [`App::peek_layout`] is the boundary with the agents view: it answers where
//! the list goes and where the peek goes, out of the content area and the
//! projection, and the two surfaces never compute it twice. Under D14's narrow
//! projection the peek takes the whole content area and the list gets none of
//! it — 10 §D14: "peek replaces the list rather than sharing the width".
//!
//! # Task ownership
//!
//! **X15**. The region is a surface shared with **X14** and that seam is X00's
//! (seam 5): X14 reserves it and X15 fills it. X14 had not merged when this
//! landed, so the split is defined here — [`PeekLayout`] — rather than guessed
//! at, and `docs/notes/m4-wave-outcomes.md` names the four entry points the view
//! calls.

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Effect, PaneId, Rect};
use amx_proto::rpc::Notification;
use amx_proto::stream::{FlowControl, StreamId, StreamKind};

use super::{App, AppError, Projection};
use crate::render::{chrome, grid};

/// The JSON-RPC method a flow-control signal rides.
///
/// The server's own spelling, and its only reader:
/// `amx-server/src/conn/reader.rs`'s `FLOW_METHOD`. It is a notification and not
/// a call — there is no reply, because there is nothing to answer: a pause is a
/// statement about what this client wants, not a question about the pane.
const FLOW_METHOD: &str = "stream.flow";

/// What the region shows for a pane that is no longer in the session.
///
/// D15's acceptance asks that a peeked pane which dies leaves the view usable
/// and says so. It says so here: the region keeps its shape, the list above it
/// keeps working, and the row that named the pane is the agents view's to
/// retire on its next refresh.
const PANE_GONE: &str = "pane closed";

/// A grid stream this client opened for a peek.
///
/// Named by [`super::App`]'s field and opaque to it: what it holds and what
/// moves it are this module's, which is what keeps the release rule in one file.
#[derive(Clone, Copy, Debug)]
pub(super) struct PeekStream {
    /// The stream a flow signal names.
    stream: StreamId,
    /// Whether the server is still sending on it.
    live: bool,
}

/// Where the agents view's list goes, and where the peek region goes.
///
/// One answer for both surfaces, so the list cannot draw over the peek or leave
/// a gap beside it. `peek` is `None` when no peek is open — and also when one is
/// and the content area has no room to split, which a caller reads as "draw the
/// list over everything" rather than as an error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PeekLayout {
    /// The rows the list has.
    pub list: Rect,
    /// The rows the peek has, when there is a peek and there is room.
    pub peek: Option<Rect>,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Show `pane` in the peek region, live and read-only.
    ///
    /// Moving the selection is this call with the next pane: the stream the
    /// previous peek owned is released first, so two peeks never overlap even
    /// for the round trip the new bind costs.
    ///
    /// # Errors
    ///
    /// The socket failed. A pane the server refuses to bind is not an error —
    /// it died between the list being drawn and the key being pressed, and the
    /// region says so.
    pub async fn open_peek(&mut self, pane: PaneId) -> Result<Effect, AppError> {
        if self.peeked == Some(pane) {
            return Ok(Effect::Nothing);
        }
        self.release_peek_stream().await?;
        self.peeked = Some(pane);
        self.ensure_peek_stream(pane).await?;
        Ok(Effect::Full)
    }

    /// Close the peek region and release whatever stream it owned.
    ///
    /// # Errors
    ///
    /// The socket failed.
    pub async fn close_peek(&mut self) -> Result<Effect, AppError> {
        if self.peeked.is_none() {
            return Ok(Effect::Nothing);
        }
        self.release_peek_stream().await?;
        self.peeked = None;
        Ok(Effect::Full)
    }

    /// The pane the peek region is showing, if one is open.
    #[must_use]
    pub const fn peeked(&self) -> Option<PaneId> {
        self.peeked
    }

    /// Every pane whose peek-owned grid stream is still being sent on.
    ///
    /// Exposed for the reason [`App::repaints`] is: a stream this client
    /// stopped drawing but never released is invisible in the frame and exact
    /// here. The rule the module header states is that this holds at most one
    /// pane, whatever the selection did on the way.
    #[must_use]
    pub fn peek_live(&self) -> Vec<PaneId> {
        self.peek_streams
            .iter()
            .filter(|(_, held)| held.live)
            .map(|(&pane, _)| pane)
            .collect()
    }

    /// Where the list and the peek go inside the content area.
    ///
    /// The agents view asks this rather than measuring the screen itself, so the
    /// two surfaces cannot disagree about the seam between them.
    ///
    /// Under [`Projection::Single`] the peek takes everything and the list is
    /// left zero rows: on a phone there is no width to share, and eight rows of
    /// list over ten rows of a pane the user opened *in order to read* is the
    /// region D14's policy exists to replace.
    #[must_use]
    pub fn peek_layout(&self) -> PeekLayout {
        let content = self.model.content_area();
        let empty = Rect::new(content.x, content.y, content.w, 0);
        if self.peeked.is_none() {
            return PeekLayout {
                list: content,
                peek: None,
            };
        }
        if matches!(self.projection(), Projection::Single(_)) {
            return PeekLayout {
                list: empty,
                peek: Some(content),
            };
        }
        // Half each, the list keeping the odd row: the list is what the user is
        // steering with and it must not lose a row to a region that is only
        // being read. Below two rows there is nothing to split, and the honest
        // answer is that the peek has no room rather than a region of height
        // zero for the caller to discover.
        let peek_rows = content.h / 2;
        if peek_rows == 0 || content.w == 0 {
            return PeekLayout {
                list: content,
                peek: None,
            };
        }
        let list_rows = content.h - peek_rows;
        PeekLayout {
            list: Rect::new(content.x, content.y, content.w, list_rows),
            peek: Some(Rect::new(
                content.x,
                content.y + list_rows,
                content.w,
                peek_rows,
            )),
        }
    }

    /// Draw the peek region: the pane's own cells, exactly as a pane draws them.
    ///
    /// Runs after the overlays and before the status line, so the region sits
    /// over whatever the list drew and never over the chrome. The border is the
    /// projection's rule and not this surface's — [`Projection::Single`] draws
    /// none, for the reason [`super::narrow`]'s header gives — so the region is
    /// bounded exactly the way a pane in the same projection is.
    pub(super) fn draw_peek(&mut self) {
        let Some(pane) = self.peeked else {
            return;
        };
        let Some(rect) = self.peek_layout().peek else {
            return;
        };
        let inner = match self.projection() {
            Projection::Single(_) => rect,
            Projection::Tiled => {
                chrome::draw_border(&mut self.writer, rect);
                chrome::inset(rect)
            }
        };
        if inner.w == 0 || inner.h == 0 {
            return;
        }
        let alive = self.pane_is_in_a_layout(pane);
        match self.model.pane(pane) {
            Some(cells) => grid::blit(&mut self.writer, cells, inner),
            // No cells yet is not the same fact as no pane. A bind's keyframe is
            // a round trip away when the peek opens, and painting "pane closed"
            // over that window would call every healthy agent dead for a frame.
            None if alive => grid::blit_absent(&mut self.writer, inner, ""),
            None => grid::blit_absent(&mut self.writer, inner, PANE_GONE),
        }
    }

    /// Give `pane` its stream back if a closed peek paused it.
    ///
    /// Called from [`App::bind_visible`](super::App::bind_visible) for every
    /// pane the projection draws that is already bound. Without it a pane that
    /// was peeked, released, and then brought on screen would be bound, silent
    /// and never rebound — the binding table says it has a stream and the server
    /// says it was told to stop.
    pub(super) async fn resume_peek_stream(&mut self, pane: PaneId) -> Result<(), AppError> {
        let Some(held) = self.peek_streams.get(&pane).copied() else {
            return Ok(());
        };
        if held.live {
            return Ok(());
        }
        self.flow(FlowControl::Resume {
            stream: held.stream,
        })
        .await?;
        self.peek_streams
            .insert(pane, PeekStream { live: true, ..held });
        Ok(())
    }

    /// Forget every peek this connection held.
    ///
    /// The pane ids and the stream ids were both minted by a server that is
    /// gone; the successor issues its own, and a paused stream on a dead socket
    /// is not a thing to resume.
    pub(super) fn forget_peek(&mut self) {
        self.peeked = None;
        self.peek_streams.clear();
    }

    /// Make sure `pane` has a grid stream that is being sent on.
    async fn ensure_peek_stream(&mut self, pane: PaneId) -> Result<(), AppError> {
        if self.peek_streams.contains_key(&pane) {
            return self.resume_peek_stream(pane).await;
        }
        // Bound by the projection: this peek borrows it and owns nothing, so
        // closing the peek leaves a visible pane painting.
        if self.bindings.has_grid(pane) {
            return Ok(());
        }
        // The same claim `bind_visible` presents, for the same reason: a client
        // that holds a complete grid for this pane asks for deltas rather than
        // a keyframe it does not need (`super::reconnect`).
        let generation = self
            .model
            .pane(pane)
            .filter(|cells| cells.complete())
            .map(crate::model::PaneGrid::generation);
        if let Some(reply) = self
            .bind_at(StreamKind::PaneGrid { pane }, generation)
            .await?
        {
            self.bindings.bind_grid(pane, reply.channel);
            self.peek_streams.insert(
                pane,
                PeekStream {
                    stream: reply.stream,
                    live: true,
                },
            );
        }
        Ok(())
    }

    /// Stop the stream the current peek owns, if it owns one the projection is
    /// not also drawing.
    async fn release_peek_stream(&mut self) -> Result<(), AppError> {
        let Some(pane) = self.peeked else {
            return Ok(());
        };
        // A pane the session no longer holds takes its pump with it — the
        // server's feed closes and the pump ends — so there is nothing to pause
        // and nothing to keep a row for.
        if !self.pane_is_in_a_layout(pane) {
            self.peek_streams.remove(&pane);
            return Ok(());
        }
        let Some(held) = self.peek_streams.get(&pane).copied() else {
            return Ok(());
        };
        // Owned but on screen: the projection took it over while the peek was
        // open (the user switched to its workspace), and pausing it now would
        // freeze a pane being watched.
        if !held.live || self.projects(pane) {
            return Ok(());
        }
        self.flow(FlowControl::Pause {
            stream: held.stream,
        })
        .await?;
        self.peek_streams.insert(
            pane,
            PeekStream {
                live: false,
                ..held
            },
        );
        Ok(())
    }

    /// Send one flow-control signal for a stream this client bound.
    async fn flow(&mut self, flow: FlowControl) -> Result<(), AppError> {
        let notification =
            flow_notification(flow).map_err(|_| AppError::BadState("unencodable flow signal"))?;
        self.session.notify(&notification).await?;
        Ok(())
    }

    /// Whether any workspace this client mirrors still holds `pane`.
    ///
    /// The mirror's own answer to "is this pane still in the session". A cached
    /// grid is not that answer — it outlives the pane until the next resync
    /// replaces the mirror — and the layout trees are.
    fn pane_is_in_a_layout(&self, pane: PaneId) -> bool {
        self.model.workspace_ids().any(|id| {
            self.model
                .workspace(id)
                .is_some_and(|ws| ws.layout.contains(pane))
        })
    }
}

/// The notification one flow-control signal rides.
///
/// Pure, and tested as such: the server reads `stream.flow` notifications by
/// method name and decodes the params back into a [`FlowControl`]
/// (`amx-server/src/conn/reader.rs`), and a signal naming the wrong stream, the
/// wrong variant or the wrong method is dropped there in silence — there is no
/// reply for a wrong one to fail.
fn flow_notification(flow: FlowControl) -> Result<Notification, serde_json::Error> {
    Ok(Notification::new(
        FLOW_METHOD,
        Some(serde_json::to_value(flow)?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing a client can get wrong about a release that no reply would
    /// ever tell it: what it put on the wire.
    #[test]
    fn a_pause_names_its_own_stream_on_the_method_the_server_reads() {
        let stream = StreamId::new(7);
        let notification =
            flow_notification(FlowControl::Pause { stream }).expect("encode the signal");

        assert_eq!(notification.method, "stream.flow");
        assert!(
            notification.params.is_some(),
            "a flow signal with no params says nothing",
        );
        let params = notification.params.expect("params");
        let decoded: FlowControl = serde_json::from_value(params).expect("the server's own decode");
        assert_eq!(decoded, FlowControl::Pause { stream });
    }

    #[test]
    fn a_resume_round_trips_the_same_way() {
        let stream = StreamId::new(9);
        let notification =
            flow_notification(FlowControl::Resume { stream }).expect("encode the signal");
        let params = notification.params.expect("params");
        let decoded: FlowControl = serde_json::from_value(params).expect("the server's own decode");
        assert_eq!(decoded, FlowControl::Resume { stream });
    }
}
