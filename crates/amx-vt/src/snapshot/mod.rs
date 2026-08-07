//! The derived POD cell snapshot.
//!
//! 04 §3: the parser copies damaged visible rows plus the cursor into a
//! "derived, double-buffered POD cell snapshot published lock-free for render
//! and tier-2 detection". Three properties follow, and they are the reason this
//! type exists at all rather than readers borrowing the terminal:
//!
//! - It is **derived**. The live grid stays exclusively owned by the parser
//!   thread; a snapshot is a copy, so reading one can never contend with the
//!   parser on a pane-state mutex.
//! - It is **POD**. No pointers into the FFI object survive publication, so a
//!   reader holding a snapshot cannot observe the terminal mutating underneath
//!   it.
//! - It is **double buffered**. The parser fills the back buffer and publishes
//!   it; readers keep whichever buffer they were handed for as long as they
//!   hold their [`SnapshotRef`].
//!
//! ## Why double buffering needs a carry list
//!
//! With two buffers, the frame being written is the one published two frames
//! ago, so the rows that changed in the *previous* frame are stale in it even
//! though the render state no longer reports them as dirty. [`Snapshots`]
//! therefore rewrites `damage(this frame) ∪ damage(previous frame)` and reports
//! only `damage(this frame)`. Skipping the union is the classic double-buffer
//! bug: every other frame would show one row of the frame before last.
//!
//! ## Text storage
//!
//! Cells are fixed size and hold a `(start, len)` slice reference into their
//! own row's UTF-8 arena, not a pointer and not an inline array. That keeps a
//! cell at 16 bytes whatever a grapheme cluster costs, keeps the whole
//! structure trivially copyable and serialisable for T09's damage encoder, and
//! lets a row be refilled by clearing two `Vec`s that keep their capacity — so
//! a steady-state frame allocates nothing.
//!
//! ## The text view
//!
//! [`Row::line`] serves that same arena as a `&str`, and [`Snapshot::tail`]
//! walks the last rows of the grid. Together they are what screen detection and
//! `pane.read` read: a borrow per row, no concatenation, no allocation on a
//! path that runs once per damage batch.
//!
//! The arena is written to be exactly the row as painted, which costs one rule
//! in [`copy_row`]: a cell holding no grapheme cluster contributes a single
//! space, because that is what the renderer draws for it, and a rule matching
//! `"foo   bar"` must not match a row where those two words sit in different
//! columns. The exception is the second column of a wide character
//! ([`CellWide::SpacerTail`]), whose cluster the *first* column already
//! contributed. So a line is one `char` per printed character and not one per
//! column, and a trailing run of blank columns is a trailing run of spaces —
//! callers that do not want them use `line().trim_end()`.
//!
//! The view is the *visible grid*. amx's scrollback lives client-side (04 §3),
//! so the server's snapshot is the live bottom of the terminal by construction,
//! and a detector reading it can never be looking at a scrolled-away viewport —
//! the anchoring problem herdr solves with a dedicated buffer does not exist
//! here.
//!
//! ## Where the two halves live
//!
//! This file is the published *value* — the cells, the rows, the frame a
//! reader holds — and [`publish`] is the parser's end of the double buffer,
//! the thing that fills one and swaps it in. They change for different reasons:
//! the value when a cell learns a new attribute, the publisher when the copy
//! discipline does. W03 split them when M3's generation seed pushed the one
//! file past the soft budget.

mod publish;

use std::sync::Arc;

use crate::enums::CellWide;
use crate::render::{Cursor, Rgb, Style};

pub use publish::Snapshots;

/// A reader's handle on a published [`Snapshot`].
///
/// Shared, refcounted and read-only: publication is an `Arc` swap, so a reader
/// never blocks the parser and the parser never waits for a reader.
pub type SnapshotRef = Arc<Snapshot>;

/// Where a cell's text lives inside its row's arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct TextRef {
    start: u32,
    len: u16,
}

/// One cell of a published grid.
///
/// Plain data: no pointers, no allocation, `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    /// The cell's grapheme cluster, addressed inside [`Row::text`].
    pub text: TextRef,
    /// Narrow, wide, or a spacer that must not be drawn.
    pub wide: CellWide,
    /// Resolved foreground; `None` means the frame default.
    pub foreground: Option<Rgb>,
    /// Resolved background; `None` means the frame default.
    pub background: Option<Rgb>,
    /// SGR attributes.
    pub style: Style,
}

/// One row of a published grid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    cells: Vec<Cell>,
    text: Vec<u8>,
    wrapped: bool,
}

impl Row {
    /// The row's cells, left to right.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The UTF-8 bytes a cell's [`TextRef`] addresses.
    #[must_use]
    pub fn text(&self, cell: &Cell) -> &[u8] {
        let start = cell.text.start as usize;
        let end = start + cell.text.len as usize;
        self.text.get(start..end).unwrap_or_default()
    }

    /// The row as painted, left to right, blank columns as spaces.
    ///
    /// A borrow of the row's own arena: calling this allocates nothing and
    /// copies nothing, which is the property the detection path (one call per
    /// row per damage batch) is built on.
    ///
    /// One printed character is one `char`, so a wide character counts once
    /// even though it covers two columns, and a grapheme cluster with combining
    /// marks counts once however many codepoints it carries. Blank columns are
    /// spaces — including trailing ones, which `trim_end` removes.
    #[must_use]
    pub fn line(&self) -> &str {
        // The arena only ever receives grapheme clusters the library encoded as
        // UTF-8 and ASCII spaces this module pushes, so the check always
        // succeeds. It is a check rather than an assumption because the bytes
        // arrive across FFI, and a row that somehow failed it should read as
        // empty, not abort a session.
        std::str::from_utf8(&self.text).unwrap_or_default()
    }

    /// Whether this row soft-wraps into the next.
    #[must_use]
    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    fn clear(&mut self) {
        self.cells.clear();
        self.text.clear();
        self.wrapped = false;
    }
}

/// A published, immutable copy of a pane's visible grid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    cols: u16,
    rows: Vec<Row>,
    damage: Vec<u16>,
    cursor: Cursor,
    generation: u64,
}

impl Snapshot {
    /// An empty snapshot of a grid this size.
    #[must_use]
    pub fn empty(rows: u16, cols: u16) -> Self {
        let mut snapshot = Self::default();
        snapshot.reshape(cols, rows);
        snapshot
    }

    /// Rows in the snapshotted grid.
    #[must_use]
    pub fn rows(&self) -> u16 {
        // The row count is bounded by the terminal's u16 row count.
        u16::try_from(self.rows.len()).unwrap_or(u16::MAX)
    }

    /// Columns in the snapshotted grid.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// The rows, top to bottom.
    #[must_use]
    pub fn grid(&self) -> &[Row] {
        &self.rows
    }

    /// One row, or `None` if the index is past the bottom.
    #[must_use]
    pub fn row(&self, index: u16) -> Option<&Row> {
        self.rows.get(index as usize)
    }

    /// The last `count` rows, top to bottom.
    ///
    /// Fewer if the grid is shorter; none if `count` is zero. The bottom of the
    /// grid is the bottom of the *live* terminal — scrollback is client-side
    /// (04 §3) and never enters a snapshot — so this is the region a screen
    /// rule means by "the last few lines", with no anchoring to get wrong.
    pub fn tail(&self, count: u16) -> impl DoubleEndedIterator<Item = &Row> + ExactSizeIterator {
        let start = self.rows.len().saturating_sub(count as usize);
        self.rows[start..].iter()
    }

    /// The indices of the rows that changed since the previous published
    /// snapshot, ascending.
    ///
    /// After a resize or any other full invalidation this is every row.
    #[must_use]
    pub fn damage(&self) -> &[u16] {
        &self.damage
    }

    /// The cursor at the moment the snapshot was taken.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// A counter incremented on every publication.
    ///
    /// Readers compare it to tell "nothing changed" from "I have not looked
    /// yet"; it is not a grid generation in the 04 §4 protocol sense.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Resize the buffers, dropping content that no longer fits.
    fn reshape(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows.resize_with(rows as usize, Row::default);
        for row in &mut self.rows {
            row.clear();
        }
    }
}
