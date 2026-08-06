//! Instrumentation for the pane host's threading invariants.
//!
//! 04 §3 makes two claims that are otherwise invisible from the outside: the
//! libghostty-vt instance is owned by exactly one thread, and readers of the
//! published snapshot never make the parser wait. A counter is the only honest
//! way to assert either — a test that merely observes correct output would pass
//! just as happily against a pane-state mutex.
//!
//! Everything here is a relaxed atomic on a shared `Arc`: the parser writes,
//! anyone reads, and nothing the parser does on the frame path can block.

use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A shared view of what the pane's threads have done.
///
/// Cloning gives another handle on the same counters. Held by the pane host and
/// by whoever asked for one, so a test can read them while the pane runs.
#[derive(Clone, Debug, Default)]
pub struct PaneProbe(Arc<Counters>);

#[derive(Debug, Default)]
struct Counters {
    parses: AtomicU64,
    frames: AtomicU64,
    max_publish_nanos: AtomicU64,
    parser_thread: AtomicU64,
    history_thread: AtomicU64,
    snapshot_thread: AtomicU64,
}

impl PaneProbe {
    /// A fresh set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A stable key for the calling thread.
    ///
    /// `ThreadId` has no stable numeric form, so it is hashed; the value is
    /// only meaningful within one process, which is all a same-process
    /// assertion needs. Zero is reserved for "never recorded".
    #[must_use]
    pub fn thread_key() -> u64 {
        let mut hasher = DefaultHasher::new();
        std::thread::current().id().hash(&mut hasher);
        hasher.finish() | 1
    }

    /// How many chunks of pty output the parser has fed to the terminal.
    #[must_use]
    pub fn parses(&self) -> u64 {
        self.0.parses.load(Ordering::Relaxed)
    }

    /// How many snapshots have been published.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.0.frames.load(Ordering::Relaxed)
    }

    /// The longest a single publication has ever taken.
    ///
    /// This is the number that would blow up if publication waited on a reader:
    /// a reader that holds a snapshot for 200 ms would show up here.
    #[must_use]
    pub fn slowest_publish(&self) -> Duration {
        Duration::from_nanos(self.0.max_publish_nanos.load(Ordering::Relaxed))
    }

    /// The thread that owns the terminal, or `None` before it starts.
    #[must_use]
    pub fn parser_thread(&self) -> Option<u64> {
        read(&self.0.parser_thread)
    }

    /// The thread that served the last history range.
    #[must_use]
    pub fn history_thread(&self) -> Option<u64> {
        read(&self.0.history_thread)
    }

    /// The thread that published the last explicitly requested snapshot.
    #[must_use]
    pub fn snapshot_thread(&self) -> Option<u64> {
        read(&self.0.snapshot_thread)
    }

    pub(super) fn parser_started(&self) {
        self.0
            .parser_thread
            .store(Self::thread_key(), Ordering::Relaxed);
    }

    pub(super) fn parsed(&self) {
        self.0.parses.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn published(&self, took: Duration) {
        self.0.frames.fetch_add(1, Ordering::Relaxed);
        let nanos = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
        self.0.max_publish_nanos.fetch_max(nanos, Ordering::Relaxed);
    }

    pub(super) fn served_history(&self) {
        self.0
            .history_thread
            .store(Self::thread_key(), Ordering::Relaxed);
    }

    pub(super) fn served_snapshot(&self) {
        self.0
            .snapshot_thread
            .store(Self::thread_key(), Ordering::Relaxed);
    }
}

fn read(slot: &AtomicU64) -> Option<u64> {
    match slot.load(Ordering::Relaxed) {
        0 => None,
        key => Some(key),
    }
}
