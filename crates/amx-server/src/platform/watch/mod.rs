//! Watching the user's `config.toml` for changes (D-M1-2).
//!
//! The watched set is one well-known path, so the watcher is built straight on
//! `rustix`: `inotify` on Linux, `kqueue` vnode filters on darwin. Both live
//! behind [`watch_config`], which hands back coalesced "the file may have
//! changed" notifications and nothing else — reading and parsing belong to the
//! consumer, so the platform modules stay descriptor plumbing.
//!
//! # Why the directory, not the file
//!
//! Editors and amx's own atomic writer (D-M1-1) replace a file by writing a
//! temporary beside it and renaming over the top. The new name points at a new
//! inode, so a watch scoped to the old file goes on watching a file nobody will
//! ever write again — silently, which is the worst kind of broken. Both
//! implementations therefore watch the containing *directory* and decide for
//! themselves which events were about `config.toml`.
//!
//! # Shape
//!
//! [`ConfigWatcher::changed`] is a method rather than a `Stream` because the
//! tree carries no `futures` dependency and the consumer is a `select!` loop,
//! which wants a future per branch, not a `Stream`. It is safe to drop
//! mid-wait: a change already observed is remembered and reported by the next
//! call.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use amx_core::Ctx;
use amx_core::platform::PlatformError;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::sync::CancellationToken;

use super::fd::{WakeWriter, wake_pipe};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(target_vendor = "apple")]
mod darwin;
#[cfg(target_vendor = "apple")]
use darwin as imp;

/// How long the watcher waits for a file to go quiet before reporting it.
///
/// A single save is several syscalls — an editor truncates, writes, renames,
/// chmods — and reloading once per syscall would be reloading a half-written
/// file. The value is D-M1-7's quiet window, reused deliberately: the milestone
/// has one pair of debounce constants, not one pair per debouncing thing.
pub const QUIET_WINDOW: Duration = Duration::from_millis(500);

/// The longest a change waits behind a stream of further changes.
///
/// A trailing window alone starves: a file written continuously never goes
/// quiet, so it would never be reported. This bounds that at D-M1-7's staleness
/// cap, measured from the first change of a burst.
pub const MAX_DELAY: Duration = Duration::from_secs(5);

/// How a burst of file activity is folded into one notification.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Debounce {
    /// How long the file must be idle before the change is reported.
    pub quiet: Duration,
    /// How long a change may be held back while activity continues.
    pub cap: Duration,
}

impl Default for Debounce {
    fn default() -> Self {
        Self {
            quiet: QUIET_WINDOW,
            cap: MAX_DELAY,
        }
    }
}

/// The config file may have changed; read it again.
///
/// Deliberately carries nothing. The watcher reports that the *name* was
/// touched, never what is behind it: an event says a rename happened, not what
/// the new file says, and re-reading is the only way to find out. Handing over
/// contents would also make every reload race the next write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConfigChanged;

/// What the platform layer reports for one wake-up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Notice {
    /// Something happened to the watched file.
    Changed,
    /// Something happened, but not to the watched file.
    Nothing,
    /// The wake pipe fired: the watcher was asked to stop.
    Stop,
}

/// How many notifications sit between the watcher thread and its consumer.
///
/// One, because they carry no information beyond "changed": a full channel
/// already says everything a second entry could, so the thread drops the
/// overflow instead of growing a queue of identical facts.
const CHANNEL: usize = 1;

/// Watch the config file named by `ctx` until `cancel` fires.
///
/// The parent directory is created if it is absent, so the watch always has a
/// directory to attach to and a config file that appears later is seen.
///
/// # Errors
///
/// Reports the platform error if the directory cannot be created or the
/// watch cannot be established.
pub fn watch_config(ctx: &Ctx, cancel: CancellationToken) -> Result<ConfigWatcher, PlatformError> {
    watch_path(&ctx.config_path, cancel, Debounce::default())
}

/// Watch one file path, coalescing bursts per `debounce`.
///
/// The seam [`watch_config`] is built on: tests drive it with a wound-down
/// [`Debounce`] so a suite does not spend its life waiting out quiet windows.
///
/// # Errors
///
/// Reports the platform error if `path` has no parent directory or file name,
/// if the directory cannot be created, or if the watch cannot be established.
pub fn watch_path(
    path: &Path,
    cancel: CancellationToken,
    debounce: Debounce,
) -> Result<ConfigWatcher, PlatformError> {
    let (dir, file) = split(path)?;
    std::fs::create_dir_all(&dir).map_err(PlatformError::Io)?;

    let pipe = wake_pipe().map_err(PlatformError::Io)?;
    let watch = imp::Watch::open(&dir, &file, pipe.reader).map_err(PlatformError::Io)?;
    let (tx, changes) = mpsc::channel(CHANNEL);
    let thread = std::thread::Builder::new()
        .name("amx-config-watch".to_owned())
        .spawn(move || run(watch, &tx))
        .map_err(PlatformError::Io)?;

    Ok(ConfigWatcher {
        changes,
        debounce,
        cancel,
        pending: false,
        wake: Some(pipe.writer),
        thread: Some(thread),
    })
}

/// Split a config path into the directory to watch and the name to filter on.
fn split(path: &Path) -> Result<(PathBuf, OsString), PlatformError> {
    let reject = |what: &str| {
        PlatformError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no {what}", path.display()),
        ))
    };
    let dir = path.parent().ok_or_else(|| reject("parent directory"))?;
    let file = path.file_name().ok_or_else(|| reject("file name"))?;
    Ok((dir.to_path_buf(), file.to_os_string()))
}

/// The watcher thread: park in the platform call, forward what it reports.
///
/// It never blocks on the channel. A full channel is success — the consumer
/// has an unread "changed" pending, which is the same fact this one carries —
/// and a closed one is the signal to end, which is what a dropped
/// [`ConfigWatcher`] that never woke us leaves behind.
fn run(mut watch: imp::Watch, tx: &mpsc::Sender<ConfigChanged>) {
    loop {
        match watch.wait() {
            Ok(Notice::Stop) => break,
            Ok(Notice::Nothing) => {}
            Ok(Notice::Changed) => match tx.try_send(ConfigChanged) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => break,
            },
            Err(err) => {
                // The watch is gone and nothing here can rebuild it; ending the
                // thread closes the channel, which tells the consumer that no
                // further change will ever be reported.
                tracing::warn!(error = %err, "config watch ended");
                break;
            }
        }
    }
}

/// A live watch on the config file.
///
/// Dropping it stops the watcher thread and joins it, so nothing outlives the
/// handle.
#[derive(Debug)]
pub struct ConfigWatcher {
    /// Raw pokes from the watcher thread, already coalesced to at most one.
    changes: mpsc::Receiver<ConfigChanged>,
    /// The windows a burst is folded into.
    debounce: Debounce,
    /// The session's shutdown signal.
    cancel: CancellationToken,
    /// A change observed by a [`ConfigWatcher::changed`] future that was
    /// dropped before it could report — held so the next call reports it.
    pending: bool,
    /// The far end of the watcher thread's wake pipe.
    wake: Option<WakeWriter>,
    /// The watcher thread, until it is joined.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ConfigWatcher {
    /// Wait for the next coalesced change.
    ///
    /// `None` means no further change will be reported: the token fired or the
    /// watcher thread ended. Safe to drop mid-wait — a change already observed
    /// is remembered rather than swallowed, which is what makes this usable as
    /// one branch of a `select!`.
    pub async fn changed(&mut self) -> Option<ConfigChanged> {
        let cancel = self.cancel.clone();
        if !self.pending {
            let first = tokio::select! {
                () = cancel.cancelled() => None,
                poke = self.changes.recv() => poke,
            };
            if first.is_none() {
                self.stop();
                return None;
            }
            self.pending = true;
        }

        let cap = tokio::time::sleep(self.debounce.cap);
        tokio::pin!(cap);
        loop {
            // A fresh quiet window per iteration: every poke re-arms it, which
            // is what makes the trailing edge of a burst the reported one.
            let quiet = tokio::time::sleep(self.debounce.quiet);
            tokio::select! {
                biased;
                () = &mut cap => break,
                () = cancel.cancelled() => break,
                // The thread ending mid-burst is still a change to report.
                poke = self.changes.recv() => {
                    if poke.is_none() {
                        break;
                    }
                }
                () = quiet => break,
            }
        }

        self.pending = false;
        Some(ConfigChanged)
    }

    /// Wake the watcher thread and join it. Idempotent.
    fn stop(&mut self) {
        if let Some(wake) = self.wake.take() {
            let _ = wake.wake();
        }
        if let Some(thread) = self.thread.take() {
            // Bounded: the thread is parked in a call the wake byte ends, and
            // it never blocks on the channel, so this is a return trip through
            // one syscall rather than a wait on anything.
            let _ = thread.join();
        }
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What makes one version of the watched file distinguishable from the next.
///
/// Only darwin needs it — kqueue reports that a directory changed without
/// naming the entry, so the file is re-identified by path — but it is pure
/// metadata arithmetic, so it lives here where a Linux test run can exercise
/// it. The identity half (device and inode) answers "is this a different
/// file?"; the whole answers "is this different content?".
#[cfg(any(target_vendor = "apple", test))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Stamp {
    /// The device the file lives on.
    device: u64,
    /// The file's inode.
    inode: u64,
    /// Its length.
    size: u64,
    /// Last modification, to the nanosecond the filesystem records.
    modified: (i64, i64),
    /// Last inode change — a rename over the top moves this when nothing else
    /// does.
    changed: (i64, i64),
}

#[cfg(any(target_vendor = "apple", test))]
impl Stamp {
    /// Stamp `path`, or `None` if nothing is there.
    ///
    /// Any error is absence: an unreadable file and a missing one are the same
    /// answer to "which file is at this name", and the caller's next step —
    /// report a change and look again — is right for both.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;

        let meta = std::fs::symlink_metadata(path).ok()?;
        Some(Self {
            device: meta.dev(),
            inode: meta.ino(),
            size: meta.size(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }

    /// Whether two stamps name the same file, ignoring its contents.
    pub(crate) fn is_same_file(left: Option<Self>, right: Option<Self>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => (left.device, left.inode) == (right.device, right.inode),
            (None, None) => true,
            _ => false,
        }
    }
}

/// Neither Linux nor darwin: no watcher, and an error that says exactly that
/// rather than a watch that never fires.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
mod imp {
    use std::ffi::OsStr;
    use std::io;
    use std::os::fd::OwnedFd;
    use std::path::Path;

    use super::Notice;

    /// A watch that was never established.
    #[derive(Debug)]
    pub(super) struct Watch;

    impl Watch {
        /// Always fails: config hot-reload is a Linux and darwin feature.
        /// Startup config loading is unaffected — it only reads the file.
        pub(super) fn open(_dir: &Path, _file: &OsStr, _wake: OwnedFd) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "watching config.toml needs inotify (Linux) or kqueue (darwin)",
            ))
        }

        /// Unreachable: [`Watch::open`] never returns a watch to wait on.
        pub(super) fn wait(&mut self) -> io::Result<Notice> {
            Ok(Notice::Stop)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Debounce, MAX_DELAY, QUIET_WINDOW, Stamp, split};

    #[test]
    fn the_default_debounce_is_the_documented_pair() {
        let debounce = Debounce::default();
        assert_eq!(debounce.quiet, QUIET_WINDOW);
        assert_eq!(debounce.cap, MAX_DELAY);
        assert!(
            debounce.quiet < debounce.cap,
            "a cap below the quiet window would report every burst at the cap",
        );
    }

    #[test]
    fn a_config_path_splits_into_the_directory_to_watch_and_the_name() {
        let (dir, file) = split(Path::new("/home/u/.config/amx/config.toml")).expect("split");
        assert_eq!(dir, Path::new("/home/u/.config/amx"));
        assert_eq!(file, "config.toml");
        assert!(
            split(Path::new("/")).is_err(),
            "a path with no file name is not a config file",
        );
    }

    #[test]
    fn a_stamp_tells_replacement_from_absence_and_from_a_rewrite() {
        let dir = std::env::temp_dir().join(format!("amx-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.toml");
        assert_eq!(Stamp::of(&path), None, "nothing is there yet");

        std::fs::write(&path, "history = false\n").expect("write");
        let first = Stamp::of(&path);
        assert!(first.is_some());
        assert!(
            !Stamp::is_same_file(first, None),
            "a file that exists is not the file that did not",
        );

        // The atomic writer's shape: a new inode arrives under the same name.
        let tmp = dir.join("config.toml.tmp");
        std::fs::write(&tmp, "history = true\n").expect("write");
        std::fs::rename(&tmp, &path).expect("rename");
        let second = Stamp::of(&path);
        assert_ne!(first, second, "a rename over the top is a change");
        assert!(
            !Stamp::is_same_file(first, second),
            "and it is a different file, so a file-scoped watch is now stale",
        );
        assert!(Stamp::is_same_file(second, Stamp::of(&path)));

        std::fs::remove_file(&path).expect("remove");
        assert_eq!(Stamp::of(&path), None);
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
