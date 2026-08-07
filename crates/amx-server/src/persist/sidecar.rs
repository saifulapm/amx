//! The scrollback sidecar codec: one file per pane, keyed by UUID.
//!
//! ```text
//! sidecar := magic "AMXR", u16 version, 16 bytes pane uuid,
//!            u64 first row id, u64 last row id, rows…
//! row     := u32 text length, text bytes, u8 flags (bit 0: soft-wrapped)
//! ```
//!
//! The row layout is not restated here: rows are appended with
//! [`put_row`](amx_proto::stream::history::put_row) and read back with
//! [`PackedRows`], the *same* writer and reader the history stream uses on the
//! wire, so the file format and the wire format cannot drift apart. What this
//! module adds is the header — enough to know a file is a sidecar, which build
//! wrote it, and which pane it came from.
//!
//! The pane's UUID is repeated inside the file even though the name carries it,
//! so a sidecar that was copied, renamed or restored under the wrong name is
//! refused rather than replayed into somebody else's scrollback. Row ids are
//! recorded but not honoured on restore: replayed lines re-wrap at the current
//! width and take fresh ids through the normal tracker, which the eviction
//! floor makes safe (D-M1-6).
//!
//! **Torn files are expected, not exceptional.** A sidecar is written whole
//! through [`write_atomic`], so a torn one means the *rows* the writer read
//! were short, or the file was damaged underneath us. Either way the reader
//! stops at the last complete row and says so through [`Sidecar::truncated`]:
//! partial scrollback replayed and reported beats a pane refusing to restore
//! because its history had a bad tail.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use amx_core::{PaneId, RowId, RowRange};
use amx_proto::stream::history::PackedRows;

use super::io::{Commit, Syncs, remove_synced, write_atomic};
use super::{
    HISTORY_DIR, PersistError, SIDECAR_MAGIC, SIDECAR_VERSION, SidecarHeader, sidecar_name,
};

impl SidecarHeader {
    /// Append this header's encoding to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&SIDECAR_MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.pane.into_bytes());
        out.extend_from_slice(&self.range.first.get().to_le_bytes());
        out.extend_from_slice(&self.range.last.get().to_le_bytes());
    }

    /// Read a header off the front of `bytes`, returning it and the rows.
    ///
    /// # Errors
    ///
    /// [`PersistError::BadSidecar`] if `bytes` is shorter than a header, does
    /// not open with [`SIDECAR_MAGIC`], or names a format version this build
    /// does not read. There is no N/N−1 window here: the sidecar bumps its
    /// version when history rows change shape (styling, D-M1-6), and a build
    /// that guessed at rows it does not understand would replay garbage into a
    /// terminal.
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), PersistError> {
        if bytes.len() < Self::ENCODED_LEN {
            return Err(PersistError::BadSidecar(format!(
                "{} bytes is shorter than the {}-byte header",
                bytes.len(),
                Self::ENCODED_LEN,
            )));
        }
        let (header, rows) = bytes.split_at(Self::ENCODED_LEN);
        if header[..SIDECAR_MAGIC.len()] != SIDECAR_MAGIC {
            return Err(PersistError::BadSidecar(
                "the file does not open with the sidecar magic".to_owned(),
            ));
        }

        let mut cursor = SIDECAR_MAGIC.len();
        let version = u16::from_le_bytes(take(header, &mut cursor));
        if version != SIDECAR_VERSION {
            return Err(PersistError::BadSidecar(format!(
                "sidecar format version {version}, this build reads {SIDECAR_VERSION}",
            )));
        }
        let pane = PaneId::from_bytes(take(header, &mut cursor));
        let first = RowId::from_raw(u64::from_le_bytes(take(header, &mut cursor)));
        let last = RowId::from_raw(u64::from_le_bytes(take(header, &mut cursor)));

        Ok((
            Self {
                version,
                pane,
                range: RowRange::new(first, last),
            },
            rows,
        ))
    }
}

/// Read `N` bytes out of `header` at `cursor` and advance it.
///
/// The caller has already checked the length, which is the invariant that makes
/// the slice conversion infallible.
fn take<const N: usize>(header: &[u8], cursor: &mut usize) -> [u8; N] {
    let mut bytes = [0_u8; N];
    bytes.copy_from_slice(&header[*cursor..*cursor + N]);
    *cursor += N;
    bytes
}

/// One row out of a sidecar.
///
/// Bytes, not `String`: they go straight back into a terminal parser on replay,
/// and the wire carries them as bytes for the same reason.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SidecarRow {
    /// The row's text.
    pub text: Vec<u8>,
    /// Whether it soft-wraps into the next row — the flag that lets replay
    /// rebuild logical lines and let them re-wrap at the current width.
    pub wrapped: bool,
}

/// One pane's saved scrollback.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Sidecar {
    /// What the file said it was.
    pub header: SidecarHeader,
    /// The rows that read back cleanly, oldest first.
    pub rows: Vec<SidecarRow>,
    /// Whether the row run ended mid-row, so [`rows`](Self::rows) is a prefix
    /// of what was written. Restore replays it and reports the pane degraded
    /// rather than losing the pane (D-M1-9).
    pub truncated: bool,
}

/// The directory sidecars live in, inside a session's state directory.
#[must_use]
pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join(HISTORY_DIR)
}

/// Where `pane`'s sidecar sits.
#[must_use]
pub fn path(state_dir: &Path, pane: PaneId) -> PathBuf {
    dir(state_dir).join(sidecar_name(pane))
}

/// Encode a whole sidecar: `header`, then `rows` exactly as packed.
///
/// `rows` is a run of rows already packed by
/// [`put_row`](amx_proto::stream::history::put_row) — the bytes a history
/// range fetch produces — so a dump copies them rather than re-encoding them.
#[must_use]
pub fn encode(header: &SidecarHeader, rows: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SidecarHeader::ENCODED_LEN + rows.len());
    header.encode(&mut out);
    out.extend_from_slice(rows);
    out
}

/// Write `pane`'s sidecar into `state_dir` durably.
///
/// Creates `history/` on first write. Same atomic sequence as the snapshot: a
/// reader sees the whole previous dump or the whole new one.
///
/// # Errors
///
/// [`PersistError::Io`] if the directory, the staging file or the rename fails.
pub fn save(
    state_dir: &Path,
    header: &SidecarHeader,
    rows: &[u8],
    syncs: &impl Syncs,
) -> Result<Commit, PersistError> {
    let bytes = encode(header, rows);
    Ok(write_atomic(
        &dir(state_dir),
        &sidecar_name(header.pane),
        &bytes,
        syncs,
    )?)
}

/// Read `pane`'s sidecar, if it has one.
///
/// `Ok(None)` is "this pane has no saved scrollback" — history was off, the
/// pane never committed a row, or the file was wiped — which is not a loss.
///
/// # Errors
///
/// [`PersistError::BadSidecar`] if the file is not a readable sidecar or was
/// written for a different pane, [`PersistError::Io`] if it cannot be read.
/// Both degrade the pane (skip the replay, keep the pane) rather than losing
/// it.
pub fn load(state_dir: &Path, pane: PaneId) -> Result<Option<Sidecar>, PersistError> {
    let bytes = match fs::read(path(state_dir, pane)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let sidecar = decode(&bytes)?;
    if sidecar.header.pane != pane {
        return Err(PersistError::BadSidecar(format!(
            "{} holds pane {}, not {pane}",
            sidecar_name(pane),
            sidecar.header.pane,
        )));
    }
    Ok(Some(sidecar))
}

/// Parse a sidecar.
///
/// # Errors
///
/// [`PersistError::BadSidecar`] for a header this build cannot read. A torn
/// *row run* is not an error: it comes back as
/// [`truncated`](Sidecar::truncated) with the rows that survived.
pub fn decode(bytes: &[u8]) -> Result<Sidecar, PersistError> {
    let (header, packed) = SidecarHeader::decode(bytes)?;

    let mut rows = Vec::new();
    let mut truncated = false;
    for row in PackedRows::new(packed) {
        match row {
            Ok(row) => rows.push(SidecarRow {
                text: row.text.to_vec(),
                wrapped: row.wrapped,
            }),
            // The packed reader stops cleanly at a torn row and yields nothing
            // after it, so this is the tail of the file by construction.
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }

    Ok(Sidecar {
        header,
        rows,
        truncated,
    })
}

/// Remove `pane`'s sidecar. `true` if there was one.
///
/// # Errors
///
/// [`PersistError::Io`] if the file exists and cannot be removed.
pub fn remove(state_dir: &Path, pane: PaneId, syncs: &impl Syncs) -> Result<bool, PersistError> {
    Ok(remove_synced(&dir(state_dir), &sidecar_name(pane), syncs)?)
}

/// Remove every sidecar in `state_dir`, and the directory holding them.
///
/// What turning `[persist] history` off does, immediately (04 §6: scrollback
/// holds secrets, so the wipe is not deferred to the next save). Scoped by
/// construction: the path is `state_dir` joined with [`HISTORY_DIR`], never
/// anything a caller names.
///
/// # Errors
///
/// [`PersistError::Io`] if the directory exists and cannot be removed.
pub fn wipe(state_dir: &Path) -> Result<bool, PersistError> {
    match fs::remove_dir_all(dir(state_dir)) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}
