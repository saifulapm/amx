//! The manifest: what a session is, as one line of JSON.
//!
//! `docs/09-m3-plan.md` D-M3-5 tables the whole inventory and where each field
//! is reachable from — schema version and read window, exporter version and
//! proto window, the `SessionId`, the bus head, the persist `Snapshot` captured
//! in memory, per-pane state from the parser thread, and the hub's statuses and
//! attention queue in block order. What is deliberately *not* carried is tabled
//! there too: client connections, in-flight waits, damage accumulators, sidecar
//! files.
//!
//! The manifest surface skews on its own N/N−1 window, separately from the
//! control protocol: D-M3-6 point 2 has the importer check the manifest
//! *window* rather than demanding version equality, which is what lets
//! self-update hand a session to any successor that can read manifest v1.
//!
//! # The pane half, and the two directions it runs in
//!
//! [`PaneManifest::capture`] runs on the parser thread against a **quiesced**
//! pane and reads everything a pane is: the styled visible grid as a replay
//! ([`super::grid`]), the modes and flags and title the terminal answers for,
//! the most recent scrollback packed through the same `read_row` path a history
//! range uses, and the three counters continuity depends on — the tracker's
//! head and floor, the grid generation, and the frame counter.
//!
//! [`PaneManifest::seed`] runs the other way, on the successor's parser thread,
//! and produces a [`PaneSeed`]: the bytes to replay and the counters to resume
//! from. The order is **rows → grid → modes**, and each step is what makes the
//! next one mean anything:
//!
//! 1. the packed rows are replayed as the lines they came from, then scrolled
//!    off the top until the whole carried range is scrollback and the screen is
//!    blank. That blank screen is what [`super::grid`] paints onto, and it is
//!    why nothing has to clear one — see that module for why clearing would be
//!    worse than not clearing;
//! 2. the grid is painted, preceded by the handful of modes a faithful paint
//!    needs (wraparound, origin, insert, synchronised output, grapheme
//!    clustering) and by the alternate-screen switch when the pane was on it;
//! 3. every carried mode is then applied at its true value, which is also what
//!    puts the prologue's forced modes back.
//!
//! # Budget, and what falls off the bottom
//!
//! A pane carries at most [`MAX_ROWS`] rows and [`MAX_ROW_BYTES`] of packed
//! rows, whichever binds first, and the *oldest* rows are the ones dropped —
//! the newest scrollback is what a client scrolled up into is about to ask for.
//! Truncation is recorded on the entry ([`PaneHistory::truncated`],
//! [`PaneHistory::dropped`]) rather than being silent, so the restore report
//! has something true to say. Rows older than what crossed are announced
//! through the eviction floor the successor resumes at: ids are never reused,
//! so a client that cached them keeps them and simply cannot refetch, which is
//! the M0 invalidation contract doing exactly its job.
//!
//! # The three files
//!
//! This one is the manifest and its pane entries, in both directions. [`modes`]
//! is the terminal-mode inventory — the list of what crosses and the reasons
//! four classes do not — and [`base64`] is the encoding the one binary field
//! rides in. Split by responsibility on arrival at 617 lines rather than at the
//! hard limit (R-M1-3); the directory needs no change to `handoff/mod.rs`,
//! which is what W05 established when `protocol.rs` became `protocol/`.

mod base64;
mod modes;

use std::path::PathBuf;

use amx_core::agent::{AgentSnapshot, HookToken};
use amx_core::platform::{ProcessId, WinSize};
use amx_core::{Ctx, GridGeneration, PaneId, RowId, RowRange, Seq, SessionId, SessionName};
use amx_vt::{Snapshot as GridSnapshot, Terminal};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use self::base64::{decode, encode};
pub use self::modes::{ModeState, PaneModes};
use super::grid;
use crate::history::HistoryTracker;
use crate::persist::Snapshot;

/// The manifest schema this build writes.
pub const VERSION: u32 = 1;

/// The manifest schema versions this build reads.
///
/// The N/N−1 window the handoff surface skews on, separately from the control
/// protocol. At v1 there is no predecessor, so the window is just `{1}`.
pub const READ_WINDOW: &[u32] = &[VERSION];

/// How many scrollback rows one pane carries.
pub const MAX_ROWS: u64 = 500;

/// How many bytes of packed scrollback one pane carries.
///
/// Measured against the packed bytes, which is what the budget in D-M3-4 means
/// and what the transport pays for once they are encoded.
pub const MAX_ROW_BYTES: usize = 256 * 1024;

/// A manifest could not be built, read, or applied.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The manifest names a version outside [`READ_WINDOW`].
    #[error(
        "manifest from an amx outside the window: version {found}, this build reads {READ_WINDOW:?}"
    )]
    UnsupportedVersion {
        /// The version the manifest declared.
        found: u32,
    },
    /// The terminal could not answer something the capture needs.
    #[error("terminal: {0}")]
    Terminal(String),
    /// The pane's scrollback could not be read.
    #[error("scrollback: {0}")]
    History(String),
    /// A carried payload is not the shape it says it is.
    #[error("malformed manifest payload: {0}")]
    Malformed(String),
    /// The manifest is about a session this process is not (§3 step 6).
    #[error("the manifest hands over session {theirs}, and this process is {ours}")]
    WrongSession {
        /// The session the exporter is handing over.
        theirs: SessionIdentity,
        /// The session this process derived for itself.
        ours: SessionIdentity,
    },
}

/// One session, frozen, as the successor receives it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Manifest {
    /// The schema version these bytes were written at.
    pub version: u32,
    /// The schema versions the exporter can read, for the audit trail.
    pub read_window: Vec<u32>,
    /// The exporter's package version.
    pub exporter: String,
    /// The control-protocol window the exporter speaks, lowest first.
    pub proto: (u16, u16),
    /// The session's identity, which the successor keeps.
    ///
    /// This is what tells a reconnecting client "same session continued" from
    /// "different server": `Welcome.session` is the only place the swap is
    /// visible at all.
    pub session: SessionId,
    /// The session's *name* and socket, which the successor must already be.
    ///
    /// §3 step 6's "session dir identity", which W07 could not check because
    /// nothing carried it. It is not decoration: a server started as `amx
    /// --session live server` selects its session from a flag, not from its
    /// environment, and a successor spawned without that flag derives `default`
    /// — so an upgrade of a named session used to unlink `live`'s socket, bind
    /// `default`'s, and leave the panes under a name nobody could reach.
    /// Observed against the real binary on 2026-08-08, fixed at the spawn
    /// ([`super::super::export::StagedBinary`]) and refused here, so a future
    /// way of losing it is a message rather than a vanished session.
    ///
    /// Additive and optional: a manifest from an exporter that never sent it
    /// is checked as it always was, which is the same both-directions rule
    /// every other field on this surface follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<SessionIdentity>,
    /// The bus head the successor's sequence numbers continue from.
    pub seq: Seq,
    /// The session as persistence would have written it, captured in memory.
    ///
    /// Layout, cwds, labels, agent identity, session refs and short numbers —
    /// everything a cold restore reads off disk, without the disk round trip.
    pub state: Box<Snapshot>,
    /// Every pane, in transfer order: entry *n* pairs with the *n*th descriptor.
    pub panes: Vec<PaneManifest>,
    /// The hub's view of each pane that has one, attention queue included.
    #[serde(default)]
    pub agents: Vec<AgentEntry>,
}

impl Manifest {
    /// Check this manifest's version against [`READ_WINDOW`].
    ///
    /// # Errors
    ///
    /// [`ManifestError::UnsupportedVersion`] for anything outside the window.
    /// The window, not equality: a successor that reads manifest v1 takes a v1
    /// session whatever its own package version is (D-M3-6 point 2).
    pub fn check_version(&self) -> Result<(), ManifestError> {
        if READ_WINDOW.contains(&self.version) {
            return Ok(());
        }
        Err(ManifestError::UnsupportedVersion {
            found: self.version,
        })
    }

    /// Check that this manifest is about the session `ctx` describes
    /// (§3 step 6).
    ///
    /// Equality, not a window: a successor either *is* this session or it is
    /// somebody else, and there is no partial answer. Both facts are compared
    /// because they can disagree independently — the name is what the process
    /// was told and the socket is what it derived from it, and a mismatched
    /// runtime root produces the second without the first.
    ///
    /// # Errors
    ///
    /// [`ManifestError::WrongSession`] when either differs. A manifest that
    /// carries no identity at all passes, which is what makes the field
    /// additive: an older exporter is checked exactly as it always was.
    pub fn check_session_identity(&self, ours: &SessionIdentity) -> Result<(), ManifestError> {
        let Some(theirs) = &self.session_name else {
            return Ok(());
        };
        if theirs == ours {
            return Ok(());
        }
        Err(ManifestError::WrongSession {
            theirs: theirs.clone(),
            ours: ours.clone(),
        })
    }
}

/// Which session a manifest is about, by the two names a process has for one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SessionIdentity {
    /// The session name, as `--session` or `AMX_SESSION` spells it.
    pub session: SessionName,
    /// The socket that name resolves to under this machine's roots.
    pub socket: PathBuf,
}

impl SessionIdentity {
    /// The session `ctx` describes.
    #[must_use]
    pub fn of(ctx: &Ctx) -> Self {
        Self {
            session: ctx.session.clone(),
            socket: ctx.socket.clone(),
        }
    }
}

impl std::fmt::Display for SessionIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}", self.session, self.socket.display())
    }
}

/// One pane's agent state, carried rather than re-derived (R-M3-13).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentEntry {
    /// The pane this describes.
    pub pane: PaneId,
    /// Its kind, state, cause, transition sequence and attention rank.
    pub agent: AgentSnapshot,
}
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

/// One pane, frozen.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PaneManifest {
    /// Which pane this is.
    pub pane: PaneId,
    /// The child process behind it, which the successor does not parent.
    ///
    /// Carried so `kill` and foreground lookups keep working; the successor
    /// never learns an exit *code* from it (D-M3-12).
    pub child: u32,
    /// The grid size, which the successor's terminal is built at.
    pub rows: u16,
    /// Columns, as above.
    pub cols: u16,
    /// The pane's grid generation (04 §4), which the successor continues.
    pub generation: GridGeneration,
    /// The frame publication counter, which the successor's snapshot buffers
    /// continue so a reader's "have I seen this?" comparison stays true.
    pub frame: u64,
    /// Modes, keyboard flags and title.
    pub modes: PaneModes,
    /// The styled visible grid, as the replay [`super::grid`] synthesized.
    ///
    /// A `String` rather than bytes because it is one: every byte of it is
    /// either an ASCII control sequence or a grapheme cluster the library
    /// already gave us as UTF-8.
    pub grid: String,
    /// The cursor's shape, blink, visibility and position, as a replay of its
    /// own.
    ///
    /// Separate from [`grid`](Self::grid) because it is applied last, after the
    /// modes: replaying DEC mode 6 homes the cursor, so a position written with
    /// the paint would not survive the modes that follow it.
    pub cursor: String,
    /// The scrollback, and what the budget dropped.
    pub history: PaneHistory,
    /// The hook token this pane's child carries in its environment.
    ///
    /// A handoff keeps the *same* children, and a child's `AMX_HOOK_TOKEN` was
    /// written into its environment at spawn and cannot be changed afterwards —
    /// so a successor that minted a fresh one would drop every hook report the
    /// inherited agent sends (D-M2-4's misattribution guard), leaving its status
    /// on tier 2 alone across the upgrade. The token therefore crosses.
    ///
    /// It rides *here*, on the manifest, rather than on the persist
    /// `PaneSnapshot` beside the argv it belongs with: the snapshot is written
    /// to `session.json`, and a cold restore respawns the child with a freshly
    /// minted token — so a persisted one would be a dead secret on disk that
    /// nothing ever reads. This one crosses a 0600 socket to a process that
    /// needs it and is never written anywhere.
    ///
    /// Optional because [`PaneManifest::capture`] runs on the parser thread,
    /// which does not know it; `Core` fills it in from its own state as the
    /// manifest is assembled. `None` is a pane that never carried a token
    /// (never spawned through `Core::spawn`) or an entry from an exporter old
    /// enough not to have had this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<HookToken>,
}

impl PaneManifest {
    /// Capture one pane, on the parser thread, against a quiesced pty.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Terminal`] if the terminal cannot answer, and
    /// [`ManifestError::History`] if the scrollback range it just described
    /// cannot be read.
    pub fn capture(parts: Capture<'_>) -> Result<Self, ManifestError> {
        let Capture {
            pane,
            child,
            generation,
            terminal,
            tracker,
            snapshot,
        } = parts;
        let mut grid_bytes = Vec::new();
        grid::synthesize(snapshot, &mut grid_bytes);
        let mut cursor_bytes = Vec::new();
        grid::put_cursor(snapshot.cursor(), &mut cursor_bytes);
        Ok(Self {
            pane,
            child: child.0,
            rows: snapshot.rows(),
            cols: snapshot.cols(),
            generation,
            frame: snapshot.generation(),
            modes: PaneModes::capture(terminal)?,
            // The synthesizer only ever emits ASCII controls and clusters the
            // library handed over as UTF-8, so this cannot lose a byte; it is
            // a conversion rather than an assumption because the clusters
            // arrive across FFI.
            grid: String::from_utf8_lossy(&grid_bytes).into_owned(),
            cursor: String::from_utf8_lossy(&cursor_bytes).into_owned(),
            history: capture_history(terminal, tracker)?,
            // Filled by `Core` on the way past: the token lives in session
            // state, and this thread owns a terminal rather than a session.
            token: None,
        })
    }

    /// The pane's size, as the successor builds its terminal.
    #[must_use]
    pub fn size(&self) -> WinSize {
        WinSize {
            rows: self.rows,
            cols: self.cols,
        }
    }

    /// Turn the entry back into the bytes and counters a fresh pane is seeded
    /// with.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Malformed`] if the packed rows are not base64.
    pub fn seed(&self) -> Result<PaneSeed, ManifestError> {
        let packed = decode(&self.history.packed)?;
        let mut rows = Vec::new();
        grid::put_rows_replay(&packed, &mut rows);
        let mut screen = Vec::new();
        grid::put_paint_prologue(&self.modes, &mut screen);
        screen.extend_from_slice(self.grid.as_bytes());
        let mut modes = Vec::new();
        grid::put_modes(&self.modes, &mut modes);
        modes.extend_from_slice(self.cursor.as_bytes());
        Ok(PaneSeed {
            size: self.size(),
            head: self.history.head,
            carried: self.history.count,
            generation: self.generation,
            frame: self.frame,
            rows,
            screen,
            modes,
        })
    }
}

/// Everything [`PaneManifest::capture`] reads, so the call stays inside the
/// argument budget.
#[derive(Debug)]
pub struct Capture<'a> {
    /// Which pane this is.
    pub pane: PaneId,
    /// The child process behind it.
    pub child: ProcessId,
    /// The pane's grid generation.
    pub generation: GridGeneration,
    /// The terminal, which only the parser thread may hold.
    pub terminal: &'a Terminal,
    /// The pane's row-identity model.
    pub tracker: &'a mut HistoryTracker,
    /// The frame the capture describes.
    pub snapshot: &'a GridSnapshot,
}

/// What a fresh pane is seeded with, in the order it is applied.
///
/// Three byte runs and four counters. The runs are separate rather than one
/// blob because the step between the first two is not bytes at all: the
/// replayed rows have to be scrolled off the top before the grid is painted,
/// and only the parser thread — which can watch the scrollback level move —
/// knows how far.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaneSeed {
    /// The size the terminal is built at.
    pub size: WinSize,
    /// One past the newest committed row on the exporter.
    pub head: RowId,
    /// How many rows the replay is meant to put into the scrollback.
    pub carried: u64,
    /// The grid generation to continue from.
    pub generation: GridGeneration,
    /// The frame publication counter to continue from.
    pub frame: u64,
    /// The scrollback, as the lines it came from.
    pub rows: Vec<u8>,
    /// The paint prologue and the styled grid.
    pub screen: Vec<u8>,
    /// Every carried mode at its true value, and then the cursor.
    ///
    /// The cursor rides at the end of this run rather than with the paint
    /// because replaying DEC mode 6 homes it; see
    /// [`grid::put_cursor`](super::grid::put_cursor).
    pub modes: Vec<u8>,
}

/// Read the newest rows the budget allows, oldest dropped first.
fn capture_history(
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

pub(super) fn terminal_error(err: amx_vt::Error) -> ManifestError {
    ManifestError::Terminal(err.to_string())
}
