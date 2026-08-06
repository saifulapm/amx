//! The Unix side of the [`ProcessTree`] seam, read out of `/proc`.
//!
//! Every answer here is a snapshot that may already be stale — the process can
//! exit between the read and the use — which is why the contract gives callers
//! a defined fallback rather than promising accuracy.

use std::path::{Path, PathBuf};

use amx_core::platform::{PlatformError, ProcessId, ProcessTree};
use rustix::io::Errno;
use rustix::process::Pid;

/// Where the process table is mounted.
const PROC: &str = "/proc";

/// Reading the process tree on Unix.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct UnixProcessTree;

impl ProcessTree for UnixProcessTree {
    fn cwd(&self, process: ProcessId) -> Result<PathBuf, PlatformError> {
        let link = Path::new(PROC).join(process.0.to_string()).join("cwd");
        std::fs::read_link(link).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                PlatformError::NotFound
            }
            _ => PlatformError::Io(err),
        })
    }

    fn children(&self, process: ProcessId) -> Result<Vec<ProcessId>, PlatformError> {
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

/// The parent pid recorded in `/proc/<pid>/stat`, or `None` if it cannot be
/// read.
///
/// The comm field is an arbitrary string in parentheses and may contain spaces
/// and parentheses of its own, so the fields are counted from the *last* `)`
/// rather than split from the front.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(Path::new(PROC).join(pid.to_string()).join("stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // The fields after the comm are: state, ppid, ...
    after_comm.split_whitespace().nth(1)?.parse().ok()
}
