//! Scrollback identity: row ids, the eviction floor, invalidation, ranges.
//!
//! 04 §3 gives the client a cache and therefore owes it an invalidation
//! contract:
//!
//! > Per-pane **monotonic row ids**, assigned when a line is committed to
//! > history, never reused across trimming or `clear`. A
//! > `history.invalidated{pane, from_row}` event fires on width-change reflow
//! > (only width changes reflow) and on `clear` … Pane metadata carries an
//! > eviction floor (`oldest_row`) so clients know which ranges the
//! > byte-limited scrollback has evicted and cannot re-fetch. Rows scrolling
//! > out of the live grid into history are announced on the pane's delta
//! > stream (id + content hash …).
//!
//! libghostty-vt has none of that. It has two levels — `SCROLLBACK_ROWS` and
//! `TOTAL_ROWS` — and one level cannot separate the rows a tick *committed*
//! from the rows it *pruned*. The T12 spike
//! (`docs/notes/scrollback-identity.md`) settles how it is derived anyway, and
//! this module is that derivation:
//!
//! - a **tracked grid reference** anchored on a row of known id reports its
//!   current history offset, and `id − offset` is exactly how many rows the
//!   scrollback has thrown away ([`tracker`]);
//! - the anchor is confirmed by **content hash** every observation, because the
//!   library keeps a discarded anchor alive pointing at a different row — a
//!   measured trap, not a hypothetical one;
//! - anything that cannot be derived is **rebaselined and announced**, never
//!   guessed: ids jump past everything ever issued and clients are told to drop
//!   what they cached, in the spirit of the event bus's `gap`.
//!
//! Reads never move the viewport ([`range`]): a history point resolves through
//! `ghostty_terminal_grid_ref`, which mutates nothing, so a bulk transfer
//! cannot disturb what another client is looking at.

mod pack;
mod range;
mod tracker;

pub use pack::{RowPack, hash_bytes};
pub use range::{CHUNK_ROWS, HistoryChunks};
pub use tracker::{Committed, HistoryEvent, HistoryTracker};
