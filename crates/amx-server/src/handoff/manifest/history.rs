//! What crosses of a pane's scrollback, and what the budget drops.
//!
//! D-M3-4's third input: the most recent rows, packed in the M1 sidecar format
//! and read through the same `read_row` path a history range uses. The budget
//! is two numbers and the *oldest* rows are what falls off, because the newest
//! scrollback is what a client scrolled up into is about to ask for.
//!
//! Truncation is recorded on the entry rather than being silent, so the restore
//! report has something true to say.

use amx_core::{RowId, RowRange};
use amx_vt::Terminal;
use serde::{Deserialize, Serialize};

use super::ManifestError;
use super::base64::{decode, encode};
use crate::history::HistoryTracker;

/// How many scrollback rows one pane carries.
pub const MAX_ROWS: u64 = 500;

/// How many bytes of packed scrollback one pane carries.
///
/// Measured against the packed bytes, which is what the budget in D-M3-4 means
/// and what the transport pays for once they are encoded.
pub const MAX_ROW_BYTES: usize = 256 * 1024;

/// The scrollback one pane carries, and what fell off the bottom.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PaneHistory {
    /// One past the newest committed row, which the successor continues from.
    pub head: RowId,
    /// The oldest row the *exporter* could still serve.
    ///
    /// Kept for the audit trail. The successor's own floor is derived from what
    /// actually landed in its scrollback, which is never older than this and is
    /// usually newer.
    pub floor: RowId,
    /// The first row id the carried rows start at.
    pub first: RowId,
    /// How many rows are carried.
    pub count: u64,
    /// The packed rows, base64 of the M1 sidecar packing.
    pub packed: String,
    /// Whether either budget dropped rows off the oldest end.
    pub truncated: bool,
    /// How many rows the budget dropped, oldest first.
    pub dropped: u64,
}

impl PaneHistory {
    /// The carried rows, back in the M1 packing they were read out as.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Malformed`] if the payload is not base64.
    pub fn rows(&self) -> Result<Vec<u8>, ManifestError> {
        decode(&self.packed)
    }
}

/// Read the newest rows the budget allows, oldest dropped first.
pub(super) fn capture_history(
    terminal: &Terminal,
    tracker: &mut HistoryTracker,
) -> Result<PaneHistory, ManifestError> {
    let head = tracker.head();
    let floor = tracker.oldest_row().0;
    let mut carried = PaneHistory {
        head,
        floor,
        first: head,
        count: 0,
        packed: String::new(),
        truncated: false,
        dropped: 0,
    };
    let Some(committed) = tracker.committed() else {
        return Ok(carried);
    };
    let wanted = head
        .get()
        .saturating_sub(MAX_ROWS)
        .max(committed.first.get());
    carried.dropped = wanted - committed.first.get();
    let range = RowRange::new(RowId::from_raw(wanted), committed.last);
    let served = tracker
        .read(terminal, range)
        .map_err(|err| ManifestError::History(err.to_string()))?;
    let (at, dropped) = trim_oldest(&served.rows, MAX_ROW_BYTES);
    carried.dropped += dropped;
    carried.first = RowId::from_raw(wanted + dropped);
    carried.count = head.get().saturating_sub(carried.first.get());
    carried.truncated = carried.dropped > 0;
    carried.packed = encode(&served.rows[at..]);
    Ok(carried)
}

/// Where to start reading a packed run so that it fits in `cap`, and how many
/// rows that drops off the oldest end.
fn trim_oldest(packed: &[u8], cap: usize) -> (usize, u64) {
    let mut at = 0usize;
    let mut dropped = 0u64;
    while packed.len() - at > cap {
        // The packing is self-delimiting: a `u32` length, the text, one flag
        // byte. A run that does not parse is one this process wrote, so a
        // short read can only mean the budget already reached the end.
        let Some(head) = packed.get(at..at + 4) else {
            break;
        };
        let Ok(length) = <[u8; 4]>::try_from(head) else {
            break;
        };
        let step = 4 + u32::from_le_bytes(length) as usize + 1;
        if at + step > packed.len() {
            break;
        }
        at += step;
        dropped += 1;
    }
    (at, dropped)
}
