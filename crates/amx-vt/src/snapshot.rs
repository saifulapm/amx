//! The derived POD cell snapshot.

use std::sync::Arc;

/// A published, immutable copy of a pane's visible grid.
///
/// 04 §3: the parser copies damaged visible rows plus the cursor into a
/// "derived, double-buffered POD cell snapshot published lock-free for render
/// and tier-2 detection". Three properties follow, and they are the reason this
/// type exists at all rather than readers borrowing the terminal:
///
/// - It is **derived**. The live grid stays exclusively owned by the parser
///   thread; a snapshot is a copy, so reading one can never contend with the
///   parser on a pane-state mutex.
/// - It is **POD**. No pointers into the FFI object survive publication, so a
///   reader holding a snapshot cannot observe the terminal mutating underneath
///   it.
/// - It is **double buffered**. The parser fills the back buffer and publishes
///   it; readers keep whichever buffer they were handed for as long as they
///   hold their [`SnapshotRef`].
///
/// Cell layout and the damage-row list land with the wrapper implementation.
#[derive(Debug)]
pub struct Snapshot {
    rows: u16,
    cols: u16,
}

impl Snapshot {
    /// An empty snapshot of a grid this size.
    #[must_use]
    pub fn empty(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }

    /// Rows in the snapshotted grid.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Columns in the snapshotted grid.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }
}

/// A reader's handle on a published [`Snapshot`].
///
/// Shared, refcounted and read-only: publication is an `Arc` swap, so a reader
/// never blocks the parser and the parser never waits for a reader.
pub type SnapshotRef = Arc<Snapshot>;
