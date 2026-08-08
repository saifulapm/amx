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

use amx_core::agent::{AgentSnapshot, HookToken};
use amx_core::platform::{ProcessId, WinSize};
use amx_core::{GridGeneration, PaneId, RowId, RowRange, Seq, SessionId};
use amx_vt::{Mode, Screen, Snapshot as GridSnapshot, Terminal};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

/// The terminal modes a pane carries across a handoff.
///
/// Every mode the vendored library knows (`terminal/modes.zig`) except four
/// classes, each excluded for a reason rather than an oversight:
///
/// - **132-column (3)** and **enable-mode-3 (40)** resize the grid, and the
///   successor's pane is built at the size the manifest already states;
/// - **left-and-right margins (69)** installs a scrolling region, which the C
///   API gives no way to read back — carrying the switch without the region
///   would be worse than carrying neither;
/// - **the screen switches (47, 1047, 1048, 1049)** are the alternate screen
///   itself, which [`PaneModes::alternate`] carries and the paint prologue
///   applies before the grid rather than after it.
const CARRIED_MODES: &[(u16, bool)] = &[
    (2, true),
    (4, true),
    (12, true),
    (20, true),
    (1, false),
    (4, false),
    (5, false),
    (6, false),
    (7, false),
    (8, false),
    (9, false),
    (12, false),
    (25, false),
    (45, false),
    (66, false),
    (67, false),
    (1000, false),
    (1002, false),
    (1003, false),
    (1004, false),
    (1005, false),
    (1006, false),
    (1007, false),
    (1015, false),
    (1016, false),
    (1035, false),
    (1036, false),
    (1039, false),
    (1045, false),
    (2004, false),
    (2026, false),
    (2027, false),
    (2031, false),
    (2048, false),
];

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
}

/// One pane's agent state, carried rather than re-derived (R-M3-13).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentEntry {
    /// The pane this describes.
    pub pane: PaneId,
    /// Its kind, state, cause, transition sequence and attention rank.
    pub agent: AgentSnapshot,
}

/// One terminal mode and whether it was on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ModeState {
    /// The mode number, as the VT specifications spell it.
    pub number: u16,
    /// Whether it is an ANSI mode rather than a DEC private one.
    pub ansi: bool,
    /// Whether it was set.
    pub on: bool,
}

/// A pane's modes, keyboard flags and title.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PaneModes {
    /// Whether the alternate screen was the active one.
    ///
    /// Separate from [`modes`](Self::modes) because it decides *which screen*
    /// the carried grid describes, so it is applied before the paint and not
    /// with the rest.
    pub alternate: bool,
    /// Every carried mode, in [`CARRIED_MODES`] order.
    pub modes: Vec<ModeState>,
    /// The Kitty keyboard flags the application asked for.
    pub kitty_flags: u8,
    /// The title set by OSC 0 or OSC 2, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PaneModes {
    /// Read every carried mode off a terminal.
    ///
    /// A mode this build of the library does not know is left out rather than
    /// guessed at: the successor runs the same library, so a mode neither side
    /// knows is a mode neither side has.
    fn capture(terminal: &Terminal) -> Result<Self, ManifestError> {
        let mut modes = Vec::with_capacity(CARRIED_MODES.len());
        for &(number, ansi) in CARRIED_MODES {
            let mode = if ansi {
                Mode::ansi(number)
            } else {
                Mode::dec(number)
            };
            if let Ok(on) = terminal.mode(mode) {
                modes.push(ModeState { number, ansi, on });
            }
        }
        let screen = terminal.active_screen().map_err(terminal_error)?;
        Ok(Self {
            alternate: screen == Screen::Alternate,
            modes,
            kitty_flags: terminal
                .kitty_keyboard_flags()
                .map_err(terminal_error)?
                .bits(),
            title: terminal.title().map_err(terminal_error)?,
        })
    }

    /// Whether a carried mode was set, or `missing` if it was not carried.
    #[must_use]
    pub fn is_set(&self, number: u16, missing: bool) -> bool {
        self.modes
            .iter()
            .find(|mode| mode.number == number && !mode.ansi)
            .map_or(missing, |mode| mode.on)
    }
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

fn terminal_error(err: amx_vt::Error) -> ManifestError {
    ManifestError::Terminal(err.to_string())
}

/// The standard base64 alphabet.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes for a JSON string field.
///
/// Hand-rolled because the packed rows are the one binary field in an otherwise
/// textual manifest, and a base64 crate is a dependency the tree does not need
/// for forty lines (HACKING.md's "prefer std, keep the tree lean").
fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for slot in 0..4 {
            if slot <= chunk.len() {
                let index = (packed >> (18 - slot * 6)) & 0x3F;
                out.push(char::from(ALPHABET[index as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode what [`encode`] produced.
fn decode(text: &str) -> Result<Vec<u8>, ManifestError> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut block = 0u32;
    let mut filled = 0u32;
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let Some(index) = ALPHABET.iter().position(|slot| *slot == byte) else {
            return Err(ManifestError::Malformed(format!(
                "base64 character {byte:#04x}"
            )));
        };
        // The index came from a 64-entry table, so it is six bits.
        block = (block << 6) | u32::try_from(index).unwrap_or(0);
        filled += 6;
        if filled >= 8 {
            filled -= 8;
            // Shifted down to a single byte by the line above.
            out.push(u8::try_from((block >> filled) & 0xFF).unwrap_or(0));
        }
    }
    Ok(out)
}
