//! The cells a client caches: one grid per pane, exactly as the server sent it.
//!
//! Split out of [`super`] so each file holds one responsibility — the mirrored
//! *session* (workspaces, panes, labels, agent status, the attention queue)
//! there, the mirrored *cells* here — and so neither grows past the module
//! budget as M2 adds to the first.
//!
//! Nothing in this file knows what a workspace is. A [`PaneGrid`] is the
//! server's own cells at the server's own size, and 04 §3's rule about them is
//! absolute: "non-active clients letterbox/clip the server-sized pane grids
//! inside their own locally-computed chrome". No render path may reflow one.

use amx_core::GridGeneration;
use amx_proto::stream::{Cursor, CursorShape, DamageRect};

pub use amx_proto::stream::Underline;

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

/// The SGR attributes carried on one cell.
///
/// Every attribute `amx_proto::stream::CellStyle` carries
/// (`amx-proto/src/stream/cell.rs:123-144`), in the wire's own vocabulary: the
/// underline is [`Underline`] rather than a boolean, and its colour sits beside
/// the foreground and the background. The decode fills all of them
/// (`stream::cell_of`) and the frame writer emits all of them
/// (`render::FrameWriter::set_attrs`), so nothing `CellStyle` can express is
/// dropped between the socket and the terminal.
///
/// Two reductions are left in the cell model, and neither is an attribute:
///
/// - **A colour arrives resolved or not at all.** The wire carries direct RGB
///   or nothing (`CellRef::foreground`), and a palette-indexed *underline*
///   colour is flattened to nothing a crate earlier, where the palette that
///   would resolve it does not exist (`amx-vt/src/render/rows.rs:387-399`).
///   [`Color::Indexed`] is therefore a colour this client can render and never
///   receives.
/// - **A cell's text is one `char`.** A multi-codepoint grapheme cluster keeps
///   its first scalar (`stream::cell_of`), which is a property of [`Cell`]
///   rather than of these attributes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attrs {
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Underline color. [`Color::Default`] follows the foreground, which is
    /// what the wire's absent underline colour means.
    pub underline_color: Color,
    /// Underline style.
    pub underline: Underline,
    /// Bold.
    pub bold: bool,
    /// Faint.
    pub faint: bool,
    /// Italic.
    pub italic: bool,
    /// Blink.
    pub blink: bool,
    /// Foreground/background swapped.
    pub reverse: bool,
    /// Invisible.
    pub invisible: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// Overlined.
    pub overline: bool,
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
    /// Whether every cell in this grid came from the server.
    ///
    /// False until the first keyframe lands: a blank grid is a placeholder the
    /// model minted so a delta had somewhere to go, and its cells are this
    /// client's invention rather than the pane's. The distinction is load
    /// bearing on a reconnect — presenting a generation asserts "I hold a
    /// **complete** grid at generation G", and a placeholder cannot make that
    /// claim (`app::reconnect`).
    complete: bool,
    /// How many keyframes this grid has absorbed.
    ///
    /// The only externally visible difference between a resumed stream that was
    /// owed nothing and one that had to repaint, and therefore what a reconnect
    /// test asserts on.
    keyframes: u64,
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
            complete: false,
            keyframes: 0,
        }
    }

    /// The generation this grid is at.
    #[must_use]
    pub const fn generation(&self) -> GridGeneration {
        self.generation
    }

    /// Whether every cell here came from the server.
    ///
    /// Only a complete grid may have its generation presented on a reattach.
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.complete
    }

    /// How many keyframes this grid has absorbed.
    #[must_use]
    pub const fn keyframes(&self) -> u64 {
        self.keyframes
    }

    /// Forget that this grid's cells are the server's.
    ///
    /// What a client does when it can no longer vouch for a pane: the cells
    /// stay on screen — a stale screen is better than a blank one — but the
    /// generation stops being presentable, so the next bind opens with a
    /// keyframe instead of delta-only.
    pub const fn doubt(&mut self) {
        self.complete = false;
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
        self.complete = true;
        self.keyframes += 1;
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
