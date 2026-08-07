//! The kqueue half of the config watcher.
//!
//! kqueue watches *descriptors*, not names, and its vnode filter says only
//! that something changed — never which entry. Both facts shape this module:
//!
//! - **Two filters, not one.** The directory descriptor reports that its entry
//!   set changed, which is what catches a file appearing, vanishing, or being
//!   renamed over. The file's own descriptor reports writes to the file it was
//!   opened on, which is what catches an in-place edit and, being attributed
//!   already, needs no second look. Either one alone has a blind spot: a
//!   file-scoped watch goes on watching an unlinked inode after a rename, and
//!   directory events cannot tell `config.toml` from its neighbours.
//! - **Re-arming.** After a rename over the top, the descriptor that was
//!   `config.toml` is a descriptor for a deleted inode, so the file filter is
//!   re-opened by path whenever the file's identity changes.
//! - **Re-stat, not guesswork.** Because a directory event names nothing, the
//!   file is re-identified by path after every batch and the answer comes from
//!   comparing [`Stamp`]s (R-M1-6: kqueue is coarser than inotify, and this is
//!   where that cost is paid).
//!
//! # Not executable on Linux
//!
//! macOS is tier 1 and CI runs this suite there, but a Linux development
//! machine can only compile this module (`cargo check --target
//! aarch64-apple-darwin`). What follows is written from kqueue(2)'s
//! documented behaviour: `NOTE_WRITE` on a directory descriptor fires when its
//! entry set changes, and a filter is removed automatically when the
//! descriptor it references is closed. The parts that are *not* darwin-only —
//! the stamp comparison and the debounce — are exercised on both platforms.

use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::Duration;

use rustix::event::kqueue::{Event, EventFilter, EventFlags, VnodeEvents, kevent, kqueue};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use super::{Notice, Stamp};

/// Register the filter, and reset it after each report.
///
/// `EV_CLEAR` is what makes the filter edge-triggered: without it a vnode that
/// changed once stays permanently ready and the thread spins.
const ARM: EventFlags = EventFlags::ADD.union(EventFlags::CLEAR);

/// What a directory reports: its entry set changed.
///
/// `NOTE_WRITE` is the one that matters — a create, a delete or a rename into
/// the directory is a write to it. The rest are cheap to ask for and cover a
/// directory that is itself replaced.
const DIR_EVENTS: VnodeEvents = VnodeEvents::WRITE
    .union(VnodeEvents::EXTEND)
    .union(VnodeEvents::DELETE)
    .union(VnodeEvents::RENAME)
    .union(VnodeEvents::LINK);

/// What the config file itself reports.
///
/// `NOTE_DELETE` is the rename-over case seen from the old inode's side: the
/// last link to it goes away. `NOTE_REVOKE` is a forced unmount.
const FILE_EVENTS: VnodeEvents = VnodeEvents::WRITE
    .union(VnodeEvents::EXTEND)
    .union(VnodeEvents::DELETE)
    .union(VnodeEvents::RENAME)
    .union(VnodeEvents::ATTRIBUTES)
    .union(VnodeEvents::REVOKE)
    .union(VnodeEvents::LINK);

/// How many events one `kevent` call collects.
///
/// Nothing is lost by a short list — the next call returns the rest, and every
/// event in a batch folds into the same answer anyway.
const BATCH: usize = 16;

/// An empty change list, spelled once so the call sites stay readable.
const NO_CHANGES: &[Event] = &[];

/// Two vnode filters and a wake pipe on one kqueue.
#[derive(Debug)]
pub(super) struct Watch {
    /// The queue everything is registered on.
    queue: OwnedFd,
    /// The config directory.
    directory: OwnedFd,
    /// The config file, when there is one to open.
    file: Option<OwnedFd>,
    /// The read end of the wake pipe: readable means "stop".
    wake: OwnedFd,
    /// The config file's path, re-opened after every replacement.
    path: PathBuf,
    /// What the file looked like when it was last examined.
    stamp: Option<Stamp>,
}

impl Watch {
    /// Establish the watch. `dir` must already exist.
    pub(super) fn open(dir: &Path, file: &OsStr, wake: OwnedFd) -> io::Result<Self> {
        let queue = kqueue()?;
        let directory = open_to_watch(dir, OFlags::DIRECTORY)?;
        let mut watch = Self {
            queue,
            directory,
            file: None,
            wake,
            path: dir.join(file),
            stamp: None,
        };
        watch.apply(&[
            vnode(watch.directory.as_raw_fd(), DIR_EVENTS),
            Event::new(
                EventFilter::Read(watch.wake.as_raw_fd()),
                ARM,
                ptr::null_mut(),
            ),
        ])?;
        watch.stamp = Stamp::of(&watch.path);
        watch.arm_file()?;
        Ok(watch)
    }

    /// Park until something happens, then say what it was.
    pub(super) fn wait(&mut self) -> io::Result<Notice> {
        let mut batch = [placeholder(); BATCH];
        let count = loop {
            // SAFETY: `kevent` requires every descriptor named by a registered
            // filter to stay valid for the lifetime of the queue. All three are
            // owned by `self` — the queue is dropped with them, and the one
            // descriptor that is ever closed early (the file, on replacement)
            // is closed by `arm_file`, which kqueue(2) documents as removing
            // its filter.
            match unsafe { kevent(&self.queue, NO_CHANGES, &mut batch[..], None) } {
                Ok(count) => break count,
                Err(Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            }
        };

        let mut changed = false;
        for event in &batch[..count] {
            match event.filter() {
                EventFilter::Read(fd) if fd == self.wake.as_raw_fd() => return Ok(Notice::Stop),
                // An event on the file's own descriptor is attributed already.
                // A descriptor number freed by `arm_file` and reused by the
                // re-open could in principle carry a stale event here; the cost
                // is one extra "changed", which the debounce absorbs.
                EventFilter::Vnode { vnode, .. } if Some(vnode) == self.watched_file() => {
                    changed = true;
                }
                // A directory event names no entry, so it decides nothing on
                // its own — `examine` below does.
                EventFilter::Vnode { .. } => {}
                _ => {}
            }
        }

        changed |= self.examine()?;
        Ok(if changed {
            Notice::Changed
        } else {
            Notice::Nothing
        })
    }

    /// The descriptor the file filter is currently attached to.
    fn watched_file(&self) -> Option<RawFd> {
        self.file.as_ref().map(AsRawFd::as_raw_fd)
    }

    /// Re-identify the file by path, re-arming the filter if it was replaced.
    ///
    /// Returns whether the file differs from the last time it was examined —
    /// which is how a directory event that was about some neighbouring file is
    /// told from one that was about ours.
    fn examine(&mut self) -> io::Result<bool> {
        let stamp = Stamp::of(&self.path);
        if !Stamp::is_same_file(stamp, self.stamp) {
            self.arm_file()?;
        }
        let changed = stamp != self.stamp;
        self.stamp = stamp;
        Ok(changed)
    }

    /// Point the file filter at whatever the config path names now.
    ///
    /// Closing the previous descriptor is what deregisters its filter. A file
    /// that cannot be opened — it is absent, or unreadable — leaves no file
    /// filter at all, which is correct rather than fatal: the directory filter
    /// still reports the moment one appears.
    fn arm_file(&mut self) -> io::Result<()> {
        self.file = None;
        let Ok(file) = open_to_watch(&self.path, OFlags::empty()) else {
            return Ok(());
        };
        self.apply(&[vnode(file.as_raw_fd(), FILE_EVENTS)])?;
        self.file = Some(file);
        Ok(())
    }

    /// Submit `changes` and collect no events.
    ///
    /// With no room in the event list, a change kqueue refuses comes back as
    /// the call's own error rather than as an `EV_ERROR` entry to be found
    /// later, which is why the registration path is separate from the wait.
    fn apply(&self, changes: &[Event]) -> io::Result<()> {
        let mut none: [Event; 0] = [];
        // SAFETY: as in `wait` — every descriptor named in `changes` is owned
        // by `self` (or, in `arm_file`, held by the caller across this call and
        // then stored on `self`).
        unsafe { kevent(&self.queue, changes, &mut none[..], Some(Duration::ZERO)) }?;
        Ok(())
    }
}

/// A vnode filter registration for `fd`.
fn vnode(fd: RawFd, flags: VnodeEvents) -> Event {
    Event::new(
        EventFilter::Vnode { vnode: fd, flags },
        ARM,
        ptr::null_mut(),
    )
}

/// A slot for the kernel to write an event into.
///
/// `Event` has no `Default` and every slot is overwritten before it is read, so
/// the value only has to exist; a read filter on descriptor −1 is the cheapest
/// one to spell and is never submitted as a change.
fn placeholder() -> Event {
    Event::new(EventFilter::Read(-1), EventFlags::empty(), ptr::null_mut())
}

/// Open a descriptor purely to watch it.
///
/// macOS prefers `O_EVTONLY` here — it does not count as a reference that holds
/// a filesystem busy — but rustix 1.1.4's `OFlags` does not expose it, so this
/// opens read-only. The difference shows only while unmounting the filesystem
/// that holds `$XDG_CONFIG_HOME`.
fn open_to_watch(path: &Path, extra: OFlags) -> io::Result<OwnedFd> {
    Ok(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | extra,
        Mode::empty(),
    )?)
}
