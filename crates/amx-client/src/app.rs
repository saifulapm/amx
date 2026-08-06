//! The client's event loop: connect, negotiate, raw mode, render (04 §3).
//!
//! `App` owns process lifecycle end to end — attach, enter the terminal,
//! render, resize, detach — which is the seam a future `crates/amx` binary
//! calls into. It stops short of interpreting a single keystroke: decoding
//! is T14's job. [`App::handle_input`] is the one place T14 attaches (see its
//! doc comment); no other function in this file is meant to change when T14
//! lands. [`Mode`] similarly reserves the arm T15's copy mode fills in.

use std::fmt;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::Path;

use amx_core::{PaneId, Rect, WorkspaceId};
use amx_proto::ClientInfo;

use crate::model::{ClientModel, WorkspaceModel};
use crate::net::{NetError, Session};
use crate::render::{FrameWriter, chrome, grid};
use crate::term::{Sigwinch, TermError, TermSize, TerminalGuard};

/// Interaction mode the client is in (04 §7).
///
/// `Prefix`/`Navigate` are T14's — the modal key layer over `hjkl`
/// focus/resize and the split/swap/move/close verbs. `Copy` is T15's: copy
/// mode over the scrollback cache, entered from `Navigate` and exited on
/// `Esc`. All four variants are declared here, together, so neither task
/// edits this enum — only the `match` arms in their own files that dispatch
/// on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Keys go to the focused pane, byte-identical (D-M0-4: no lossy
    /// `KeyEvent` decode, kitty sequences pass through untouched).
    #[default]
    Terminal,
    /// One-shot prefix key (`ctrl+a`) was just pressed; the next key selects
    /// a verb or enters `Navigate`. T14 seam.
    Prefix,
    /// Sticky modal layer: `hjkl` focus, `HJKL` resize, split/swap/move/close,
    /// `Esc` back to `Terminal`. T14 seam.
    Navigate,
    /// Copy mode over the client-side scrollback cache. T15 seam
    /// (`crates/amx-client/src/copy.rs`); unreachable until T14 wires the
    /// `Navigate` key that enters it.
    Copy,
}

/// An `App` operation failed.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The session socket failed.
    #[error(transparent)]
    Net(#[from] NetError),
    /// A terminal operation failed.
    #[error(transparent)]
    Term(#[from] TermError),
}

/// How long a burst of `SIGWINCH` is allowed to keep arriving before it is
/// treated as settled and acted on once.
pub const RESIZE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(60);

/// The running client: its server connection, its terminal, and what it has
/// drawn so far.
pub struct App<Fd: AsFd, W: Write> {
    session: Session,
    term: TerminalGuard<Fd, W>,
    model: ClientModel,
    writer: FrameWriter,
    mode: Mode,
    pending_resize: Option<TermSize>,
    /// The last computed pane layout, kept across frames so an unchanged
    /// layout costs zero allocation on repaint: `amx_core::Layout::rects`
    /// builds a fresh `Vec` every call (it lives in `amx-core`, out of this
    /// crate's scope), so `repaint` only calls it when `layout_dirty` says
    /// the tree or the content area actually moved, and every other frame
    /// just re-reads this buffer.
    pane_rects: Vec<(PaneId, Rect)>,
    layout_dirty: bool,
    /// How many full repaints this app has done. Exposed for tests; production
    /// callers have no need of it.
    pub repaints: u64,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Connect to `socket`, negotiate, take `fd` into raw mode (writing the
    /// alt-screen sequence to `out`), and build an empty model sized to
    /// `fd`'s current window.
    pub async fn attach(
        socket: &Path,
        fd: Fd,
        out: W,
        client: ClientInfo,
    ) -> Result<Self, AppError> {
        let stream = crate::net::connect(socket).await?;
        let (session, _welcome) = Session::attach(stream, client, None).await?;
        let term = TerminalGuard::enter(fd, out)?;
        let size = term.size()?;
        Ok(Self {
            session,
            term,
            model: ClientModel::new(size.rows, size.cols),
            writer: FrameWriter::new(),
            mode: Mode::default(),
            pending_resize: None,
            pane_rects: Vec::new(),
            layout_dirty: true,
            repaints: 0,
        })
    }

    /// The interaction mode the client is in.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// The underlying control session, for making calls outside the render
    /// loop (workspace/pane verbs).
    pub fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    /// The presentation model, for tests and for T14/T15 to update as their
    /// own calls succeed.
    pub fn model(&mut self) -> &mut ClientModel {
        &mut self.model
    }

    /// Record a `WorkspaceId`, its layout mirror and a pane's server-sized
    /// grid, folding both into the presentation model (the shape every
    /// integration test in this crate drives, in lieu of the event-bus
    /// subscription and bound grid stream that would populate this over the
    /// wire once they exist — see `net`'s module doc for why neither does
    /// yet).
    pub fn adopt_workspace(&mut self, id: WorkspaceId, model: WorkspaceModel) {
        self.model.set_workspace(id, model);
        self.layout_dirty = true;
    }

    /// T14 hook seam.
    ///
    /// Terminal-mode bytes reach here byte-identical, unparsed; a real
    /// implementation recognises the prefix key and `Navigate`'s keys and
    /// forwards everything else to the focused pane. Left `todo!()`
    /// deliberately: T13 owns everything upstream and downstream of this one
    /// call, T14 owns only this function's body plus `input/**`, so no other
    /// part of `app.rs` needs to change when it lands.
    pub fn handle_input(&mut self, bytes: &[u8]) {
        let _ = bytes;
        todo!("T14: mode machine, prefix/navigate keys, kitty passthrough")
    }

    /// Note that a resize happened; does not yet act on it.
    ///
    /// Called once per `SIGWINCH` in [`Self::run`]. A burst of signals
    /// (the terminal emulator itself coalesces some, but not all, depending
    /// on how the user dragged) each overwrite the pending size rather than
    /// queuing, so [`Self::settle_resize`] always acts on the *last* one.
    pub fn note_resize(&mut self, size: TermSize) {
        self.pending_resize = Some(size);
    }

    /// Whether a resize is waiting to be settled.
    #[must_use]
    pub const fn has_pending_resize(&self) -> bool {
        self.pending_resize.is_some()
    }

    /// Act on the pending resize, if any: apply it to the model, report it
    /// once through `report`, and repaint once.
    ///
    /// `report` stands in for `client.viewport` — declared in
    /// `amx_proto::control::client` but not yet a row in the method table
    /// (`net`'s module doc), so there is no live call to make yet. Returns
    /// whether a resize was actually settled, so a caller (and a test) can
    /// tell "nothing was pending" from "one was applied".
    pub fn settle_resize(&mut self, report: &mut impl FnMut(TermSize)) -> bool {
        let Some(size) = self.pending_resize.take() else {
            return false;
        };
        self.model.term = Rect::new(0, 0, size.cols, size.rows);
        self.layout_dirty = true;
        report(size);
        self.repaint();
        true
    }

    /// Redraw everything: chrome and every visible pane's grid.
    ///
    /// Allocation-free once the layout has settled: the only thing that can
    /// grow a buffer here is [`FrameWriter`]'s own `Vec<u8>` reaching a new
    /// high-water mark, which happens at most once per distinct frame size.
    pub fn repaint(&mut self) {
        self.writer.begin_frame();
        if self.layout_dirty {
            let content = self.model.content_area();
            let rects = self.model.pane_rects(content);
            self.pane_rects.clear();
            self.pane_rects.extend_from_slice(&rects);
            self.layout_dirty = false;
        }
        for &(pane, rect) in &self.pane_rects {
            chrome::draw_border(&mut self.writer, rect);
            let inner = chrome::inset(rect);
            if let Some(pane_grid) = self.model.pane(pane) {
                grid::blit(&mut self.writer, pane_grid, inner);
            }
        }
        let status = status_text(&self.model);
        chrome::status_line(
            &mut self.writer,
            self.model.term.h.saturating_sub(1),
            self.model.term.w,
            &status,
        );
        self.repaints += 1;
    }

    /// The bytes the last [`Self::repaint`] produced.
    #[must_use]
    pub fn frame(&self) -> &[u8] {
        self.writer.bytes()
    }

    /// Run until `sigwinch` and the connection both end, resizing on a
    /// debounced `SIGWINCH` and otherwise idle — the loop T14 will extend
    /// with a stdin arm calling [`Self::handle_input`].
    pub async fn run(mut self, mut sigwinch: Sigwinch) -> Result<(), AppError> {
        self.repaint();
        loop {
            tokio::select! {
                signal = sigwinch.recv() => {
                    let Some(()) = signal else { return Ok(()) };
                    let size = self.term.size()?;
                    self.note_resize(size);
                }
                () = tokio::time::sleep(RESIZE_DEBOUNCE), if self.has_pending_resize() => {
                    self.settle_resize(&mut |_size| {});
                }
            }
        }
    }
}

impl<Fd: AsFd, W: Write> fmt::Debug for App<Fd, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("mode", &self.mode)
            .field("repaints", &self.repaints)
            .finish_non_exhaustive()
    }
}

fn status_text(model: &ClientModel) -> String {
    let label = model
        .focused_workspace()
        .and_then(|ws| ws.label.clone())
        .unwrap_or_else(|| "amx".to_owned());
    format!(" {label} ")
}

/// A pane's id together with the rect chrome computed for it — exposed so
/// tests can assert on layout without reaching into `App` internals.
pub type PaneSlot = (PaneId, Rect);
