//! Scrollback identity.
//!
//! 04 §3: "a cache needs an invalidation contract, like the event bus's `gap`".
//! These are the types that contract is written in. The client caches history
//! by [`RowId`] and scrolls locally at memory speed; every way the server can
//! invalidate that cache has a type here, so no cached row can silently become
//! wrong.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A row of scrollback, identified for the lifetime of the pane.
///
/// 04 §3: "Per-pane **monotonic row ids**, assigned when a line is committed to
/// history, never reused across trimming or `clear`." Ids are per pane — two
/// panes both have a row 0 — and are dense in allocation order, so a
/// [`RowRange`] is a pair of endpoints rather than a list.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RowId(u64);

impl RowId {
    /// The first row a pane ever commits to history.
    pub const FIRST: Self = Self(0);

    /// Rebuild a row id from the wire or from a snapshot.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next row id in allocation order.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for RowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// An inclusive range of rows, `first..=last`.
///
/// Inclusive on both ends because it always describes rows that exist: a range
/// is only ever built around committed rows, so there is no empty range to
/// represent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RowRange {
    /// Oldest row in the range.
    pub first: RowId,
    /// Newest row in the range.
    pub last: RowId,
}

impl RowRange {
    /// A range covering exactly the rows `first..=last`.
    #[must_use]
    pub const fn new(first: RowId, last: RowId) -> Self {
        Self { first, last }
    }

    /// A range covering exactly one row.
    #[must_use]
    pub const fn single(row: RowId) -> Self {
        Self {
            first: row,
            last: row,
        }
    }

    /// How many rows the range covers, or `None` if `last < first`.
    #[must_use]
    pub const fn row_count(&self) -> Option<u64> {
        match self.last.get().checked_sub(self.first.get()) {
            Some(span) => Some(span + 1),
            None => None,
        }
    }

    /// Whether `row` falls inside the range.
    #[must_use]
    pub const fn contains(&self, row: RowId) -> bool {
        row.get() >= self.first.get() && row.get() <= self.last.get()
    }
}

/// A content hash of one committed row.
///
/// 04 §3: rows scrolling out of the live grid into history are "announced on
/// the pane's delta stream (id + content hash)", so a client that was scrolled
/// back during heavy output can tell whether the row it cached under that id is
/// the row the server is talking about.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RowHash(u64);

impl RowHash {
    /// Wrap a computed hash.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw hash.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Why cached history at or beyond a row is no longer valid.
///
/// 04 §3: `history.invalidated{pane, from_row}` fires "on width-change reflow
/// (only width changes reflow) and on `clear`"; clients drop cached ranges and
/// cancel any copy-mode selection anchored at or beyond `from_row`. A height
/// change is deliberately not a cause — it does not reflow.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InvalidationCause {
    /// The pane's width changed and history reflowed.
    WidthReflow,
    /// The application cleared the screen and its scrollback.
    Clear,
    /// The terminal was fully reset (RIS), discarding history.
    Reset,
}

/// The oldest row a pane can still serve.
///
/// 04 §3: "Pane metadata carries an eviction floor (`oldest_row`) so clients
/// know which ranges the byte-limited scrollback has evicted and cannot
/// re-fetch." A request below the floor is refused, never faked with blanks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvictionFloor(pub RowId);

impl EvictionFloor {
    /// Whether `row` is still fetchable from the pane's history.
    #[must_use]
    pub const fn admits(self, row: RowId) -> bool {
        row.get() >= self.0.get()
    }
}
