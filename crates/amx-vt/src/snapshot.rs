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

use std::sync::Arc;

use crate::enums::{CellWide, Dirty};
use crate::error::Result;
use crate::render::{Cursor, RenderState, Rgb, Style};

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

/// The parser's end of the double buffer.
///
/// Owned by the pane's parser thread, which is the only thing that ever writes
/// a snapshot. Readers get [`SnapshotRef`]s from [`Snapshots::latest`].
#[derive(Debug)]
pub struct Snapshots {
    front: SnapshotRef,
    /// The other half of the double buffer. It is written in place through
    /// [`Arc::get_mut`], so a steady-state frame allocates nothing at all — not
    /// even the `Arc`. If a reader still holds it, it is given up and a fresh
    /// buffer taken for that frame rather than blocking the parser, which is
    /// the trade 04 §3's "lock-free for render" asks for.
    back: SnapshotRef,
    /// Rows written into the *other* buffer last frame, and therefore stale in
    /// the one about to be filled.
    carry: Vec<u16>,
    /// Set when a publish failed partway. A failed pass may have cleared
    /// library dirty flags and copied only some rows, so nothing incremental
    /// can be trusted: the next publish is forced to a full frame.
    poisoned: bool,
    generation: u64,
}

impl Snapshots {
    /// A published empty grid of this size.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            front: Arc::new(Snapshot::empty(rows, cols)),
            back: Arc::new(Snapshot::empty(rows, cols)),
            carry: Vec::with_capacity(rows as usize),
            poisoned: false,
            generation: 0,
        }
    }

    /// The most recently published snapshot.
    #[must_use]
    pub fn latest(&self) -> SnapshotRef {
        Arc::clone(&self.front)
    }

    /// Copy the damaged rows of `render` into the back buffer and publish it.
    ///
    /// `render` must already have been updated from the terminal; this reads
    /// render-state-owned memory only, which is the half of the two-phase
    /// contract that makes the copy cheap.
    ///
    /// Both layers of dirty state are cleared on the way through, because
    /// `render.h:65` puts that on the caller and clearing one does not clear
    /// the other.
    ///
    /// # Errors
    ///
    /// Propagates any library failure while reading the render state. A failed
    /// publish leaves the published frame untouched and poisons the next one
    /// to a full frame: the failed pass may already have cleared dirty flags
    /// the library will never set again, so incremental damage can no longer
    /// be trusted.
    pub fn publish(&mut self, render: &mut RenderState) -> Result<()> {
        let result = self.publish_frame(render);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn publish_frame(&mut self, render: &mut RenderState) -> Result<()> {
        let cols = render.cols()?;
        let rows = render.rows()?;
        let cursor = render.cursor()?;
        let frame_dirty = render.dirty()?;
        let first = self.generation == 0;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        let (back, reused) = Self::back_mut(&mut self.back);
        let reshaped = back.cols != cols || back.rows.len() != rows as usize;
        if reshaped {
            back.reshape(cols, rows);
        }
        // A fresh or reshaped buffer holds nothing worth keeping, and a full
        // frame invalidates everything the library had. The very first frame is
        // forced full rather than trusting the library to report it that way:
        // both buffers start empty, so anything less would publish blank rows.
        // A poisoned pass is full for the same reason a fresh buffer is:
        // whatever the failed publish left behind cannot be described by a
        // damage list.
        let full = self.poisoned || first || !reused || reshaped || frame_dirty == Dirty::Full;

        back.damage.clear();
        let mut index: u16 = 0;
        let mut iterator = render.rows_iter()?;
        while let Some(mut row) = iterator.next_row() {
            if index as usize >= rows as usize {
                break;
            }
            let dirty = row.dirty()?;
            if dirty || full {
                back.damage.push(index);
            }
            if (dirty || full || self.carry.contains(&index))
                && let Some(target) = back.rows.get_mut(index as usize)
            {
                copy_row(&mut row, target)?;
            }
            // The library sets dirty flags and never clears them; leaving one
            // set would report the row as damaged forever (render.h:65).
            row.set_dirty(false)?;
            index = index.wrapping_add(1);
        }
        render.set_dirty(Dirty::Clean)?;

        back.cursor = cursor;
        back.generation = generation;
        // Only a completed pass may rewrite the carry: an early error return
        // above keeps the previous frame's rows in it, and the poison flag
        // makes the next pass full regardless.
        self.carry.clear();
        self.carry.extend_from_slice(&back.damage);

        std::mem::swap(&mut self.front, &mut self.back);
        self.poisoned = false;
        Ok(())
    }

    /// Borrow the back buffer, replacing it first if a reader still holds it.
    ///
    /// The flag says whether the buffer that came back is the one written two
    /// frames ago (so its untouched rows are worth keeping) or a fresh one.
    fn back_mut(back: &mut SnapshotRef) -> (&mut Snapshot, bool) {
        let reused = Arc::get_mut(back).is_some();
        if !reused {
            *back = Arc::new(Snapshot::default());
        }
        #[allow(
            clippy::expect_used,
            reason = "invariant: a shared Arc was just replaced with a sole-owner one"
        )]
        let buffer = Arc::get_mut(back).expect("the back buffer is solely owned");
        (buffer, reused)
    }
}

/// Copy one render-state row into a snapshot row.
fn copy_row(row: &mut crate::render::Row<'_>, target: &mut Row) -> Result<()> {
    target.wrapped = row.wrapped()?;
    target.cells.clear();
    target.text.clear();

    let mut cells = row.cells()?;
    while let Some(cell) = cells.next_cell() {
        let start = target.text.len();
        let len = cell.text(&mut target.text)?;
        let wide = cell.wide()?;
        if len == 0 && wide != CellWide::SpacerTail {
            // A column the application never wrote holds no cluster, and the
            // renderer paints it blank. The arena carries that blank as a space
            // so `Row::line` reads as the row looks; the cell's own `TextRef`
            // stays empty, so the wire encoder still sends nothing for it and
            // this is invisible to every other reader. A spacer tail is skipped
            // because the wide cluster in the column before it already covers
            // this one.
            target.text.push(b' ');
        }
        target.cells.push(Cell {
            text: TextRef {
                // Both fits are structural: a row's text is bounded by
                // cols × the longest cluster, far inside these types.
                start: u32::try_from(start).unwrap_or(u32::MAX),
                len: u16::try_from(len).unwrap_or(u16::MAX),
            },
            wide,
            foreground: cell.foreground()?,
            background: cell.background()?,
            style: if cell.has_styling()? {
                cell.style()?
            } else {
                Style::default()
            },
        });
    }
    Ok(())
}

// Inline rather than in `tests/`: the poison flag is exactly the state a
// failed publish leaves behind, and only this module can produce that state
// without an injectable library failure.
#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, reason = "test")]

    use super::*;
    use crate::Effects;
    use crate::terminal::{Terminal, TerminalOptions};

    fn frame(cols: u16, rows: u16) -> (Terminal, RenderState, Snapshots) {
        let terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 0,
        })
        .expect("a terminal");
        let render = RenderState::new().expect("a render state");
        (terminal, render, Snapshots::new(cols, rows))
    }

    fn write_and_publish(
        terminal: &mut Terminal,
        render: &mut RenderState,
        snapshots: &mut Snapshots,
        bytes: &[u8],
    ) {
        let mut effects = Effects::new();
        terminal.write(bytes, &mut effects);
        render.update(terminal).expect("an update");
        snapshots.publish(render).expect("a publish");
    }

    #[test]
    fn a_poisoned_publish_recovers_with_a_full_frame() {
        let (mut terminal, mut render, mut snapshots) = frame(10, 4);

        // Park the cursor first (moving it dirties the rows it crosses), then
        // establish the steady state: a single-row change publishes as a
        // single-row damage list.
        write_and_publish(&mut terminal, &mut render, &mut snapshots, b"\x1b[3;1H");
        write_and_publish(&mut terminal, &mut render, &mut snapshots, b"xy");
        assert_eq!(
            snapshots.latest().damage(),
            &[2],
            "the baseline must be incremental for the poison assertion to mean anything"
        );

        // A publish that fails partway leaves this flag behind; the next pass
        // must republish every row, because the failed one may already have
        // consumed dirty flags the library will never set again.
        snapshots.poisoned = true;
        write_and_publish(&mut terminal, &mut render, &mut snapshots, b"z");
        assert_eq!(
            snapshots.latest().damage(),
            &[0, 1, 2, 3],
            "a poisoned publish must be a full frame"
        );

        // The poison does not stick: the frame after the recovery is
        // incremental again.
        write_and_publish(&mut terminal, &mut render, &mut snapshots, b"w");
        assert_eq!(snapshots.latest().damage(), &[2]);
    }
}
