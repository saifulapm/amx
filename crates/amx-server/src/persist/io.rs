//! The durable write: stage, fsync, rename, fsync the directory.
//!
//! Every state file amx writes goes through [`write_atomic`], in exactly the
//! order 04 §6 requires ("written atomic tmp+rename **+ fsync of file and
//! directory**"):
//!
//! 1. create `<name>.<pid>.<counter>.tmp` in the destination directory;
//! 2. write the whole payload into it;
//! 3. sync the staging file;
//! 4. rename it onto the destination name;
//! 5. sync an fd of the containing directory.
//!
//! Each step earns its place. Step 3 has to precede step 4 because rename
//! orders nothing: fsync(2) is what "transfers all modified in-core data … to
//! the disk device", so a rename made durable before the staged blocks are
//! leaves the committed name pointing at whatever the page cache had. Step 5 is
//! the same man page's other sentence — "calling fsync() does not necessarily
//! ensure that the entry in the directory containing the file has also reached
//! disk. For that an explicit fsync() on a file descriptor for the directory is
//! also needed" — without which the *rename itself* can vanish at power loss.
//!
//! The syncs are [`std::fs::File`] calls, deliberately, and this is the one
//! place in the server that prefers std file I/O over rustix: on Apple targets
//! `File::sync_all` issues `fcntl(F_FULLFSYNC)`, which is what actually flushes
//! the drive cache there, while `rustix::fs::fsync` is the raw syscall and
//! would silently drop that guarantee on a tier-1 platform.
//!
//! **Failure handling.** A staging failure — including a failed sync — aborts
//! before the rename: the previous file is still whole and untouched, the
//! staging file is unlinked, and the error is returned. After an fsync error
//! the page cache state is unknown (fsync(2), ERRORS), so nothing here retries
//! the same descriptor; the next save opens fresh files. A directory sync that
//! fails *after* a successful rename is reported through [`Commit`] rather than
//! rolled back — the content at that point is correct and visible, only its
//! survival of a power loss in the next few seconds is weaker.
//!
//! **No symlink chasing.** These files are machine-owned state under
//! `$XDG_STATE_HOME`, not the stow-managed config trees that make writing
//! through a symlink worth the ceremony, and a symlinked `session.json` is
//! simply replaced by the rename. Config, which users do manage with symlinks,
//! is only ever read.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The suffix every staging file carries.
const TMP_SUFFIX: &str = ".tmp";

/// Distinguishes staging files written by this process from one another.
///
/// Paired with the pid in the staging name, because a fixed `session.json.tmp`
/// lets two writers to one directory clobber each other's half-written file —
/// a collision that costs a save even though the committed name is safe.
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

/// The two sync points of the durable write sequence, as a seam.
///
/// It exists so a test can *record the ordering* — the payload written before
/// the file sync, the file sync before the rename, the rename before the
/// directory sync — which is the only honest way to check fsync discipline in
/// CI. The power-loss claim itself rests on the man-page-backed sequence above;
/// the seam is what keeps the sequence from silently changing.
///
/// Both methods receive the path as well as the descriptor: the descriptor is
/// what gets synced, the path is what a recording implementation reports.
pub trait Syncs {
    /// Flush a staging file's contents to the disk device.
    ///
    /// # Errors
    ///
    /// Whatever the sync fails with. [`write_atomic`] treats it as fatal for
    /// this write and never retries the same descriptor.
    fn sync_file(&self, file: &File, path: &Path) -> io::Result<()>;

    /// Flush a directory's entries to the disk device.
    ///
    /// # Errors
    ///
    /// Whatever the sync fails with. After a successful rename this is
    /// reported, not fatal — see [`Commit`].
    fn sync_dir(&self, dir: &File, path: &Path) -> io::Result<()>;
}

/// The real thing: `File::sync_all`, which is `F_FULLFSYNC` on Apple targets.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyncAll;

impl Syncs for SyncAll {
    fn sync_file(&self, file: &File, _path: &Path) -> io::Result<()> {
        file.sync_all()
    }

    fn sync_dir(&self, dir: &File, _path: &Path) -> io::Result<()> {
        dir.sync_all()
    }
}

/// How durable a completed write is.
///
/// A write that reaches this type has already committed: the destination holds
/// the new bytes either way. The distinction is only what a power loss in the
/// next moments could still undo, and it is a return value rather than a log
/// line because 04 §6 wants losses reportable, never log-only.
#[derive(Debug)]
pub enum Commit {
    /// Staged, synced, renamed, and the directory synced: durable.
    Durable,
    /// Renamed, but the directory sync failed. The content is correct and
    /// visible to every reader; only the *entry* is not known to be on disk.
    DirectoryUnsynced(io::Error),
}

impl Commit {
    /// Whether the write survives a power loss right now.
    #[must_use]
    pub const fn is_durable(&self) -> bool {
        matches!(self, Self::Durable)
    }

    /// The directory sync failure, if there was one.
    #[must_use]
    pub const fn directory_error(&self) -> Option<&io::Error> {
        match self {
            Self::Durable => None,
            Self::DirectoryUnsynced(err) => Some(err),
        }
    }
}

/// Write `bytes` to `dir/name`, atomically and durably.
///
/// `dir` is created first if it is missing, syncing the parent of every
/// directory this creates so the directories themselves survive a power loss
/// (D-M1-4: the state directory and its `history/` are made on first write).
///
/// A reader of `dir/name` sees either the whole previous file or the whole new
/// one, never a mixture and never a truncated file: the payload is complete and
/// synced before the name is claimed. Leftover staging files for `name` — a
/// crash between step 1 and step 4 leaves one — are swept after the commit;
/// they are inert until then, since nothing ever reads a `.tmp` name.
///
/// # Errors
///
/// The directory could not be created, the payload could not be staged, the
/// staging file could not be synced, or the rename failed. In all four the
/// destination is untouched and the staging file is removed.
pub fn write_atomic(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    syncs: &impl Syncs,
) -> io::Result<Commit> {
    create_dir_synced(dir, syncs)?;

    let staged = dir.join(staging_name(name));
    if let Err(err) = stage(&staged, bytes, syncs) {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }

    let dest = dir.join(name);
    if let Err(err) = fs::rename(&staged, &dest) {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }

    let commit = sync_directory(dir, syncs);
    sweep_staging(dir, name);
    Ok(commit)
}

/// Remove `dir/name` if it is there; `true` if it was.
///
/// The directory sync afterwards is what makes the *removal* durable, for the
/// same reason step 5 of the write sequence exists — wiping scrollback
/// sidecars because the user turned history off is exactly the operation that
/// must not come back after a power loss (D-M1-6).
///
/// # Errors
///
/// The file existed and could not be removed.
pub fn remove_synced(dir: &Path, name: &str, syncs: &impl Syncs) -> io::Result<bool> {
    match fs::remove_file(dir.join(name)) {
        Ok(()) => {
            let _ = sync_directory(dir, syncs);
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Write the payload into the staging file and sync it.
fn stage(staged: &Path, bytes: &[u8], syncs: &impl Syncs) -> io::Result<()> {
    let mut file = File::create(staged)?;
    file.write_all(bytes)?;
    syncs.sync_file(&file, staged)
}

/// Sync `dir` itself, turning a failure into the reportable [`Commit`].
fn sync_directory(dir: &Path, syncs: &impl Syncs) -> Commit {
    match File::open(dir).and_then(|handle| syncs.sync_dir(&handle, dir)) {
        Ok(()) => Commit::Durable,
        Err(err) => Commit::DirectoryUnsynced(err),
    }
}

/// The staging name for `name`: unique per process and per write.
fn staging_name(name: &str) -> String {
    let counter = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
    format!("{name}.{}.{counter}{TMP_SUFFIX}", std::process::id())
}

/// Remove every staging file for `name` left in `dir`.
///
/// Best effort by design: cleanup failing is not a reason to fail a write that
/// already committed. Only files staged for `name` are swept, and a session's
/// state directory has exactly one writer — the server that claimed the
/// session's socket — so a staging file here is this session's own litter.
fn sweep_staging(dir: &Path, name: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{name}.");
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name.starts_with(&prefix) && file_name.ends_with(TMP_SUFFIX) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Create `dir` and any missing ancestors, syncing each new entry's parent.
///
/// `create_dir_all` with the durability step: a created directory whose parent
/// was never synced can be gone after a power loss, taking a perfectly synced
/// `session.json` inside it with it. A parent sync that fails is not fatal —
/// the directory exists and the write can proceed — for the same reason a
/// failed step 5 is reported rather than rolled back.
fn create_dir_synced(dir: &Path, syncs: &impl Syncs) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }

    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cursor = dir;
    while !cursor.is_dir() {
        missing.push(cursor.to_path_buf());
        match cursor.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => cursor = parent,
            _ => break,
        }
    }

    for path in missing.iter().rev() {
        match fs::create_dir(path) {
            Ok(()) => {}
            // Somebody else got there first, which is the outcome we wanted.
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
        if let Some(parent) = path.parent() {
            let _ = sync_directory(parent, syncs);
        }
    }
    Ok(())
}
