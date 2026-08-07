//! The parser's end of the double buffer: filling a frame and swapping it in.
//!
//! [`super`] has the value a reader holds and the three properties that make it
//! one — derived, POD, double buffered — and the carry-list argument that this
//! module is the implementation of. Everything here runs on the pane's parser
//! thread and nowhere else.

use std::sync::Arc;

use super::{Cell, Row, Snapshot, SnapshotRef, TextRef};
use crate::enums::{CellWide, Dirty};
use crate::error::Result;
use crate::render::{RenderState, Style};

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
    /// Whether anything has been published yet. The first frame is forced full
    /// regardless of what the library reports, because both buffers start empty
    /// and a damage-only first pass would publish blank rows. Tracked as its own
    /// flag rather than read off `generation == 0`, so a seeded counter
    /// ([`Snapshots::new_at`]) cannot make a fresh buffer look already-painted.
    published: bool,
    generation: u64,
}

impl Snapshots {
    /// A published empty grid of this size.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::new_at(cols, rows, 0)
    }

    /// A published empty grid of this size, whose counter continues from
    /// `generation`.
    ///
    /// The seed a live upgrade needs (`docs/09-m3-plan.md` D-M3-4). A pane that
    /// crosses a handoff is rebuilt around a *fresh* terminal on the successor,
    /// so every counter it carries would otherwise restart at zero — and a
    /// reader that compares publication counters to tell "nothing changed" from
    /// "I have not looked yet" would read the successor's first frames as older
    /// than the ones it already has.
    ///
    /// This is the frame-publication counter, deliberately **not** the grid
    /// generation of 04 §4 (see [`Snapshot::generation`]): the protocol
    /// generation lives on the pane host beside the terminal and is seeded
    /// there. Both are counters a successor continues; they count different
    /// things.
    #[must_use]
    pub fn new_at(cols: u16, rows: u16, generation: u64) -> Self {
        Self {
            front: Arc::new(Snapshot::empty(rows, cols)),
            back: Arc::new(Snapshot::empty(rows, cols)),
            carry: Vec::with_capacity(rows as usize),
            poisoned: false,
            published: false,
            generation,
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
        let first = !self.published;
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
        self.published = true;
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
