//! Reading and writing `session.json`.
//!
//! The schema is [`Snapshot`] (frozen next door in [`super`]); this module is
//! the file it lives in — the bytes, the read window, and the two errors a
//! restore turns into report entries instead of log lines.
//!
//! **Bytes.** Pretty-printed JSON with a trailing newline. A snapshot is small
//! (one object per workspace and pane) and is read by humans debugging a
//! restore far more often than by anything performance-sensitive, so the
//! checked-in golden stays a readable diff.
//!
//! **The read window.** 04 §6 asks for "a version field with an N/N−1 read
//! window". [`load`] reads the version *first*, out of an otherwise ignored
//! document, and refuses one outside the window before trying to interpret a
//! single other field: a snapshot from a newer amx is a whole-session loss the
//! user is told about, and diagnosing it as "malformed" — which is what parsing
//! it first would produce — would be a worse answer to the same file.
//!
//! **Unknown fields** are ignored, the same tolerance rule the wire keeps in
//! both directions (04 §4), so a later version can add fields without stranding
//! readers inside the window.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::io::{Commit, Syncs, write_atomic};
use super::{PersistError, SNAPSHOT_NAME, Snapshot};

/// Where the snapshot sits inside a session's state directory.
#[must_use]
pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(SNAPSHOT_NAME)
}

/// Serialize `snapshot` to the bytes that go on disk.
///
/// # Errors
///
/// [`PersistError::Malformed`] if the snapshot cannot be serialized, which for
/// this schema means a `Layout` that is not representable — a bug, but not one
/// worth losing the process over during a save.
pub fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, PersistError> {
    let mut bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|err| PersistError::Malformed(err.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Write `snapshot` into `state_dir` durably.
///
/// Creates the state directory on first write. See
/// [`write_atomic`](super::io::write_atomic) for what "durably" is doing.
///
/// # Errors
///
/// [`PersistError::Malformed`] if the snapshot will not serialize,
/// [`PersistError::Io`] if the directory, the staging file or the rename fails
/// — in which case the previous snapshot is still on disk, whole.
pub fn save(
    state_dir: &Path,
    snapshot: &Snapshot,
    syncs: &impl Syncs,
) -> Result<Commit, PersistError> {
    let bytes = encode(snapshot)?;
    Ok(write_atomic(state_dir, SNAPSHOT_NAME, &bytes, syncs)?)
}

/// Read the snapshot in `state_dir`, if there is one.
///
/// `Ok(None)` is "this session has no saved state": a first run, or a state
/// directory that only ever held staging files. Restore treats it as the empty
/// case and falls through to the normal first-attach seed (D-M1-9), which is
/// why it is not an error.
///
/// # Errors
///
/// [`PersistError::NewerVersion`] for a snapshot outside the read window,
/// [`PersistError::Malformed`] for one this build cannot parse, and
/// [`PersistError::Io`] if the file exists but cannot be read. All three are
/// whole-session losses that restore reports; none of them is partially
/// applied, because a snapshot only becomes a value once it is entirely
/// understood.
pub fn load(state_dir: &Path) -> Result<Option<Snapshot>, PersistError> {
    match fs::read(path(state_dir)) {
        Ok(bytes) => decode(&bytes).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Parse a snapshot, applying the read window before anything else.
///
/// # Errors
///
/// As [`load`], minus the I/O case.
pub fn decode(bytes: &[u8]) -> Result<Snapshot, PersistError> {
    /// Just enough of the document to answer "can this build read it at all?".
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }

    let probe: Version =
        serde_json::from_slice(bytes).map_err(|err| PersistError::Malformed(err.to_string()))?;
    // Ask the window where the window lives, rather than restating it: a
    // snapshot carrying only the version is enough to be refused by it.
    Snapshot {
        version: probe.version,
        ..Snapshot::empty()
    }
    .check_version()?;

    serde_json::from_slice(bytes).map_err(|err| PersistError::Malformed(err.to_string()))
}
