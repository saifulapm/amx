//! Staging, and the one rename that is the install.
//!
//! # Why a staging directory at all
//!
//! Because a download is not an install and must never be able to become one by
//! accident. The bytes land under the session's state directory
//! ([`staging_dir`]), where nothing execs them and where a crash leaves a file
//! nobody will run; the checksum runs there; and only a verified file is
//! carried to where amx lives.
//!
//! # Why the rename is safe on a running binary
//!
//! `rename(2)` replaces a *directory entry*. The running process holds its image
//! by inode, not by name, so it keeps executing the old file — including pages
//! it has not faulted in yet — until it exits, and the next exec of the path
//! gets the new one. `ETXTBSY` is the kernel refusing a *write* into the image
//! of a running program; it does not apply here, which is exactly why the
//! install is a rename and not a write.
//!
//! # Why the copy in the middle
//!
//! `rename(2)` cannot cross filesystems, and the state directory (under
//! `$XDG_STATE_HOME`) and the binary (under `/usr/local/bin`, `~/.local/bin`,
//! `/nix/store`, …) routinely live on different ones. So [`install`] copies the
//! verified file to a hidden sibling of the target — same directory, therefore
//! same filesystem, guaranteed — fsyncs it, and renames that. The atomic step
//! is still one rename; the copy just puts it where a rename is possible.

use std::fs;
use std::io;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use super::manifest::Asset;
use super::{fetch, verify};

/// The mode an installed amx is given.
///
/// Fixed rather than copied from whatever is being replaced: the file must be
/// executable by the user who just installed it, and inheriting the mode of the
/// old binary would carry over a bad one silently.
pub const MODE: u32 = 0o755;

/// Where downloads land, under a session's state directory.
#[must_use]
pub fn staging_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("update")
}

/// A downloaded, verified binary that has not been installed.
///
/// Removed when dropped. Every failure path between the download and the
/// rename therefore leaves nothing behind, which is what lets `apply` say
/// "nothing was installed" and be telling the whole truth.
#[derive(Debug)]
pub struct Staged {
    /// `None` once the file has been moved into place.
    path: Option<PathBuf>,
}

impl Staged {
    /// Where the staged binary is.
    ///
    /// # Panics
    ///
    /// Never: the `None` state exists only inside [`install`], after which the
    /// value is dropped without being read again.
    #[must_use]
    pub fn path(&self) -> &Path {
        // The invariant: `path` is `Some` for the whole public life of the
        // value; only `install` takes it, and only as it consumes `self`.
        #[allow(
            clippy::expect_used,
            reason = "stated invariant: taken only by install"
        )]
        self.path.as_deref().expect("a staged file has a path")
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

/// Download `asset` into the staging directory and verify it.
///
/// # Errors
///
/// If the staging directory cannot be made, the download fails, or the digest
/// does not match what the manifest published. In every one of those cases the
/// staged file is gone by the time the error is returned.
pub async fn stage(state_dir: &Path, asset: &Asset) -> anyhow::Result<Staged> {
    let dir = staging_dir(state_dir);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    // Named by pid so two `amx update apply` runs cannot land on one file.
    let path = dir.join(format!("amx-{}", std::process::id()));
    let _ = fs::remove_file(&path);

    fetch::asset(&asset.url, &path).await?;
    // From here on the file is owned: `Staged`'s `Drop` removes it unless the
    // install takes it, so no failure below needs its own cleanup.
    let staged = Staged {
        path: Some(path.clone()),
    };
    verify::verify(&path, &asset.sha256)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(MODE))
        .with_context(|| format!("make {} executable", path.display()))?;
    Ok(staged)
}

/// Put the staged binary at `target`, atomically.
///
/// # Errors
///
/// If the directory holding `target` cannot be written, or the copy or rename
/// fails. `target` is untouched unless the rename itself succeeded, and the
/// rename is the last thing that happens.
pub fn install(mut staged: Staged, target: &Path) -> anyhow::Result<()> {
    // The invariant: `path` is only ever taken here, as the value is consumed.
    #[allow(clippy::expect_used, reason = "stated invariant: taken only here")]
    let source = staged.path.take().expect("a staged file has a path");
    // Removed by hand from here on, since `Drop` no longer will.
    let outcome = place(&source, target);
    let _ = fs::remove_file(&source);
    outcome
}

/// The copy-beside-then-rename this module's docs describe.
fn place(source: &Path, target: &Path) -> anyhow::Result<()> {
    let dir = target
        .parent()
        .with_context(|| format!("{} has no directory to install into", target.display()))?;
    let beside = dir.join(format!(".amx-update-{}", std::process::id()));

    let outcome = copy(source, &beside).with_context(|| {
        format!(
            "stage the new amx beside {} — is {} writable?",
            target.display(),
            dir.display()
        )
    });
    if let Err(err) = outcome {
        let _ = fs::remove_file(&beside);
        return Err(err);
    }
    if let Err(err) = fs::rename(&beside, target) {
        let _ = fs::remove_file(&beside);
        return Err(anyhow::Error::from(err)
            .context(format!("rename the new amx over {}", target.display())));
    }
    Ok(())
}

/// Copy `source` to `dest`, executable and on the disk before it is renamed.
///
/// The fsync is not ceremony: without it the rename can be durable while the
/// bytes behind it are not, and the file a crash leaves at the path amx is
/// invoked by would be the one file on the system that must never be truncated.
fn copy(source: &Path, dest: &Path) -> io::Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(MODE)
        .open(dest)?;
    io::copy(&mut input, &mut output)?;
    output.set_permissions(fs::Permissions::from_mode(MODE))?;
    output.sync_all()
}
