//! Scrollback sidecars: which pane's history moved, and getting it to disk.
//!
//! D-M1-6's half of a save. The snapshot is the session's shape and is written
//! every time it changes; a sidecar is one pane's scrollback and is written only
//! when that pane's history is not what was dumped last time — a full 5,000-row
//! pane costs ~17 ms of parser time to read, which is affordable at debounce
//! cadence precisely because most saves skip most panes.
//!
//! "Has it moved?" is answered from the bus, never by asking the pane: the
//! committed window is folded from `HistoryCommitted`/`HistoryEvicted`/
//! `HistoryInvalidated` and compared against the window last dumped. All three
//! matter, and for different reasons — new rows extend it, eviction shortens it
//! from the front, and a reflow shortens it from the back *and* changes what
//! the already-written rows mean.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use amx_core::{Event, PaneId, RowId, RowRange};
use tokio::sync::oneshot;

use super::actor::Shared;
use crate::actor::{PaneCommand, PaneHandle};
use crate::persist::io::Syncs;
use crate::persist::{SidecarHeader, sidecar};

/// How long a dump waits on one pane before giving up on it.
///
/// A dump is a request to a *sibling actor*, and the shutdown rules are built on
/// `Persist` never being stuck inside one. A pane that does not answer within
/// the budget keeps its scrollback out of this save and is dumped at the next
/// one; the alternative is an unbounded wait on the one task that has to return
/// promptly when the runtime is coming down (R-M1-2).
const DUMP_BUDGET: Duration = Duration::from_millis(250);

/// Per-pane scrollback bookkeeping, and the dump it decides on.
#[derive(Debug, Default)]
pub(super) struct Sidecars {
    /// Each pane's committed history window, folded from the bus.
    windows: HashMap<PaneId, RowRange>,
    /// The window each pane's sidecar was last written at.
    dumped: HashMap<PaneId, RowRange>,
}

impl Sidecars {
    /// Fold one history event into the windows.
    ///
    /// Only the three history variants reach here; the caller has already
    /// decided that structure is dirt and everything else is noise.
    pub(super) fn observe(&mut self, event: &Event) {
        match event {
            Event::HistoryCommitted { pane, range } => {
                self.windows
                    .entry(*pane)
                    .and_modify(|window| window.last = range.last)
                    .or_insert(*range);
            }
            Event::HistoryEvicted { pane, oldest_row } => self.evicted(*pane, *oldest_row),
            Event::HistoryInvalidated { pane, from_row, .. } => self.invalidated(*pane, *from_row),
            _ => {}
        }
    }

    /// The eviction floor advanced: the window shrinks from the front, or is
    /// gone entirely if the floor overtook the head.
    fn evicted(&mut self, pane: PaneId, oldest_row: RowId) {
        let Some(window) = self.windows.get_mut(&pane) else {
            return;
        };
        if oldest_row > window.last {
            self.windows.remove(&pane);
        } else if oldest_row > window.first {
            window.first = oldest_row;
        }
    }

    /// Rows at or past `from_row` no longer mean what was dumped: the window
    /// shrinks from the back, which also makes the pane due for a fresh dump.
    fn invalidated(&mut self, pane: PaneId, from_row: RowId) {
        let Some(window) = self.windows.get_mut(&pane) else {
            return;
        };
        if from_row <= window.first {
            self.windows.remove(&pane);
        } else if from_row <= window.last {
            // `from_row > window.first >= 0`, so there is a row before it.
            window.last = RowId::from_raw(from_row.get() - 1);
        }
    }

    /// Forget which panes have been dumped, without forgetting their windows.
    ///
    /// What a wipe leaves behind: the sidecars are gone, so every pane is due
    /// again if history is ever turned back on, but the committed windows are
    /// still whatever the bus said they were — those are facts about the panes,
    /// and no config edit changes them.
    pub(super) fn forget_dumps(&mut self) {
        self.dumped.clear();
    }

    /// Dump every pane whose history moved since last time; count them.
    ///
    /// `panes` is empty whenever `[persist] history` is off — `Core` only clones
    /// pane handles into a capture that asked for them — so the whole path costs
    /// nothing when nobody opted in.
    pub(super) async fn dump(
        &mut self,
        state_dir: &Path,
        syncs: &Arc<dyn Syncs + Send + Sync>,
        panes: &[(PaneId, PaneHandle)],
    ) -> u64 {
        let mut dumps = 0;
        for (pane, handle) in panes {
            let Some(window) = self.windows.get(pane).copied() else {
                continue;
            };
            if self.dumped.get(pane) == Some(&window) {
                continue;
            }
            if dump_one(state_dir, syncs, *pane, handle, window).await {
                self.dumped.insert(*pane, window);
                dumps += 1;
            }
        }
        dumps
    }
}

/// Read one pane's history and write its sidecar. `false` if it did not
/// happen, which costs that pane's scrollback and nothing else.
async fn dump_one(
    state_dir: &Path,
    syncs: &Arc<dyn Syncs + Send + Sync>,
    pane: PaneId,
    handle: &PaneHandle,
    window: RowRange,
) -> bool {
    let (reply, answer) = oneshot::channel();
    let asked = handle.send(PaneCommand::HistoryRange {
        range: window,
        reply,
    });
    // `HistoryRange` executes on the pane's parser thread (04 §3), so the wait
    // is bounded by that thread's queue — bounded, but not by anything here.
    let rows = match tokio::time::timeout(DUMP_BUDGET, async {
        asked.await.ok()?;
        answer.await.ok()
    })
    .await
    {
        Ok(Some(Ok(rows))) => rows,
        Ok(Some(Err(err))) => {
            tracing::debug!(%pane, error = %err, "no scrollback to dump");
            return false;
        }
        Ok(None) => return false,
        Err(_) => {
            tracing::debug!(%pane, "the pane did not serve its history in time");
            return false;
        }
    };

    // The header records what was actually served, not what was asked for: a
    // sidecar must describe its own contents.
    let header = SidecarHeader::new(pane, rows.range);
    let dir = state_dir.to_path_buf();
    let syncs = Shared(Arc::clone(syncs));
    let written =
        tokio::task::spawn_blocking(move || sidecar::save(&dir, &header, &rows.rows, &syncs)).await;
    match written {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            tracing::warn!(%pane, error = %err, "the scrollback sidecar was not written");
            false
        }
        Err(err) => {
            tracing::warn!(%pane, error = %err, "the sidecar write panicked");
            false
        }
    }
}
