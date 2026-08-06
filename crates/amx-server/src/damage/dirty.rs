//! The accumulated dirty-region set: what a stalled client actually costs.
//!
//! 04 §4: "Per client, per visible pane, the server keeps an accumulated
//! dirty-region set (rects + grid generation), **not a queue of deltas**. When
//! the client's writer is not ready, new damage coalesces into that set."
//!
//! The whole flow-control argument rests on this type's size being a function
//! of the *grid*, never of how long a client has been stalled or how much
//! output arrived while it was. So the set is a fixed-width row bitmap: one bit
//! per visible row, allocated once at the pane's height and reused. Marking the
//! same row a million times costs a million `|=` operations and zero bytes.
//!
//! Rows rather than arbitrary rectangles because rows are the granularity the
//! damage actually arrives at — [`amx_vt::Snapshot::damage`] reports row
//! indices, since libghostty-vt's dirty tracking is per row (`render.h:60`).
//! Inventing sub-row rectangles here would mean tracking column extents nobody
//! ever reports, so [`DirtySet::rects`] emits full-width rects and coalesces
//! runs of adjacent rows into one, which is both the honest shape of the damage
//! and fewer rects on the wire.

use amx_proto::stream::DamageRect;

/// Bits in one word of the bitmap.
const BITS: u16 = u64::BITS as u16;

/// Rows that changed and have not been sent yet, for one client × one pane.
///
/// Fixed size: `ceil(rows / 64)` words plus two counters, whatever happens to
/// the pane. That is the bound the flow-control test asserts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirtySet {
    rows: u16,
    cols: u16,
    words: Vec<u64>,
    marked: u16,
}

impl DirtySet {
    /// An empty set for a grid this size.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        let mut set = Self::default();
        set.reshape(rows, cols);
        set
    }

    /// Rows in the grid this set covers.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// Columns in the grid this set covers.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// Resize the set, dropping whatever was pending.
    ///
    /// Returns `true` if the shape actually changed — the caller owes a
    /// keyframe in that case, because a delta whose rects were computed against
    /// the old geometry cannot be applied to the new one.
    pub fn reshape(&mut self, rows: u16, cols: u16) -> bool {
        if self.rows == rows && self.cols == cols {
            return false;
        }
        self.rows = rows;
        self.cols = cols;
        self.words.clear();
        self.words.resize(words_for(rows), 0);
        self.marked = 0;
        true
    }

    /// Mark one row damaged. Out-of-range rows are ignored.
    pub fn mark(&mut self, row: u16) {
        if row >= self.rows {
            return;
        }
        let (word, bit) = (usize::from(row / BITS), row % BITS);
        let Some(slot) = self.words.get_mut(word) else {
            return;
        };
        let mask = 1_u64 << bit;
        if *slot & mask == 0 {
            *slot |= mask;
            self.marked += 1;
        }
    }

    /// Mark the whole grid damaged.
    ///
    /// Used whenever the incremental story breaks: a resize, a generation
    /// change, or a published frame we never saw (see
    /// [`GridStream`](crate::damage::GridStream)). Every one of those makes the
    /// per-row damage report meaningless, and the only safe reading is "all of
    /// it".
    pub fn mark_all(&mut self) {
        for word in &mut self.words {
            *word = u64::MAX;
        }
        self.marked = self.rows;
        self.trim();
    }

    /// Forget one row, because it has just been sent.
    pub fn unmark(&mut self, row: u16) {
        if row >= self.rows {
            return;
        }
        let (word, bit) = (usize::from(row / BITS), row % BITS);
        let Some(slot) = self.words.get_mut(word) else {
            return;
        };
        let mask = 1_u64 << bit;
        if *slot & mask != 0 {
            *slot &= !mask;
            self.marked -= 1;
        }
    }

    /// Forget everything pending, keeping the allocation.
    pub fn clear(&mut self) {
        for word in &mut self.words {
            *word = 0;
        }
        self.marked = 0;
    }

    /// Whether row `row` is pending.
    #[must_use]
    pub fn contains(&self, row: u16) -> bool {
        if row >= self.rows {
            return false;
        }
        self.words
            .get(usize::from(row / BITS))
            .is_some_and(|word| word & (1_u64 << (row % BITS)) != 0)
    }

    /// Whether nothing is pending.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.marked == 0
    }

    /// How many rows are pending.
    #[must_use]
    pub const fn marked(&self) -> u16 {
        self.marked
    }

    /// Whether every row is pending.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.marked == self.rows && self.rows != 0
    }

    /// Write the pending rows into `out` as full-width rects, top to bottom.
    ///
    /// `out` is cleared and refilled rather than returned, so a steady stream of
    /// deltas reuses one buffer. Adjacent rows are merged into a single rect:
    /// the common case under heavy output is a scrolled screen, which is every
    /// row and therefore exactly one rect.
    pub fn rects(&self, out: &mut Vec<DamageRect>) {
        out.clear();
        let mut run: Option<(u16, u16)> = None;
        for row in 0..self.rows {
            if self.contains(row) {
                match &mut run {
                    Some((_, len)) => *len += 1,
                    none => *none = Some((row, 1)),
                }
            } else if let Some((start, len)) = run.take() {
                out.push(self.rect(start, len));
            }
        }
        if let Some((start, len)) = run.take() {
            out.push(self.rect(start, len));
        }
    }

    /// Bytes this set holds on the heap.
    ///
    /// Part of the accounting the bounded-memory test asserts against, and the
    /// number that makes 04 §4's "cost is O(dirty bookkeeping) per client"
    /// checkable rather than aspirational.
    #[must_use]
    pub fn accounted_bytes(&self) -> usize {
        self.words.capacity() * size_of::<u64>()
    }

    fn rect(&self, row: u16, rows: u16) -> DamageRect {
        DamageRect {
            row,
            col: 0,
            rows,
            cols: self.cols,
        }
    }

    /// Clear the bits past the last row, which [`Self::mark_all`] sets.
    fn trim(&mut self) {
        let capacity = words_for(self.rows) * usize::from(BITS);
        let spare = u32::try_from(capacity - usize::from(self.rows)).unwrap_or(u64::BITS);
        if spare == 0 || spare >= u64::BITS {
            return;
        }
        if let Some(last) = self.words.last_mut() {
            *last &= u64::MAX >> spare;
        }
    }
}

/// Words needed to hold one bit per row.
fn words_for(rows: u16) -> usize {
    usize::from(rows).div_ceil(usize::from(BITS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_the_same_row_twice_counts_once() {
        let mut set = DirtySet::new(24, 80);
        set.mark(3);
        set.mark(3);
        assert_eq!(set.marked(), 1);
    }

    #[test]
    fn adjacent_rows_coalesce_into_one_rect() {
        let mut set = DirtySet::new(10, 40);
        for row in [1, 2, 3, 7] {
            set.mark(row);
        }
        let mut rects = Vec::new();
        set.rects(&mut rects);
        assert_eq!(
            rects,
            vec![
                DamageRect {
                    row: 1,
                    col: 0,
                    rows: 3,
                    cols: 40
                },
                DamageRect {
                    row: 7,
                    col: 0,
                    rows: 1,
                    cols: 40
                },
            ]
        );
    }

    #[test]
    fn mark_all_sets_exactly_the_rows_that_exist() {
        let mut set = DirtySet::new(70, 80);
        set.mark_all();
        assert_eq!(set.marked(), 70);
        assert!(set.is_full());
        assert!(set.contains(69));
        assert!(!set.contains(70));

        let mut rects = Vec::new();
        set.rects(&mut rects);
        assert_eq!(rects.len(), 1, "one run covering the whole grid");
        assert_eq!(rects[0].rows, 70);
    }

    #[test]
    fn size_does_not_grow_with_the_amount_of_damage() {
        let mut set = DirtySet::new(24, 80);
        let empty = set.accounted_bytes();
        for _ in 0..100_000 {
            for row in 0..24 {
                set.mark(row);
            }
        }
        assert_eq!(
            set.accounted_bytes(),
            empty,
            "a dirty set is sized by the grid, never by the flood"
        );
    }
}
