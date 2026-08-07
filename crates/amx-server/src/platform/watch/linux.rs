//! The inotify half of the config watcher.
//!
//! One watch on the config directory, filtered to the one name that matters.
//! inotify attributes every event to an entry by name, so the filtering is
//! exact and no file has to be stat'd to find out whether an event was ours.
//!
//! The thread parks in `poll()` over two descriptors — the inotify queue and
//! the wake pipe — which is how a cancelled watcher stops at once instead of
//! at the end of some timeout.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;

use super::Notice;

/// Everything that can happen to an entry in the config directory.
///
/// `CREATE`/`MOVED_TO` catch the file appearing, `DELETE`/`MOVED_FROM` it
/// leaving, `MODIFY` an in-place write and `CLOSE_WRITE` the close that ends
/// one (an editor that writes through a mapping produces the second without
/// the first), `ATTRIB` a chmod. `DELETE_SELF`/`MOVE_SELF` are about the
/// directory itself, which is how the watch learns it has been orphaned.
/// `ONLYDIR` makes a non-directory an error at registration rather than a
/// watch that never fires.
const WATCHED: WatchFlags = WatchFlags::CREATE
    .union(WatchFlags::DELETE)
    .union(WatchFlags::MODIFY)
    .union(WatchFlags::CLOSE_WRITE)
    .union(WatchFlags::MOVED_TO)
    .union(WatchFlags::MOVED_FROM)
    .union(WatchFlags::ATTRIB)
    .union(WatchFlags::DELETE_SELF)
    .union(WatchFlags::MOVE_SELF)
    .union(WatchFlags::ONLYDIR);

/// How much of the inotify queue one read collects.
///
/// An `inotify_event` is 16 bytes plus a name of at most `NAME_MAX + 1`, so
/// this holds a dozen worst-case events and dozens of realistic ones. A short
/// buffer is not a correctness problem — the reader loops until the queue is
/// empty — only a syscall-count one.
const BUF_LEN: usize = 4096;

/// One inotify watch on the directory holding the config file.
#[derive(Debug)]
pub(super) struct Watch {
    /// The inotify queue, non-blocking so the drain ends on `AGAIN`.
    inotify: OwnedFd,
    /// The read end of the wake pipe: readable means "stop".
    wake: OwnedFd,
    /// The watched directory, kept for re-establishing the watch.
    dir: PathBuf,
    /// The entry name events must carry to count.
    file: OsString,
    /// The current watch descriptor, so events queued against a previous one
    /// are recognised as stale.
    descriptor: i32,
}

impl Watch {
    /// Establish the watch. `dir` must already exist.
    pub(super) fn open(dir: &Path, file: &OsStr, wake: OwnedFd) -> io::Result<Self> {
        let inotify = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        let descriptor = inotify::add_watch(&inotify, dir, WATCHED)?;
        Ok(Self {
            inotify,
            wake,
            dir: dir.to_path_buf(),
            file: file.to_os_string(),
            descriptor,
        })
    }

    /// Park until something happens, then say what it was.
    pub(super) fn wait(&mut self) -> io::Result<Notice> {
        if self.park()? {
            return Ok(Notice::Stop);
        }
        self.drain()
    }

    /// Block until the queue has events or the wake pipe fires.
    ///
    /// Returns whether it was the wake pipe. A hangup on either descriptor is
    /// also a stop: the only writer to the pipe is the handle that owns this
    /// thread, so its disappearance means the same thing a wake byte does.
    fn park(&self) -> io::Result<bool> {
        let mut fds = [
            PollFd::from_borrowed_fd(self.wake.as_fd(), PollFlags::IN),
            PollFd::from_borrowed_fd(self.inotify.as_fd(), PollFlags::IN),
        ];
        loop {
            match poll(&mut fds, None::<&Timespec>) {
                Ok(_) => break,
                // No timeout to restart: the park is unbounded by design.
                Err(Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        let ended = PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL;
        Ok(fds[0].revents().intersects(PollFlags::IN | ended))
    }

    /// Read every queued event and fold them into one answer.
    fn drain(&mut self) -> io::Result<Notice> {
        let mut buf = [MaybeUninit::uninit(); BUF_LEN];
        let mut reader = inotify::Reader::new(&self.inotify, &mut buf);
        let mut changed = false;
        let mut orphaned = false;

        loop {
            let event = match reader.next() {
                Ok(event) => event,
                Err(Errno::AGAIN) => break,
                Err(Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            };
            let flags = event.events();
            if flags.contains(ReadFlags::QUEUE_OVERFLOW) {
                // The kernel dropped events to make room. Any of them could
                // have been ours, so the only safe reading is "changed" — and
                // the overflow event carries no watch descriptor to check.
                changed = true;
                continue;
            }
            if event.wd() != self.descriptor {
                // Queued against a watch that has already been replaced.
                continue;
            }
            if flags.intersects(ReadFlags::IGNORED | ReadFlags::UNMOUNT) {
                orphaned = true;
                continue;
            }
            match event.file_name() {
                Some(name) if name.to_bytes() == self.file.as_bytes() => changed = true,
                // Another entry in the same directory — a neighbouring file, or
                // the staging file of a rename that has not landed yet.
                Some(_) => {}
                // Unnamed events are about the directory itself.
                None => {
                    orphaned |= flags.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF);
                }
            }
        }

        if orphaned {
            // The directory the watch was attached to is gone. Recreate it and
            // re-attach: the config directory is amx's own, and losing it must
            // not silently end hot reload. Whatever happened to the file while
            // there was no watch is unknown, so this counts as a change.
            self.reattach()?;
            return Ok(Notice::Changed);
        }
        Ok(if changed {
            Notice::Changed
        } else {
            Notice::Nothing
        })
    }

    /// Recreate the config directory and put the watch back on it.
    fn reattach(&mut self) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        self.descriptor = inotify::add_watch(&self.inotify, &self.dir, WATCHED)?;
        Ok(())
    }
}
