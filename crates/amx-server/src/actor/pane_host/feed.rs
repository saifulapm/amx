//! A reader's handle on one pane's published frames.
//!
//! Split out of [`super`] by X02 on the same terms as [`super::config`]
//! (`docs/11-m4-plan.md` R-M4-5); the code is T09's, moved and not changed. It
//! is the whole of what a *reader* of a pane needs, which is why it is worth a
//! file of its own: everything else in this module is about starting and
//! stopping one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use amx_core::GridGeneration;
use amx_vt::SnapshotRef;
use tokio::sync::watch;

/// What one publication carries: the frame and the grid generation it was
/// built at, stored in one `watch` slot so a reader can never pair a frame
/// with another publication's generation.
pub(crate) type PublishedFrame = (SnapshotRef, GridGeneration);

/// A reader's handle on one pane's published frames.
///
/// Cloning gives another reader. Reading is an `Arc` clone out of the published
/// slot: whatever a reader then does with the snapshot, it does to plain data
/// that nothing will mutate under it, and the parser never waits for it.
#[derive(Clone, Debug)]
pub struct SnapshotFeed {
    pub(super) frames: watch::Receiver<PublishedFrame>,
    pub(super) generation: Arc<AtomicU64>,
}

impl SnapshotFeed {
    /// The most recently published frame.
    #[must_use]
    pub fn latest(&self) -> SnapshotRef {
        self.frames.borrow().0.clone()
    }

    /// The most recently published frame with the grid generation it was
    /// built at, read atomically.
    ///
    /// This is the pair a delta stream must use: [`SnapshotFeed::latest`] and
    /// [`SnapshotFeed::generation`] are two reads, and a resize between them
    /// would label one publication's cells with another's generation.
    #[must_use]
    pub fn frame(&self) -> (SnapshotRef, GridGeneration) {
        let published = self.frames.borrow();
        (published.0.clone(), published.1)
    }

    /// The pane's grid generation (04 §4), which the frame counter is not.
    ///
    /// This is the *live* generation: after a resize it can lead the one in
    /// [`SnapshotFeed::frame`] until the reflowed frame is published.
    #[must_use]
    pub fn generation(&self) -> GridGeneration {
        GridGeneration::from_raw(self.generation.load(Ordering::Acquire))
    }

    /// Wait for a frame newer than the one last seen through this handle.
    ///
    /// `false` once the pane is gone.
    pub async fn changed(&mut self) -> bool {
        self.frames.changed().await.is_ok()
    }
}
