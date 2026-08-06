//! Serving a range of history, in chunks.
//!
//! 04 §4: "the connection writer has strict channel priority (control > grid
//! deltas > history/bulk), and history transfers are chunked — a big scrollback
//! fetch can never head-of-line-block a keystroke's response". A chunk is
//! therefore a yield point, which makes two properties load-bearing:
//!
//! - the packing is self-delimiting, so concatenating the chunks of a range
//!   gives byte-for-byte what serving the range in one piece would have; and
//! - every chunk is re-checked against the pane's *current* floor and head, so
//!   a transfer that the scrollback outruns is refused where it stops rather
//!   than quietly serving whatever rows moved into those offsets.
//!
//! Reads resolve a history point per row, which never touches the viewport
//! (R5, settled by the T12 spike), and cost ~3 µs a row — a bulk command on the
//! parser thread, never anything near the frame path.

use amx_core::{RowId, RowRange};
use amx_vt::Terminal;

use super::tracker::HistoryTracker;
use crate::actor::{HistoryError, HistoryRows};

/// Rows per chunk when the caller has no size of its own.
///
/// Small enough that a chunk is a real yield point on a slow link, large enough
/// that an 80-column row's framing overhead stays noise.
pub const CHUNK_ROWS: u64 = 256;

/// A chunked read in progress.
///
/// Holds no borrow on the pane, so a transfer can be suspended between chunks
/// and resumed on the next turn of the parser's command loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HistoryChunks {
    next: u64,
    last: u64,
    rows: u64,
}

impl HistoryChunks {
    /// Whether any rows are left to serve.
    #[must_use]
    pub const fn more(&self) -> bool {
        self.next <= self.last
    }

    /// Serve the next chunk into `out`, appending to whatever is there.
    ///
    /// Returns the rows this chunk carries, or `None` once the range is done.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Evicted`] or [`HistoryError::NotCommitted`] if the pane
    /// moved out from under the transfer, [`HistoryError::Terminal`] if the row
    /// could not be read.
    pub fn next_chunk(
        &mut self,
        tracker: &mut HistoryTracker,
        terminal: &Terminal,
        out: &mut Vec<u8>,
    ) -> Option<Result<RowRange, HistoryError>> {
        if !self.more() {
            return None;
        }
        let last = self.last.min(self.next + self.rows - 1);
        let range = RowRange::new(RowId::from_raw(self.next), RowId::from_raw(last));
        Some(match serve(tracker, terminal, range, out) {
            Ok(()) => {
                self.next = last + 1;
                Ok(range)
            }
            Err(err) => {
                // A failed chunk ends the transfer: the caller has a partial
                // buffer and an error saying why, not a hole to paper over.
                self.next = self.last + 1;
                Err(err)
            }
        })
    }
}

impl HistoryTracker {
    /// Start a chunked read of `range`, `rows` rows at a time.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Evicted`] if the range starts below the eviction floor,
    /// [`HistoryError::NotCommitted`] if it reaches past the committed head.
    /// Both are refusals: 04 §3 has the server say plainly that a range is gone
    /// rather than answer with blanks.
    pub fn chunked(&self, range: RowRange, rows: u64) -> Result<HistoryChunks, HistoryError> {
        self.admits(range)?;
        Ok(HistoryChunks {
            next: range.first.get(),
            last: range.last.get(),
            rows: rows.max(1),
        })
    }

    /// Serve a whole range in one buffer, through the same chunking.
    ///
    /// # Errors
    ///
    /// As [`HistoryTracker::chunked`], plus [`HistoryError::Terminal`].
    pub fn read(
        &mut self,
        terminal: &Terminal,
        range: RowRange,
    ) -> Result<HistoryRows, HistoryError> {
        let mut chunks = self.chunked(range, CHUNK_ROWS)?;
        let mut rows = Vec::new();
        while let Some(chunk) = chunks.next_chunk(self, terminal, &mut rows) {
            chunk?;
        }
        Ok(HistoryRows { range, rows })
    }

    /// Whether the pane can still serve every row of `range`.
    fn admits(&self, range: RowRange) -> Result<(), HistoryError> {
        let oldest = self.oldest_row();
        if !oldest.admits(range.first) {
            return Err(HistoryError::Evicted { oldest: oldest.0 });
        }
        let head = self.head();
        if range.last.get() >= head.get() || range.row_count().is_none() {
            return Err(HistoryError::NotCommitted {
                head: RowId::from_raw(head.get().saturating_sub(1)),
            });
        }
        Ok(())
    }
}

/// Pack `range` into `out`, re-checking it against the pane first.
fn serve(
    tracker: &mut HistoryTracker,
    terminal: &Terminal,
    range: RowRange,
    out: &mut Vec<u8>,
) -> Result<(), HistoryError> {
    tracker.admits(range)?;
    let floor = tracker.oldest_row().0.get();
    for id in range.first.get()..=range.last.get() {
        let offset = u32::try_from(id - floor)
            .map_err(|_| HistoryError::Terminal("row is past the addressable grid".into()))?;
        tracker
            .pack(terminal, offset, out)
            .map_err(|err| HistoryError::Terminal(err.to_string()))?;
    }
    Ok(())
}
