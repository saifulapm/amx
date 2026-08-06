//! The identity model: what a row id means and how it is derived.
//!
//! One [`HistoryTracker`] per pane, living on the parser thread beside the
//! terminal it observes. It is observed at frame boundaries — the same place
//! the snapshot is published — and every observation answers one question: what
//! did the scrollback do since last time?
//!
//! The answer comes from three measurements: the scrollback level, an anchor of
//! known identity, and two content hashes that keep the anchor honest. What
//! they cannot explain is announced rather than guessed; see the module
//! documentation and `docs/notes/scrollback-identity.md`.

use amx_core::{EvictionFloor, InvalidationCause, RowHash, RowId, RowRange};
use amx_vt::{Point, PointSpace, Screen, Terminal, TrackedRow};
use tracing::debug;

use super::pack::hash_row;

/// How many rows one observation will hash.
///
/// Hashing a row costs a read (~3 µs measured), and a flood can commit more
/// rows in one frame than a whole scrollback holds, so hashing every committed
/// row would put an unbounded cost on the frame path. The cap keeps the newest
/// rows — the ones a client scrolled back into is about to ask for — and drops
/// hashes for the rest. That is sound because a hash only ever settles a
/// disagreement about an id a client already holds, and ids past the previous
/// head were never announced to anyone; the one window where ids *are* reissued
/// (rows pulled back into the live grid by a height change and committed again)
/// is bounded by the grid height and always fits.
const MAX_HASHED_ROWS: u64 = 256;

/// Rows that entered history, with hashes for as many as were hashed.
///
/// `hashes` is parallel to the **last** `hashes.len()` rows of `range`; see
/// [`Committed::hashed`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Committed {
    /// Every row that entered history, in order.
    pub range: RowRange,
    /// Content hashes for the tail of `range`.
    pub hashes: Vec<RowHash>,
}

impl Committed {
    /// The rows `hashes` covers, or `None` if none were hashed.
    #[must_use]
    pub fn hashed(&self) -> Option<RowRange> {
        let count = u64::try_from(self.hashes.len()).ok()?;
        let first = self.range.last.get().checked_sub(count.checked_sub(1)?)?;
        Some(RowRange::new(RowId::from_raw(first), self.range.last))
    }
}

/// What one observation found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HistoryEvent {
    /// Rows left the live grid and were committed to history.
    Committed(Committed),
    /// Everything cached at or beyond `from_row` is no longer what the server
    /// means by those ids.
    Invalidated {
        /// The first row a client must drop.
        from_row: RowId,
        /// Why.
        cause: InvalidationCause,
    },
    /// The eviction floor advanced; rows below `oldest_row` are unfetchable.
    Evicted {
        /// The oldest row still fetchable.
        oldest_row: RowId,
    },
}

/// An anchor whose row id and content are known.
struct Anchor {
    row: TrackedRow,
    id: u64,
    hash: RowHash,
}

/// One pane's scrollback identity.
pub struct HistoryTracker {
    /// Row id of history row 0: the number of rows ever discarded.
    floor: u64,
    /// One past the newest committed row.
    head: u64,
    /// The highest id ever issued, which the head can fall back below when a
    /// taller grid reclaims rows. A fresh id space starts here, never at the
    /// head, so an id announced once is never handed to a different row.
    issued: u64,
    anchor: Option<Anchor>,
    /// The hash of history row 0, so a floor kept without a probe is checked.
    floor_hash: Option<RowHash>,
    cols: u16,
    rows: u16,
    /// Reused across observations: no per-frame allocation once warm.
    text: String,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for HistoryTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryTracker")
            .field("oldest_row", &self.floor)
            .field("head", &self.head)
            .field("anchored", &self.anchor.is_some())
            .finish_non_exhaustive()
    }
}

impl HistoryTracker {
    /// A tracker for a pane that has committed nothing yet.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            floor: 0,
            head: 0,
            issued: 0,
            anchor: None,
            floor_hash: None,
            cols,
            rows,
            text: String::new(),
            bytes: Vec::new(),
        }
    }

    /// The oldest row still fetchable. Ranges below it are refused, not faked.
    #[must_use]
    pub fn oldest_row(&self) -> EvictionFloor {
        EvictionFloor(RowId::from_raw(self.floor))
    }

    /// One past the newest committed row.
    #[must_use]
    pub fn head(&self) -> RowId {
        RowId::from_raw(self.head)
    }

    /// Every row currently in history, or `None` while it is empty.
    #[must_use]
    pub fn committed(&self) -> Option<RowRange> {
        (self.head > self.floor)
            .then(|| RowRange::new(RowId::from_raw(self.floor), RowId::from_raw(self.head - 1)))
    }

    /// Look at the terminal and append what changed to `out`.
    ///
    /// Call it at frame boundaries on the parser thread. A library failure
    /// leaves the model untouched rather than moving it on a guess.
    pub fn observe(&mut self, terminal: &mut Terminal, out: &mut Vec<HistoryEvent>) {
        if let Err(err) = self.step(terminal, out) {
            debug!(error = %err, "pane history could not be observed");
        }
    }

    fn step(&mut self, terminal: &mut Terminal, out: &mut Vec<HistoryEvent>) -> amx_vt::Result<()> {
        // Both scrollback counters are per active screen, and the alternate
        // screen has no scrollback at all: observing there would read a wipe
        // that never happened (measured).
        if terminal.active_screen()? == Screen::Alternate {
            return Ok(());
        }

        let cols = terminal.cols()?;
        let rows = terminal.rows()?;
        let scrollback = terminal.scrollback_rows()? as u64;
        let width_changed = cols != self.cols;
        let height_changed = rows != self.rows;
        self.cols = cols;
        self.rows = rows;

        let floor_was = self.floor;
        let head_was = self.head;

        // 04 §3: only width changes reflow. A reflow rewrites history from its
        // oldest row, so no id survives it and the anchor's offset — which the
        // library keeps updating — means nothing afterwards.
        let cause = if width_changed {
            Some(InvalidationCause::WidthReflow)
        } else if let Some(floor) = self.probe(terminal, scrollback)? {
            self.floor = floor;
            None
        } else if height_changed && self.floor_intact(terminal, scrollback)? {
            // A taller grid pulls the newest history rows back into the live
            // area: the head recedes, the floor does not move, and nothing a
            // client cached became wrong.
            None
        } else {
            Some(InvalidationCause::Clear)
        };

        if let Some(cause) = cause {
            self.rebaseline(floor_was, cause, out);
        }
        self.head = self.floor + scrollback;
        self.issued = self.issued.max(self.head);

        if self.floor > floor_was {
            out.push(HistoryEvent::Evicted {
                oldest_row: RowId::from_raw(self.floor),
            });
        }
        if self.head > head_was {
            let committed = self.hash_committed(terminal, head_was.max(self.floor))?;
            out.push(HistoryEvent::Committed(committed));
        }
        self.reanchor(terminal, scrollback)?;
        Ok(())
    }

    /// Ask the anchor where it is, and believe it only if it can prove itself.
    ///
    /// Three ways an anchor lies, all measured: it survives a discarded
    /// scrollback pointing at a different row, its history offset counts on
    /// past the end of history once its row is pulled back into the live area,
    /// and a reflow moves it without moving what it means.
    fn probe(&mut self, terminal: &Terminal, scrollback: u64) -> amx_vt::Result<Option<u64>> {
        let Some(anchor) = &self.anchor else {
            return Ok(None);
        };
        let expected = anchor.hash;
        if !anchor.row.alive() || anchor.row.row(PointSpace::Active)?.is_some() {
            return Ok(None);
        }
        let Some(offset) = anchor.row.row(PointSpace::History)? else {
            return Ok(None);
        };
        if u64::from(offset) >= scrollback {
            return Ok(None);
        }
        let Some(floor) = anchor.id.checked_sub(u64::from(offset)) else {
            return Ok(None);
        };
        let hash = hash_row(terminal, offset, &mut self.text, &mut self.bytes)?;
        Ok((hash == expected).then_some(floor))
    }

    /// Whether history row 0 is still the row the floor was last read from.
    fn floor_intact(&mut self, terminal: &Terminal, scrollback: u64) -> amx_vt::Result<bool> {
        if scrollback == 0 {
            // Nothing left to check. A height change that empties history has
            // pulled every row back into the live area; a discarded scrollback
            // that arrives in the same observation is reported as a clear one
            // observation later, when the rows commit again under ids whose
            // hashes no longer match.
            return Ok(true);
        }
        let Some(expected) = self.floor_hash else {
            return Ok(false);
        };
        let hash = hash_row(terminal, 0, &mut self.text, &mut self.bytes)?;
        Ok(hash == expected)
    }

    /// Give up on deriving anything and start a fresh, never-issued id space.
    ///
    /// Ids stay monotonic and are never reused: the surviving rows are numbered
    /// from above every id ever issued, and everything below is announced as
    /// invalid and then as evicted.
    fn rebaseline(
        &mut self,
        floor_was: u64,
        cause: InvalidationCause,
        out: &mut Vec<HistoryEvent>,
    ) {
        if self.issued > floor_was {
            out.push(HistoryEvent::Invalidated {
                from_row: RowId::from_raw(floor_was),
                cause,
            });
        }
        self.floor = self.issued;
        self.anchor = None;
        self.floor_hash = None;
    }

    /// Hash the tail of `first..head` and describe the whole batch.
    fn hash_committed(&mut self, terminal: &Terminal, first: u64) -> amx_vt::Result<Committed> {
        let range = RowRange::new(RowId::from_raw(first), RowId::from_raw(self.head - 1));
        let count = self.head - first;
        let hashed_from = self.head - count.min(MAX_HASHED_ROWS);
        let mut hashes = Vec::with_capacity((self.head - hashed_from) as usize);
        for id in hashed_from..self.head {
            let offset = u32::try_from(id - self.floor).unwrap_or(u32::MAX);
            hashes.push(hash_row(terminal, offset, &mut self.text, &mut self.bytes)?);
        }
        Ok(Committed { range, hashes })
    }

    /// Pack one history row into `out`, reusing the tracker's scratch.
    ///
    /// Bounds are the caller's ([`super::range`] re-checks them per chunk):
    /// libghostty-vt's history space runs on into the live rows.
    pub(super) fn pack(
        &mut self,
        terminal: &Terminal,
        offset: u32,
        out: &mut Vec<u8>,
    ) -> amx_vt::Result<()> {
        super::pack::RowPack::append(terminal, offset, &mut self.text, out)
    }

    /// Anchor on the newest history row: the last row a prune reaches.
    fn reanchor(&mut self, terminal: &mut Terminal, scrollback: u64) -> amx_vt::Result<()> {
        if scrollback == 0 {
            self.anchor = None;
            self.floor_hash = None;
            return Ok(());
        }
        let newest = u32::try_from(scrollback - 1).unwrap_or(u32::MAX);
        let hash = hash_row(terminal, newest, &mut self.text, &mut self.bytes)?;
        self.floor_hash = Some(hash_row(terminal, 0, &mut self.text, &mut self.bytes)?);
        let point = Point::history(newest);
        let id = self.head - 1;
        match &mut self.anchor {
            // Reusing the handle rather than allocating one per frame; `set`
            // also clears a "no value" state, so a dead anchor comes back.
            Some(anchor) => {
                anchor.row.set(terminal, point)?;
                anchor.id = id;
                anchor.hash = hash;
            }
            None => {
                self.anchor = Some(Anchor {
                    row: terminal.track(point)?,
                    id,
                    hash,
                });
            }
        }
        Ok(())
    }
}
