//! The local scrollback cache, keyed by the server's stable row ids (04 §3).
//!
//! "Scrollback is locally cached: the client requests history ranges by
//! stable row id, caches them, and scrolls locally at memory speed." This
//! module is that cache, and the invalidation contract 04 §3 attaches to it
//! is the whole design:
//!
//! - [`Scrollback::invalidate`] answers `history.invalidated{from_row}` by
//!   dropping every cached row at or beyond `from_row` — the server
//!   rebaselined those ids, so what the cache holds under them is no longer
//!   what the server means by them;
//! - [`Scrollback::evict`] tracks the pane's `oldest_row` floor, and a row
//!   below it is [`RowSlot::Unavailable`] — reported gone, never rendered as
//!   a blank pretending to be content;
//! - [`Scrollback::misses`] names exactly the fetchable gaps in a range, so
//!   scrolling asks the server for what is missing and nothing else.
//!
//! Rows are held as unstyled text: the server's history packing carries
//! characters and no styling until `amx-proto`'s packed cell layout exists
//! (the T12 spike says so explicitly), so caching anything richer would be
//! inventing data. The transfer that fills this cache — `HistoryChunk` over a
//! bound stream — has no codec yet either; [`Scrollback::fill`] takes decoded
//! rows and is the seam that decoder plugs into, the same shape `net` gives
//! the grid stream.

use std::collections::BTreeMap;

use amx_core::{EvictionFloor, RowId, RowRange};

/// What the cache can say about one row.
///
/// Three answers, deliberately not two: a miss the client can repair with a
/// range request is a different situation from a row that is gone for good,
/// and collapsing them is how blanks end up rendered as content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowSlot<'a> {
    /// The row's text, as served by the server (unstyled in M0).
    Cached(&'a str),
    /// Not cached, but fetchable: at or above the floor and committed.
    Missing,
    /// Not fetchable: evicted below the pane's `oldest_row` floor, or not
    /// (or no longer) committed. Render this as *unavailable*, never blank.
    Unavailable,
}

/// One pane's cached scrollback.
///
/// `head` is one past the newest row the server has announced as committed
/// (the pane's delta stream announces commits by id); `floor` is the pane's
/// `oldest_row` from its metadata and `history.evicted` announcements. The
/// cache never holds a row outside `[floor, head)`: eviction and invalidation
/// drop, and a late [`Scrollback::fill`] for ids that have since left the
/// window is ignored rather than allowed to resurrect them.
#[derive(Debug, Default)]
pub struct Scrollback {
    /// Cached rows by raw id. A `BTreeMap` because every consumer — slots,
    /// misses, invalidation — works in id ranges.
    rows: BTreeMap<u64, Box<str>>,
    /// The pane's eviction floor: rows below it cannot be re-fetched.
    floor: u64,
    /// One past the newest committed row.
    head: u64,
}

impl Scrollback {
    /// A cache for a pane that has committed nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The pane's eviction floor.
    #[must_use]
    pub const fn floor(&self) -> EvictionFloor {
        EvictionFloor(RowId::from_raw(self.floor))
    }

    /// One past the newest committed row.
    #[must_use]
    pub const fn head(&self) -> RowId {
        RowId::from_raw(self.head)
    }

    /// The newest committed row that is still fetchable, if any.
    #[must_use]
    pub fn newest(&self) -> Option<RowId> {
        let last = self.head.checked_sub(1)?;
        (last >= self.floor).then(|| RowId::from_raw(last))
    }

    /// Record that the server announced `range` as committed to history.
    ///
    /// Ids the announcement *re*-commits (a taller grid pulled rows back into
    /// the live area and they scrolled out again — the one deliberate reissue
    /// in the identity model) may carry different content now, so their cached
    /// text is dropped rather than trusted; checking the announcement's hash
    /// against the cached row would keep it, but the hash covers the packed
    /// wire bytes and no codec delivers those yet.
    pub fn commit(&mut self, range: RowRange) {
        let one_past = range.last.get().saturating_add(1);
        let recommitted = self.head.min(one_past);
        if range.first.get() < recommitted {
            let keep = self.rows.split_off(&recommitted);
            self.rows.split_off(&range.first.get());
            self.rows.extend(keep);
        }
        self.head = self.head.max(one_past);
    }

    /// Cache the served text of `range`, row by row in id order.
    ///
    /// Rows outside `[floor, head)` are ignored: a transfer that raced an
    /// invalidation or an eviction must not plant rows the contract says are
    /// gone. Extra rows beyond the range are ignored the same way.
    pub fn fill<'r>(&mut self, range: RowRange, rows: impl IntoIterator<Item = &'r str>) {
        for (id, text) in (range.first.get()..=range.last.get()).zip(rows) {
            if id >= self.floor && id < self.head {
                self.rows.insert(id, Box::from(text));
            }
        }
    }

    /// Apply `history.invalidated{from_row}`: every cached row at or beyond
    /// `from_row` is dropped, and the committed head falls back with it.
    ///
    /// The head falls back because the server rebaselined: the ids above
    /// `from_row` will not be served again until they are re-announced, so
    /// leaving them "committed" would have [`Scrollback::misses`] request
    /// ranges the server refuses.
    pub fn invalidate(&mut self, from_row: RowId) {
        self.rows.split_off(&from_row.get());
        self.head = self.head.min(from_row.get());
    }

    /// Apply an advanced eviction floor: rows below `oldest_row` can no longer
    /// be fetched, and their cached copies are dropped so the cache stays a
    /// bounded mirror of what the server can still serve.
    ///
    /// A floor never recedes (row ids are monotonic), so a stale announcement
    /// is a no-op.
    pub fn evict(&mut self, oldest_row: RowId) {
        if oldest_row.get() <= self.floor {
            return;
        }
        self.floor = oldest_row.get();
        self.rows = self.rows.split_off(&self.floor);
        self.head = self.head.max(self.floor);
    }

    /// What the cache can say about `row`.
    #[must_use]
    pub fn slot(&self, row: RowId) -> RowSlot<'_> {
        if let Some(text) = self.rows.get(&row.get()) {
            return RowSlot::Cached(text);
        }
        if row.get() >= self.floor && row.get() < self.head {
            RowSlot::Missing
        } else {
            RowSlot::Unavailable
        }
    }

    /// Append to `out` the fetchable gaps inside `range`, coalesced into the
    /// fewest ranges — exactly what a client sends as range requests to fill
    /// its view. Rows below the floor or past the head are not gaps: they are
    /// unavailable, and asking for them would only earn a refusal.
    pub fn misses(&self, range: RowRange, out: &mut Vec<RowRange>) {
        let first = range.first.get().max(self.floor);
        let Some(newest) = self.head.checked_sub(1) else {
            return;
        };
        let last = range.last.get().min(newest);
        if first > last {
            return;
        }
        let mut next = first;
        for &cached in self.rows.range(first..=last).map(|(id, _)| id) {
            if cached > next {
                out.push(RowRange::new(
                    RowId::from_raw(next),
                    RowId::from_raw(cached - 1),
                ));
            }
            next = cached + 1;
        }
        if next <= last {
            out.push(RowRange::new(RowId::from_raw(next), RowId::from_raw(last)));
        }
    }
}
