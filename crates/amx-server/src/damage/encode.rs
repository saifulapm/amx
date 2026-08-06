//! Building one grid message from the authoritative grid, at send time.
//!
//! 04 §4: "on drain the server emits one delta built from the current
//! authoritative grid … cell contents are always read from the single
//! authoritative grid at send time".
//!
//! That sentence is the whole reason this type takes a [`Snapshot`] and a
//! [`DirtySet`] as *separate* arguments. The dirty set says which rows to
//! describe; the snapshot says what is in them. They are read at different
//! times — damage accumulates while a client is stalled, cells are read when it
//! drains — and keeping them apart is what stops a coalesced delta from
//! describing a grid that no longer exists.
//!
//! Every published snapshot is a complete copy of the visible grid, not just
//! the rows that changed (see [`amx_vt::Snapshots`]: rows nobody has touched
//! since the buffer was last filled are still correct in it), so "the
//! authoritative grid at send time" is exactly [`SnapshotFeed::latest`].
//!
//! [`SnapshotFeed::latest`]: crate::actor::SnapshotFeed::latest
//!
//! ## Buffers
//!
//! Reused scratch, cleared and refilled, never freed: the rect list and the
//! packed cells are `Vec`s, and the payload is a `BytesMut` so the finished
//! message leaves as a zero-copy [`Bytes`] ([`Encoder::take_payload`]) instead
//! of being copied into a frame. Once the writer drops the frame the split-off
//! bytes fall back to one owner and the next build reclaims the allocation, so
//! after the first few frames encoding and flushing allocate nothing at all —
//! the performance rule, asserted by `damage_encode_does_not_allocate_per_frame`
//! and `grid_stream_flush_and_outbound_do_not_allocate_per_frame`.

use amx_core::GridGeneration;
use amx_proto::stream::DamageRect;
use amx_vt::Snapshot;
use bytes::{Bytes, BytesMut};

use amx_proto::stream::cell;
use amx_proto::stream::{Cells, GridMessage};

use super::DamageError;
use super::codec::{self, CURSOR_BYTES, RECT_BYTES};
use super::dirty::DirtySet;

/// Bytes before the rect list: the tag and the generation.
const DELTA_PREFIX: usize = 1 + 8;
/// Bytes after the rect list: the rect count, the cursor and the cell length.
const DELTA_SUFFIX: usize = 2 + CURSOR_BYTES + 4;
/// Bytes a keyframe spends outside its cells.
const RESET_OVERHEAD: usize = 1 + 8 + 2 + 2 + CURSOR_BYTES + 4;

/// Reusable scratch for one grid stream's encoder.
#[derive(Debug, Default)]
pub struct Encoder {
    rects: Vec<DamageRect>,
    covered: Vec<u16>,
    cells: Vec<u8>,
    payload: BytesMut,
    complete: bool,
}

impl Encoder {
    /// An encoder with empty buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The message built by the last successful call.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Take the last message as a frame payload, without copying it.
    ///
    /// The bytes are split out of the scratch buffer and frozen; the scratch
    /// is left empty. When the frame is written and dropped the refcount on
    /// the split-off bytes falls back to one, and the next build's `reserve`
    /// reclaims the allocation — so a steady stream of frames cycles one
    /// buffer instead of copying out of it once per message.
    #[must_use]
    pub fn take_payload(&mut self) -> Bytes {
        self.payload.split().freeze()
    }

    /// The rects the last delta described.
    #[must_use]
    pub fn rects(&self) -> &[DamageRect] {
        &self.rects
    }

    /// The rows the last delta actually carried.
    ///
    /// Only these may be cleared from the dirty set: a delta stops at the
    /// stream's frame cap, and the rows it left out are still owed.
    #[must_use]
    pub fn covered(&self) -> &[u16] {
        &self.covered
    }

    /// Whether the last delta covered the whole dirty set.
    #[must_use]
    pub const fn complete(&self) -> bool {
        self.complete
    }

    /// Bytes the scratch holds on the heap.
    #[must_use]
    pub fn accounted_bytes(&self) -> usize {
        self.rects.capacity() * size_of::<DamageRect>()
            + self.covered.capacity() * size_of::<u16>()
            + self.cells.capacity()
            + self.payload.capacity()
    }

    /// Build a keyframe carrying every visible cell.
    ///
    /// # Errors
    ///
    /// [`DamageError::FrameTooLarge`] if the whole grid does not fit the
    /// stream's negotiated frame cap. A keyframe is indivisible by definition,
    /// so the answer is a larger cap, not a smaller keyframe.
    pub fn reset(
        &mut self,
        generation: GridGeneration,
        snapshot: &Snapshot,
        cap: usize,
    ) -> Result<(), DamageError> {
        self.cells.clear();
        self.rects.clear();
        self.covered.clear();
        for index in 0..snapshot.rows() {
            self.pack_row(snapshot, index);
            self.covered.push(index);
        }
        self.complete = true;

        let len = RESET_OVERHEAD + self.cells.len();
        if len > cap {
            return Err(DamageError::FrameTooLarge {
                kind: "keyframe",
                len,
                cap,
            });
        }

        self.payload.clear();
        GridMessage::Reset {
            generation,
            rows: snapshot.rows(),
            cols: snapshot.cols(),
            cells: Cells::new(&self.cells),
            cursor: codec::cursor_of(snapshot.cursor()),
        }
        .encode(&mut self.payload);
        Ok(())
    }

    /// Build a delta over as much of `dirty` as the frame cap allows.
    ///
    /// Rows are packed in grid order and the frame stops at the last one that
    /// fits, leaving the rest dirty for the next drain. Deltas are incremental,
    /// so arriving in two frames instead of one is slower, never wrong — and it
    /// is what bounds this encoder's buffers under a flood.
    ///
    /// # Errors
    ///
    /// [`DamageError::FrameTooLarge`] if not even one row fits the cap.
    pub fn delta(
        &mut self,
        generation: GridGeneration,
        snapshot: &Snapshot,
        dirty: &DirtySet,
        cap: usize,
    ) -> Result<(), DamageError> {
        self.cells.clear();
        self.covered.clear();
        let mut budget = cap.saturating_sub(DELTA_PREFIX + DELTA_SUFFIX);
        let mut needed = 0;

        for index in 0..dirty.rows() {
            if !dirty.contains(index) {
                continue;
            }
            let before = self.cells.len();
            self.pack_row(snapshot, index);
            // Charge each row a rect's worth of header: runs of adjacent rows
            // merge later, so this over-charges rather than under-charges, and
            // a frame that fits this budget always fits the real cap.
            let cost = self.cells.len() - before + RECT_BYTES;
            if cost > budget {
                self.cells.truncate(before);
                needed = cost;
                break;
            }
            budget -= cost;
            self.covered.push(index);
        }
        self.complete = self.covered.len() == usize::from(dirty.marked());

        if self.covered.is_empty() {
            return Err(DamageError::FrameTooLarge {
                kind: "delta row",
                len: needed + DELTA_PREFIX + DELTA_SUFFIX,
                cap,
            });
        }
        self.rects_from_covered(snapshot.cols());

        self.payload.clear();
        GridMessage::Delta {
            generation,
            rects: &self.rects,
            cells: Cells::new(&self.cells),
            cursor: codec::cursor_of(snapshot.cursor()),
        }
        .encode(&mut self.payload);
        Ok(())
    }

    /// Build a cursor-only message.
    pub fn cursor(&mut self, snapshot: &Snapshot) {
        self.rects.clear();
        self.covered.clear();
        self.complete = true;
        self.payload.clear();
        GridMessage::Cursor(codec::cursor_of(snapshot.cursor())).encode(&mut self.payload);
    }

    /// Append one row's cells to the scratch, count first.
    ///
    /// Rows are self-delimiting because a row need not be `cols` cells wide —
    /// the snapshot copies what the render state reported — and a decoder that
    /// inferred the boundary from the geometry would mis-split every row after
    /// the first short one.
    ///
    /// A row past the bottom of the snapshot packs as a count of zero: the rect
    /// still names it, so the client clears it, which is the correct reading of
    /// "this row is damaged and now holds no cells".
    fn pack_row(&mut self, snapshot: &Snapshot, index: u16) {
        let row = snapshot.row(index);
        let cells = row.map_or(&[][..], amx_vt::Row::cells);
        // A row cannot hold more cells than the grid is wide.
        let count = u16::try_from(cells.len()).unwrap_or(u16::MAX);
        self.cells.extend_from_slice(&count.to_le_bytes());
        let Some(row) = row else {
            return;
        };
        for cell in &cells[..usize::from(count)] {
            cell::encode(&codec::wire_cell(row, cell), &mut self.cells);
        }
    }

    /// Merge the covered rows into runs.
    fn rects_from_covered(&mut self, cols: u16) {
        self.rects.clear();
        for &row in &self.covered {
            match self.rects.last_mut() {
                Some(last) if last.row.saturating_add(last.rows) == row => last.rows += 1,
                _ => self.rects.push(DamageRect {
                    row,
                    col: 0,
                    rows: 1,
                    cols,
                }),
            }
        }
    }
}
