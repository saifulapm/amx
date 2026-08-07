//! The Unix side of the [`ProcessTree`] seam.
//!
//! One public type, per-OS readers behind it: Linux answers out of `/proc`,
//! macOS out of libproc, and any other Unix degrades to the contract's
//! defined fallbacks (a warning once, then `NotFound`/empty). Every answer
//! is a snapshot that may already be stale — the process can exit between
//! the read and the use — which is why the contract gives callers a defined
//! fallback rather than promising accuracy.

use std::ffi::OsString;
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

    fn argv(&self, process: ProcessId) -> Result<Vec<OsString>, PlatformError> {
        imp::argv(process)
    }

    fn exe(&self, process: ProcessId) -> Result<PathBuf, PlatformError> {
        imp::exe(process)
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
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};

    use amx_core::platform::{PlatformError, ProcessId};

    /// Where the process table is mounted.
    const PROC: &str = "/proc";

    pub(super) fn cwd(process: ProcessId) -> Result<PathBuf, PlatformError> {
        let link = Path::new(PROC).join(process.0.to_string()).join("cwd");
        std::fs::read_link(link).map_err(unreadable)
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

    /// `/proc/<pid>/cmdline`: the argv as NUL-delimited bytes.
    ///
    /// The file is a copy of the process's own argument area, so a program
    /// that rewrote it (a `setproctitle`) is reported as it rewrote it — which
    /// is the honest answer, since that is also what `ps` shows the user.
    /// Kernel threads and zombies have an empty file, and empty is `Ok`.
    pub(super) fn argv(process: ProcessId) -> Result<Vec<OsString>, PlatformError> {
        let path = Path::new(PROC).join(process.0.to_string()).join("cmdline");
        let raw = std::fs::read(path).map_err(unreadable)?;
        // The area is NUL-*terminated*, not NUL-separated: splitting the
        // trailing terminator would append an empty argument to every argv.
        let raw = raw.strip_suffix(&[0]).unwrap_or(&raw);
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        Ok(raw
            .split(|byte| *byte == 0)
            .map(|token| OsStr::from_bytes(token).to_os_string())
            .collect())
    }

    pub(super) fn exe(process: ProcessId) -> Result<PathBuf, PlatformError> {
        let link = Path::new(PROC).join(process.0.to_string()).join("exe");
        std::fs::read_link(link).map_err(unreadable)
    }

    /// The two ways `/proc` says "not for you": the process is gone, or this
    /// user may not look at it. Both are the seam's `NotFound`.
    fn unreadable(err: std::io::Error) -> PlatformError {
        match err.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                PlatformError::NotFound
            }
            _ => PlatformError::Io(err),
        }
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
    use std::ffi::{OsStr, OsString, c_int, c_uint};
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

    /// The argv, out of `sysctl(KERN_PROCARGS2)`.
    ///
    /// libproc does not wrap this one, so the buffer layout is read directly.
    /// It comes from xnu's `sysctl_procargsx` (`bsd/kern/kern_sysctl.c`), which
    /// writes, in this order:
    ///
    /// 1. `argc`, as a native-endian `c_int`;
    /// 2. the executable path, NUL-terminated;
    /// 3. zero or more NUL bytes of alignment padding;
    /// 4. `argc` NUL-terminated argument strings;
    /// 5. the environment, which this function stops before.
    ///
    /// Every malformed shape — a short buffer, a path with no terminator, a
    /// truncated argument — is [`NotFound`](PlatformError::NotFound) rather
    /// than a partial answer, because a half-read argv would identify a pane
    /// as the wrong program, which is worse than not identifying it.
    pub(super) fn argv(process: ProcessId) -> Result<Vec<OsString>, PlatformError> {
        let raw = procargs(process)?;
        let Some((count, rest)) = raw.split_at_checked(size_of::<c_int>()) else {
            return Err(PlatformError::NotFound);
        };
        // `try_into` cannot fail: the split above is exactly this many bytes.
        let argc = c_int::from_ne_bytes(count.try_into().unwrap_or_default());
        let Ok(argc) = usize::try_from(argc) else {
            return Err(PlatformError::NotFound);
        };

        // Step past the executable path and the padding behind it. The first
        // NUL ends the path; the arguments start at the first non-NUL after it.
        let path_end = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(PlatformError::NotFound)?;
        let mut cursor = path_end;
        while rest.get(cursor) == Some(&0) {
            cursor += 1;
        }

        let mut argv = Vec::with_capacity(argc);
        for _ in 0..argc {
            let start = cursor;
            while rest.get(cursor).is_some_and(|byte| *byte != 0) {
                cursor += 1;
            }
            if cursor >= rest.len() {
                // The kernel promised `argc` arguments and the buffer ran out:
                // report nothing rather than a truncated last argument.
                return Err(PlatformError::NotFound);
            }
            argv.push(OsStr::from_bytes(&rest[start..cursor]).to_os_string());
            cursor += 1;
        }
        Ok(argv)
    }

    /// The raw `KERN_PROCARGS2` buffer for one process.
    ///
    /// Sized from `KERN_ARGMAX`, which is what the kernel itself caps the area
    /// at; `sysctl` then reports how much of it it wrote.
    fn procargs(process: ProcessId) -> Result<Vec<u8>, PlatformError> {
        let pid = pid_of(process)?;
        let mut argmax: c_int = 0;
        let mut size = size_of::<c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        // SAFETY: `mib` is two entries as `namelen` says, and the answer is one
        // `c_int` written into `argmax`, whose length `size` carries in and out.
        let read = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as c_uint,
                (&raw mut argmax).cast(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 {
            return Err(errno_error());
        }
        let capacity = usize::try_from(argmax).unwrap_or_default();
        if capacity == 0 {
            return Err(PlatformError::NotFound);
        }

        let mut buffer = vec![0u8; capacity];
        let mut size = buffer.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: `buffer` is `size` writable bytes and the call writes at most
        // that many, reporting the count back through `size`.
        let read = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as c_uint,
                buffer.as_mut_ptr().cast(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 {
            return Err(errno_error());
        }
        buffer.truncate(size.min(capacity));
        Ok(buffer)
    }

    pub(super) fn exe(process: ProcessId) -> Result<PathBuf, PlatformError> {
        let pid = pid_of(process)?;
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the call writes at most `buffersize` bytes into `buffer`.
        let wrote =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if wrote <= 0 {
            return Err(errno_error());
        }
        buffer.truncate(wrote as usize);
        Ok(PathBuf::from(OsStr::from_bytes(&buffer)))
    }

    /// A `ProcessId` as the `pid_t` libproc takes.
    fn pid_of(process: ProcessId) -> Result<c_int, PlatformError> {
        c_int::try_from(process.0).map_err(|_| PlatformError::NotFound)
    }

    /// The current `errno`, folded onto the seam's contract: a process that
    /// is gone or unreadable is `NotFound`, anything else is an I/O error.
    ///
    /// `EINVAL` is in the list because of `KERN_PROCARGS2`: that is how the
    /// sysctl reports a pid that has exited or that this user may not inspect,
    /// where libproc would say `ESRCH`.
    fn errno_error() -> PlatformError {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH | libc::EPERM | libc::ENOENT | libc::EINVAL) => {
                PlatformError::NotFound
            }
            _ => PlatformError::Io(err),
        }
    }
}

/// The honest fallback for Unixes without a reader: the contract's defined
/// degradation (callers fall back to the pane's recorded cwd, and treat "no
/// children" as "ask the shell"), announced once instead of silently.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use amx_core::platform::{PlatformError, ProcessId};

    fn warn_once() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(
                os = std::env::consts::OS,
                "no process-tree reader for this platform; \
                 pane cwd, child inspection and agent identity degrade to \
                 their fallbacks"
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

    /// `Unsupported`, not `NotFound`: the process is fine, it is this platform
    /// that cannot say what it is running — which for tier 3 means every pane
    /// is an unidentified program, forever, and the message should say so once
    /// rather than read as a pane that keeps exiting.
    pub(super) fn argv(_process: ProcessId) -> Result<Vec<OsString>, PlatformError> {
        warn_once();
        Err(PlatformError::Unsupported("reading a process's argv"))
    }

    /// See [`argv`].
    pub(super) fn exe(_process: ProcessId) -> Result<PathBuf, PlatformError> {
        warn_once();
        Err(PlatformError::Unsupported("reading a process's executable"))
    }
}
