//! The client's event loop: connect, negotiate, raw mode, render (04 §3).
//!
//! `App` owns process lifecycle end to end — attach, enter the terminal,
//! render, resize, detach. Decoding lives in [`crate::input`]; here
//! [`App::handle_input`] only turns the machine's actions into their
//! consequences (bytes to the focused pane, control calls, local focus).
//! [`Mode::Copy`]'s keys are owned by `crate::copy`: the `Mode::Copy` arm in
//! `input` dispatches through that module's key table.
//!
//! The module splits by responsibility: this file is the presentation core —
//! model, layout, input consequences — [`wired`] is the live loop over the
//! session socket (frames in, input and calls out), [`overlay`] is the picker
//! and copy-mode surfaces drawn over the panes, and [`status`] is the status
//! line and the cursor a repaint finishes with.

mod overlay;
mod status;
mod wired;

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{Direction, PaneId, Rect, RowRange, WorkspaceId};
use amx_proto::control::{Call, pane as pane_proto};

use crate::cache::Scrollback;
use crate::input::{self, Action, Input, InputEvent};
use crate::model::{ClientModel, WorkspaceModel};
use crate::net::{NetError, Session};
use crate::render::{FrameWriter, chrome, grid};
use crate::term::{TermError, TermSize, TerminalGuard};

pub use overlay::{CopyUi, PickTarget, PickerUi};

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
    /// a verb or enters `Navigate`.
    Prefix,
    /// Sticky modal layer: `hjkl` focus, `HJKL` resize, split/swap/move/close,
    /// `Esc` back to `Terminal`.
    Navigate,
    /// Copy mode over the client-side scrollback cache: entered from
    /// `Navigate` with `c`, keys decoded by `crate::copy`'s table (`y`,
    /// `Esc` and `q` leave it), selection and view in stable-row coordinates.
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
    /// The server's snapshot could not be decoded.
    #[error("malformed session state: {0}")]
    BadState(&'static str),
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
    /// This client's live stream bindings.
    bindings: crate::stream::Bindings,
    /// Per-pane scrollback caches, filled over the history stream.
    caches: HashMap<PaneId, Scrollback>,
    /// The open picker, if any; its bytes bypass the mode machine.
    picker: Option<PickerUi>,
    /// The live copy-mode engine while [`Mode::Copy`] is active.
    copy: Option<CopyUi>,
    /// Bytes owed to the client's own terminal outside the frame (OSC 52).
    emit: Vec<u8>,
    /// History ranges copy mode wants fetched, drained by the wired loop.
    wanted_history: Vec<(PaneId, RowRange)>,
    /// Something changed since the last flush; the wired loop repaints.
    dirty: bool,
    /// The last computed pane layout, kept across frames so an unchanged
    /// layout costs zero allocation on repaint: `amx_core::Layout::rects`
    /// builds a fresh `Vec` every call (it lives in `amx-core`, out of this
    /// crate's scope), so `repaint` only calls it when `layout_dirty` says
    /// the tree or the content area actually moved, and every other frame
    /// just re-reads this buffer.
    pane_rects: Vec<(PaneId, Rect)>,
    layout_dirty: bool,
    /// The focused pane per workspace — client presentation state (04 §3),
    /// used to address input at a pane and to seed navigate's `from`. The
    /// server's own focus (T07's ops) stays canonical for session semantics:
    /// every `hjkl` move is echoed to it through `pane.focus`.
    focus: HashMap<WorkspaceId, PaneId>,
    /// The modal byte machine behind [`Self::handle_input`].
    input: Input,
    /// The rendered status line, cached against the inputs it was built from
    /// (see [`status`]).
    status: status::StatusLine,
    /// How many full repaints this app has done. Exposed for tests; production
    /// callers have no need of it.
    pub repaints: u64,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Wrap a negotiated session and an entered terminal into an app with an
    /// empty model sized to the terminal.
    ///
    /// The tail of [`App::attach`], public so tests can assemble an `App`
    /// from pieces they control.
    pub fn assemble(session: Session, term: TerminalGuard<Fd, W>) -> Result<Self, AppError> {
        let size = term.size()?;
        Ok(Self {
            session,
            term,
            model: ClientModel::new(size.rows, size.cols),
            writer: FrameWriter::new(),
            mode: Mode::default(),
            pending_resize: None,
            bindings: crate::stream::Bindings::new(),
            caches: HashMap::new(),
            picker: None,
            copy: None,
            emit: Vec::new(),
            wanted_history: Vec::new(),
            dirty: true,
            pane_rects: Vec::new(),
            layout_dirty: true,
            focus: HashMap::new(),
            input: Input::new(),
            status: status::StatusLine::default(),
            repaints: 0,
        })
    }

    /// The interaction mode the client is in.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether the picker is open (its bytes bypass the mode machine).
    #[must_use]
    pub const fn picker_open(&self) -> bool {
        self.picker.is_some()
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

    /// The scrollback cache for `pane`, created empty on first touch — the
    /// same seam the history stream fills, exposed for tests.
    pub fn cache_mut(&mut self, pane: PaneId) -> &mut Scrollback {
        self.caches.entry(pane).or_default()
    }

    /// Record a `WorkspaceId`, its layout mirror and a pane's server-sized
    /// grid, folding both into the presentation model (the shape every
    /// integration test in this crate drives, in lieu of the event-bus
    /// subscription and bound grid stream that would populate this over the
    /// wire once they exist — see `net`'s module doc for why neither does
    /// yet).
    pub fn adopt_workspace(&mut self, id: WorkspaceId, model: WorkspaceModel) {
        self.model.set_workspace(id, model);
        // Adopting is a test seam: the adopted mirror is what the test wants
        // rendered, ahead of whatever the live attach already folded.
        self.model.focus_workspace(id);
        self.layout_dirty = true;
    }

    /// Feed raw terminal bytes through the modal input layer (04 §7).
    ///
    /// Bytes arrive byte-identical and unparsed; the machine in
    /// [`crate::input`] recognises only the prefix key, mode keys and SGR
    /// mouse report extents, and everything else forwards to the focused
    /// pane untouched — kitty sequences included. What must leave the client
    /// goes through `sink` (the same shape T13 gave `settle_resize`): bytes
    /// for the focused pane's raw I/O stream, and control calls for the T04
    /// method table. `pane.focus` is emitted fire-and-forget *after* local
    /// focus has moved — the server's focus stays canonical and the echo
    /// keeps the two from silently diverging.
    pub fn handle_input(&mut self, bytes: &[u8], sink: &mut impl FnMut(InputEvent<'_>)) {
        if self.picker.is_some() {
            self.picker_input(bytes, sink);
            return;
        }
        if self.mode == Mode::Copy {
            self.copy_input(bytes);
            return;
        }
        let mut actions = self.input.take_scratch();
        self.mode = self.input.feed(self.mode, bytes, &mut actions);
        for &action in &actions {
            self.apply_action(action, bytes, sink);
        }
        self.input.put_scratch(actions);
        if self.mode == Mode::Copy {
            // Navigate's `c` was consumed by the machine; the engine opens
            // here, over the focused pane's cache, or not at all.
            self.enter_copy();
        }
    }

    /// The pane input currently addresses: this workspace's recorded focus,
    /// self-healing to the layout's first pane when nothing was recorded yet
    /// or the recorded pane has since left the mirror.
    pub fn focused_pane(&mut self) -> Option<PaneId> {
        let ws = self.model.focused_workspace_id()?;
        let layout = &self.model.focused_workspace()?.layout;
        if let Some(&pane) = self.focus.get(&ws)
            && layout.contains(pane)
        {
            return Some(pane);
        }
        let first = layout.panes().first().copied()?;
        self.focus.insert(ws, first);
        Some(first)
    }

    /// The input machine's own state, for the pane-state path (and tests) to
    /// record per-pane mouse reporting on.
    pub fn input(&mut self) -> &mut Input {
        &mut self.input
    }

    /// Turn one decoded [`Action`] into its consequence: bytes to the pane,
    /// a control call, or a local focus change.
    fn apply_action(
        &mut self,
        action: Action,
        bytes: &[u8],
        sink: &mut impl FnMut(InputEvent<'_>),
    ) {
        // The two verbs that need no focused pane come first: a client must be
        // able to leave (or reach the picker) even in an empty session.
        match action {
            Action::Detach => {
                sink(InputEvent::Detach);
                return;
            }
            Action::Picker => {
                self.open_picker();
                return;
            }
            _ => {}
        }
        let Some(ws) = self.model.focused_workspace_id() else {
            return;
        };
        let Some(pane) = self.focused_pane() else {
            return;
        };
        match action {
            Action::Forward { start, end } => sink(InputEvent::Forward {
                pane,
                bytes: &bytes[start..end],
            }),
            Action::Mouse { start, end } => {
                if self.input.mouse_enabled(pane) {
                    sink(InputEvent::Forward {
                        pane,
                        bytes: &bytes[start..end],
                    });
                }
            }
            Action::CarriedMouse => {
                if self.input.mouse_enabled(pane) {
                    sink(InputEvent::Forward {
                        pane,
                        bytes: self.input.carried(),
                    });
                }
            }
            Action::CarriedBytes => sink(InputEvent::Forward {
                pane,
                bytes: self.input.carried(),
            }),
            Action::Focus(dir) => {
                if let Some(next) = self.neighbour_of(pane, dir) {
                    self.focus.insert(ws, next);
                    self.repaint();
                }
                sink(InputEvent::Call(Call::PaneFocus(pane_proto::FocusParams {
                    workspace: ws,
                    direction: input::wire_direction(dir),
                })));
            }
            // Deliberately no local layout change: the mirror is server truth
            // with no mutable accessor, so the resize round-trips and the
            // repaint follows the updated layout state (04 §3).
            Action::Resize(dir) => sink(InputEvent::Call(Call::PaneResize(
                pane_proto::ResizeParams {
                    pane,
                    direction: input::wire_direction(dir),
                    delta: input::RESIZE_STEP,
                },
            ))),
            Action::Split(direction) => {
                sink(InputEvent::Call(Call::PaneSplit(pane_proto::SplitParams {
                    pane,
                    direction,
                    command: None,
                    cwd: None,
                })))
            }
            Action::Swap(dir) => {
                if let Some(with) = self.neighbour_of(pane, dir) {
                    sink(InputEvent::Call(Call::PaneSwap(pane_proto::SwapParams {
                        pane,
                        with,
                    })));
                }
            }
            // Interim target until T15's picker chooses one (04 §7): the
            // next workspace in id order, wrapping. No other workspace means
            // nowhere to move to.
            Action::MovePane => {
                if let Some(to) = self.next_workspace(ws) {
                    sink(InputEvent::Call(Call::PaneMove(pane_proto::MoveParams {
                        pane,
                        to,
                    })));
                }
            }
            Action::Close => sink(InputEvent::Call(Call::PaneClose(pane_proto::CloseParams {
                pane,
            }))),
            Action::Zoom => sink(InputEvent::Call(Call::PaneZoom(pane_proto::ZoomParams {
                pane,
            }))),
            // Handled above, before the focused-pane requirement.
            Action::Detach | Action::Picker => {}
            // Interim numbering until short numbers reach the client: n-th
            // pane in layout order. Local-only — `pane.focus` speaks
            // directions, and a direct set-focus core op does not exist yet.
            Action::Jump(n) => {
                let target = self
                    .model
                    .focused_workspace()
                    .and_then(|w| w.layout.panes().get(usize::from(n) - 1).copied());
                if let Some(target) = target
                    && target != pane
                {
                    self.focus.insert(ws, target);
                    self.repaint();
                }
            }
        }
    }

    /// The focused workspace's geometric neighbour of `pane`, over the same
    /// content area chrome tiles.
    fn neighbour_of(&self, pane: PaneId, dir: Direction) -> Option<PaneId> {
        let area = self.model.content_area();
        self.model
            .focused_workspace()?
            .layout
            .neighbour(area, pane, dir)
    }

    /// The workspace after `current` in id order, wrapping; `None` if it is
    /// the only one.
    fn next_workspace(&self, current: WorkspaceId) -> Option<WorkspaceId> {
        self.model
            .workspace_ids()
            .filter(|&id| id > current)
            .min()
            .or_else(|| self.model.workspace_ids().filter(|&id| id != current).min())
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
        self.draw_overlays();
        self.draw_status();
        self.place_cursor();
        self.repaints += 1;
    }

    /// The bytes the last [`Self::repaint`] produced.
    #[must_use]
    pub fn frame(&self) -> &[u8] {
        self.writer.bytes()
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

/// A pane's id together with the rect chrome computed for it — exposed so
/// tests can assert on layout without reaching into `App` internals.
pub type PaneSlot = (PaneId, Rect);
