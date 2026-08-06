//! The Unix side of the [`ProcessTree`] seam.
//!
//! One public type, per-OS readers behind it: Linux answers out of `/proc`,
//! macOS out of libproc, and any other Unix degrades to the contract's
//! defined fallbacks (a warning once, then `NotFound`/empty). Every answer
//! is a snapshot that may already be stale — the process can exit between
//! the read and the use — which is why the contract gives callers a defined
//! fallback rather than promising accuracy.

use std::path::PathBuf;

use amx_core::platform::{PlatformError, ProcessId, ProcessTree};
use rustix::io::Errno;
use rustix::process::Pid;

/// Reading the process tree on Unix.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct UnixProcessTree;

impl ProcessTree for UnixProcessTree {
    fn cwd(&self, process: ProcessId) -> Result<PathBuf, PlatformError> {
        imp::cwd(process)
    }

    fn children(&self, process: ProcessId) -> Result<Vec<ProcessId>, PlatformError> {
        imp::children(process)
    }

    fn is_alive(&self, process: ProcessId) -> bool {
        let Ok(raw) = i32::try_from(process.0) else {
            return false;
        };
        let Some(pid) = Pid::from_raw(raw) else {
            return false;
        };
        // A process this user may not signal is still a process: only "no such
        // process" is an answer of no.
        !matches!(rustix::process::test_kill_process(pid), Err(Errno::SRCH))
    }
}

/// The `/proc` reader.
#[cfg(target_os = "linux")]
mod imp {
    use std::path::{Path, PathBuf};

    use amx_core::platform::{PlatformError, ProcessId};

    /// Where the process table is mounted.
    const PROC: &str = "/proc";

    pub(super) fn cwd(process: ProcessId) -> Result<PathBuf, PlatformError> {
        let link = Path::new(PROC).join(process.0.to_string()).join("cwd");
        std::fs::read_link(link).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                PlatformError::NotFound
            }
            _ => PlatformError::Io(err),
        })
    }

    pub(super) fn children(process: ProcessId) -> Result<Vec<ProcessId>, PlatformError> {
        // `/proc/<pid>/task/<tid>/children` would answer this directly, but it
        // is behind `CONFIG_PROC_CHILDREN` and is not there on every kernel;
        // the parent link in `stat` always is. This is not a hot path — it
        // runs when a pane is inspected, not when it draws.
        let mut children = Vec::new();
        for entry in std::fs::read_dir(PROC).map_err(PlatformError::Io)? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if parent_of(pid) == Some(process.0) {
                children.push(ProcessId(pid));
            }
        }
        children.sort_unstable();
        Ok(children)
    }

    /// The parent pid recorded in `/proc/<pid>/stat`, or `None` if it cannot
    /// be read.
    ///
    /// The comm field is an arbitrary string in parentheses and may contain
    /// spaces and parentheses of its own, so the fields are counted from the
    /// *last* `)` rather than split from the front.
    fn parent_of(pid: u32) -> Option<u32> {
        let stat =
            std::fs::read_to_string(Path::new(PROC).join(pid.to_string()).join("stat")).ok()?;
        let after_comm = &stat[stat.rfind(')')? + 1..];
        // The fields after the comm are: state, ppid, ...
        after_comm.split_whitespace().nth(1)?.parse().ok()
    }
}

/// The libproc reader.
///
/// macOS mounts no `/proc`; `proc_pidinfo` and `proc_listchildpids` are the
/// sanctioned answers. Return conventions verified against the wrapper source
/// (xnu `libsyscall/wrappers/libproc/libproc.c`): `proc_pidinfo` returns the
/// bytes it wrote and 0 on error with `errno` set; `proc_listchildpids`
/// returns a *count* of pids (its inner byte count divided by `sizeof(int)`),
/// and a null buffer sizes the follow-up call.
#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_int;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::PathBuf;

    use amx_core::platform::{PlatformError, ProcessId};

    pub(super) fn cwd(process: ProcessId) -> Result<PathBuf, PlatformError> {
        let pid = pid_of(process)?;
        // SAFETY: `proc_pidinfo` writes at most `size` bytes into `info`,
        // which is plain old data for which zeroed bytes are a valid value.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_vnodepathinfo>();
        let wrote = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                (&raw mut info).cast(),
                size as c_int,
            )
        };
        if wrote <= 0 {
            return Err(errno_error());
        }
        if (wrote as usize) < size {
            // A short answer is not an errno case; treat it as unreadable.
            return Err(PlatformError::NotFound);
        }
        // The path is NUL-terminated inside a MAXPATHLEN field; libc models
        // the field as nested arrays, but the bytes are one flat C string.
        // SAFETY: the cast reads the same POD bytes the kernel just wrote.
        let raw: &[u8] = unsafe {
            std::slice::from_raw_parts(
                (&raw const info.pvi_cdir.vip_path).cast(),
                size_of_val(&info.pvi_cdir.vip_path),
            )
        };
        let len = raw
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(PlatformError::NotFound)?;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&raw[..len])))
    }

    pub(super) fn children(process: ProcessId) -> Result<Vec<ProcessId>, PlatformError> {
        let pid = pid_of(process)?;
        // SAFETY: a null buffer asks for the count needed and writes nothing.
        let needed = unsafe { libc::proc_listchildpids(pid, std::ptr::null_mut(), 0) };
        let Ok(needed) = usize::try_from(needed) else {
            return Err(errno_error());
        };
        // Slack for children born between the two calls; the kernel fills at
        // most `buffer.len()` entries and reports how many.
        let mut buffer = vec![0 as libc::pid_t; needed + 8];
        let bytes = size_of_val(buffer.as_slice());
        // SAFETY: the buffer is `bytes` bytes of writable pid-sized slots.
        let filled =
            unsafe { libc::proc_listchildpids(pid, buffer.as_mut_ptr().cast(), bytes as c_int) };
        let Ok(filled) = usize::try_from(filled) else {
            return Err(errno_error());
        };
        buffer.truncate(filled.min(buffer.len()));
        let mut children: Vec<ProcessId> = buffer
            .into_iter()
            .filter_map(|pid| u32::try_from(pid).ok().map(ProcessId))
            .collect();
        children.sort_unstable();
        Ok(children)
    }

    /// A `ProcessId` as the `pid_t` libproc takes.
    fn pid_of(process: ProcessId) -> Result<c_int, PlatformError> {
        c_int::try_from(process.0).map_err(|_| PlatformError::NotFound)
    }

    /// The current `errno`, folded onto the seam's contract: a process that
    /// is gone or unreadable is `NotFound`, anything else is an I/O error.
    fn errno_error() -> PlatformError {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH | libc::EPERM | libc::ENOENT) => PlatformError::NotFound,
            _ => PlatformError::Io(err),
        }
    }
}

/// The honest fallback for Unixes without a reader: the contract's defined
/// degradation (callers fall back to the pane's recorded cwd, and treat "no
/// children" as "ask the shell"), announced once instead of silently.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    use amx_core::platform::{PlatformError, ProcessId};

    fn warn_once() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(
                os = std::env::consts::OS,
                "no process-tree reader for this platform; \
                 pane cwd and child inspection degrade to their fallbacks"
            );
        });
    }

    pub(super) fn cwd(_process: ProcessId) -> Result<PathBuf, PlatformError> {
        warn_once();
        Err(PlatformError::NotFound)
    }

    pub(super) fn children(_process: ProcessId) -> Result<Vec<ProcessId>, PlatformError> {
        warn_once();
        Ok(Vec::new())
    }
}
