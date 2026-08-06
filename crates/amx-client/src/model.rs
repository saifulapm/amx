//! Client-owned presentation state (04 §3).
//!
//! The client receives the layout tree and status as *state*, not pixels, and
//! draws its own chrome from it. [`ClientModel`] is that state: a mirror of
//! each visible workspace's [`Layout`] (the same pure BSP algebra
//! `amx-core` gives the server — split/close/swap here would be the same
//! tree rewrite there, which is what "presentation, not truth" means in
//! practice) plus one [`PaneGrid`] per pane the client is currently
//! rendering.

use std::collections::HashMap;

use amx_core::{GridGeneration, Layout, PaneId, Rect, WorkspaceId};
use amx_proto::stream::{Cursor, CursorShape, DamageRect};

/// A cell's foreground or background color.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    /// The terminal's default color.
    #[default]
    Default,
    /// One of the 256-color palette entries.
    Indexed(u8),
    /// A direct RGB color.
    Rgb(u8, u8, u8),
}

/// The SGR-ish attributes carried on one cell.
///
/// A minimal stand-in for the server's real per-cell attribute set. The wire
/// type that would carry it end to end
/// (`amx_proto::stream::grid::GridMessage`'s `Cells`) has no encode/decode
/// yet — both bodies are still `todo!()`, and no per-cell byte layout is
/// defined anywhere in the tree — so there is nothing to decode against today.
/// This is only as rich as the blit path and the SGR differ need to prove
/// themselves against; reconciling it with the server's real cell model is
/// follow-up work for whichever task lands that codec.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attrs {
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Foreground/background swapped.
    pub reverse: bool,
}

/// One cell of a pane's grid.
#[derive(Clone, PartialEq, Debug)]
pub struct Cell {
    /// The cell's content. A space for an empty cell.
    pub ch: char,
    /// The cell's attributes.
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            attrs: Attrs::default(),
        }
    }
}

/// One pane's grid as the client has it cached: exactly the server's cells,
/// at the server's own size.
///
/// 04 §3: "Client sizes... non-active clients letterbox/clip the server-sized
/// pane grids inside their own locally-computed chrome" — this type is
/// deliberately never resized by anything other than [`PaneGrid::apply_reset`]
/// answering a generation bump the server announced. Nothing in the render
/// path may stretch or reflow it to fit a client's own rect.
#[derive(Clone, Debug)]
pub struct PaneGrid {
    generation: GridGeneration,
    rows: u16,
    cols: u16,
    cells: Vec<Cell>,
    cursor: Cursor,
}

impl PaneGrid {
    /// A blank grid of `rows` by `cols` cells, at [`GridGeneration::FIRST`].
    #[must_use]
    pub fn blank(rows: u16, cols: u16) -> Self {
        Self {
            generation: GridGeneration::FIRST,
            rows,
            cols,
            cells: vec![Cell::default(); usize::from(rows) * usize::from(cols)],
            cursor: Cursor {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::default(),
                blink: false,
            },
        }
    }

    /// The generation this grid is at.
    #[must_use]
    pub const fn generation(&self) -> GridGeneration {
        self.generation
    }

    /// Rows in the grid.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// Columns in the grid.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// The cursor as of the last applied message.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The cell at `(row, col)`, or `None` outside the grid.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        self.index(row, col).map(|i| &self.cells[i])
    }

    fn index(&self, row: u16, col: u16) -> Option<usize> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        Some(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }

    /// Replace the whole grid with a keyframe.
    ///
    /// The cell buffer is resized only when the shape actually changed —
    /// resize is what bumps a generation in the first place, so this is not a
    /// per-frame cost.
    pub fn apply_reset(
        &mut self,
        generation: GridGeneration,
        rows: u16,
        cols: u16,
        cells: &[Cell],
        cursor: Cursor,
    ) {
        self.generation = generation;
        self.rows = rows;
        self.cols = cols;
        self.cells.clear();
        self.cells.extend_from_slice(cells);
        self.cells
            .resize(usize::from(rows) * usize::from(cols), Cell::default());
        self.cursor = cursor;
    }

    /// Apply an incremental update: overwrite the cells inside `rects`.
    ///
    /// `cells` is packed rect-by-rect, row-major within each rect, matching
    /// the order [`DamageRect`]s are listed in — the same packing
    /// `GridMessage::Delta` documents for the wire.
    pub fn apply_delta(
        &mut self,
        generation: GridGeneration,
        rects: &[DamageRect],
        cells: &[Cell],
        cursor: Cursor,
    ) {
        self.generation = generation;
        self.cursor = cursor;
        let mut at = 0;
        for rect in rects {
            for r in rect.row..rect.row.saturating_add(rect.rows) {
                for c in rect.col..rect.col.saturating_add(rect.cols) {
                    let Some(cell) = cells.get(at) else { return };
                    if let Some(i) = self.index(r, c) {
                        self.cells[i] = cell.clone();
                    }
                    at += 1;
                }
            }
        }
    }

    /// Apply a cursor-only update.
    pub fn apply_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }
}

/// One workspace's layout, mirrored client-side.
#[derive(Clone, Debug)]
pub struct WorkspaceModel {
    /// The workspace's user-visible label, if any.
    pub label: Option<String>,
    /// The BSP layout tree — the exact same pure algebra the server runs.
    pub layout: Layout,
}

/// Whether this client currently drives the size of the panes it can see.
///
/// Mirrors `amx_proto::control::client::SizeAuthority`; kept as a client-local
/// type because nothing on the wire currently declares or negotiates it (the
/// `client.viewport` call the real value would ride is not wired into the
/// control method table yet — see the crate-level gap note in `net`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SizeAuthority {
    /// This client's size sets pane grid sizes while it is the active one.
    Active,
    /// This client never drives pane sizes: it letterboxes/clips instead.
    Observer,
}

/// The client's whole presentation state: every workspace it knows about,
/// every pane grid it is caching, and which one has focus.
#[derive(Debug)]
pub struct ClientModel {
    workspaces: HashMap<WorkspaceId, WorkspaceModel>,
    panes: HashMap<PaneId, PaneGrid>,
    focus_workspace: Option<WorkspaceId>,
    /// The client's own terminal size, chrome included.
    pub term: Rect,
}

impl ClientModel {
    /// An empty model at term size `rows` by `cols`.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            workspaces: HashMap::new(),
            panes: HashMap::new(),
            focus_workspace: None,
            term: Rect::new(0, 0, cols, rows),
        }
    }

    /// Insert or replace a workspace's mirrored layout.
    pub fn set_workspace(&mut self, id: WorkspaceId, model: WorkspaceModel) {
        self.workspaces.insert(id, model);
        self.focus_workspace.get_or_insert(id);
    }

    /// The workspace currently in focus, if any.
    #[must_use]
    pub fn focused_workspace(&self) -> Option<&WorkspaceModel> {
        self.focus_workspace.and_then(|id| self.workspaces.get(&id))
    }

    /// The id of the workspace currently in focus, if any.
    #[must_use]
    pub const fn focused_workspace_id(&self) -> Option<WorkspaceId> {
        self.focus_workspace
    }

    /// Every workspace id the client mirrors, in no particular order.
    pub fn workspace_ids(&self) -> impl Iterator<Item = WorkspaceId> + '_ {
        self.workspaces.keys().copied()
    }

    /// The label of a mirrored workspace, if it has one.
    #[must_use]
    pub fn workspace_label(&self, id: WorkspaceId) -> Option<String> {
        self.workspaces.get(&id).and_then(|ws| ws.label.clone())
    }

    /// Focus `id`. A no-op if it is not a known workspace.
    pub fn focus_workspace(&mut self, id: WorkspaceId) {
        if self.workspaces.contains_key(&id) {
            self.focus_workspace = Some(id);
        }
    }

    /// The pane grid cache for `pane`, creating a blank `rows`x`cols` one if
    /// this is the first the client has heard of it.
    pub fn pane_mut(&mut self, pane: PaneId, rows: u16, cols: u16) -> &mut PaneGrid {
        self.panes
            .entry(pane)
            .or_insert_with(|| PaneGrid::blank(rows, cols))
    }

    /// The cached grid for `pane`, if the client has one.
    #[must_use]
    pub fn pane(&self, pane: PaneId) -> Option<&PaneGrid> {
        self.panes.get(&pane)
    }

    /// Drop a pane's cached grid, e.g. after `pane.close`.
    pub fn remove_pane(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// Drop a workspace the server no longer reports, moving focus off it.
    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        self.workspaces.remove(&id);
        if self.focus_workspace == Some(id) {
            self.focus_workspace = self.workspaces.keys().copied().next();
        }
    }

    /// Keep only the workspaces `keep` admits, with the same focus rule as
    /// [`Self::remove_workspace`].
    pub fn retain_workspaces(&mut self, keep: impl Fn(WorkspaceId) -> bool) {
        self.workspaces.retain(|id, _| keep(*id));
        if let Some(focus) = self.focus_workspace
            && !self.workspaces.contains_key(&focus)
        {
            self.focus_workspace = self.workspaces.keys().copied().next();
        }
    }

    /// Keep only the pane grids `keep` admits.
    pub fn retain_panes(&mut self, keep: impl Fn(PaneId) -> bool) {
        self.panes.retain(|id, _| keep(*id));
    }

    /// The content area chrome leaves for panes: the whole terminal minus one
    /// status line at the bottom.
    #[must_use]
    pub fn content_area(&self) -> Rect {
        Rect::new(
            self.term.x,
            self.term.y,
            self.term.w,
            self.term.h.saturating_sub(1),
        )
    }

    /// Every visible pane's rect within `area`, for the focused workspace.
    #[must_use]
    pub fn pane_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        self.focused_workspace()
            .map(|ws| ws.layout.rects(area))
            .unwrap_or_default()
    }
}
