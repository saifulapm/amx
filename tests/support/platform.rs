//! Platform probes for the rig: the process table, memory, ingestion.
//!
//! Tier 1 is Linux and macOS (03 §design principles), and a probe that
//! quietly answers wrong on one of them converts a porting gap into a 10s
//! timeout — or worse, a green test. So every probe here either answers from
//! the platform's real source, degrades *loudly* (an `Option` the caller must
//! acknowledge), or panics naming the gap.

#[cfg(target_os = "macos")]
use std::process::Command;

/// A process's resident set size in bytes.
///
/// Linux reads `statm` (pages, scaled by the real page size — arm64 kernels
/// run 16K and 64K pages, so 4096 must not be assumed); macOS asks `ps`,
/// whose `rss=` column is kilobytes.
#[must_use]
pub fn rss_bytes(pid: u32) -> u64 {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm"))
            .expect("the process has a /proc entry");
        let resident: u64 = statm
            .split_whitespace()
            .nth(1)
            .expect("statm has a resident field")
            .parse()
            .expect("resident is a number");
        resident * rustix::param::page_size() as u64
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        let kilobytes: u64 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .expect("ps reports a resident size");
        kilobytes * 1024
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        panic!("unsupported platform probe: rss_bytes has no reader for this OS");
    }
}

/// Bytes the process has read from anywhere, or `None` where the platform
/// cannot say.
///
/// Linux answers from `/proc/<pid>/io`, which exists only on kernels built
/// with `CONFIG_TASK_IO_ACCOUNTING`; macOS has no equivalent the rig can
/// reach without entitlements. `None` is a loud answer: the caller must
/// decide what its assertion means without the number, visibly.
#[must_use]
pub fn read_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{pid}/io");
        // Availability, not permission: a missing file is a kernel without io
        // accounting; anything else is still a hard expectation.
        if !std::path::Path::new(&path).exists() {
            return None;
        }
        let io = std::fs::read_to_string(&path).expect("the io entry is readable");
        Some(
            io.lines()
                .find_map(|line| line.strip_prefix("rchar: "))
                .expect("io reports rchar")
                .trim()
                .parse()
                .expect("rchar is a number"),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// How many live processes have `marker` in their argv.
///
/// A claim like "the pane's process survived the client dying" is about
/// processes, and only the process table can attest to it: `/proc` on Linux,
/// `pgrep -f` on macOS. An unreadable table is a panic, not a zero — a zero
/// here silently satisfies every "wait until it is gone" and times out every
/// "wait until it exists".
#[must_use]
pub fn processes_with_arg(marker: &str) -> usize {
    #[cfg(target_os = "linux")]
    {
        let entries =
            std::fs::read_dir("/proc").expect("unsupported platform probe: /proc is not readable");
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                nul_separated(&entry.path().join("cmdline"))
                    .iter()
                    .any(|arg| arg.to_string_lossy().contains(marker))
            })
            .count()
    }
    #[cfg(target_os = "macos")]
    {
        // `pgrep` exits 1 for "no match", which is an answer, not a failure.
        // `-l` carries the matched command lines so a failed wait can say who
        // matched instead of just how many.
        let out = Command::new("pgrep")
            .args(["-fl", marker])
            .output()
            .expect("run pgrep");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let matched: Vec<&str> = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        eprintln!(
            "processes_with_arg({marker:?}) -> {} [{}]",
            matched.len(),
            matched.join(" | ")
        );
        matched.len()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = marker;
        panic!("unsupported platform probe: processes_with_arg has no reader for this OS");
    }
}

/// The pids of live processes with `marker` in their argv.
///
/// The pid-returning sibling of [`processes_with_arg`], for a diagnosis that
/// must *do* something to the matched processes — deliver a signal by hand —
/// rather than count them.
#[must_use]
pub fn pids_with_arg(marker: &str) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        let entries =
            std::fs::read_dir("/proc").expect("unsupported platform probe: /proc is not readable");
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_string_lossy().parse().ok()?;
                nul_separated(&entry.path().join("cmdline"))
                    .iter()
                    .any(|arg| arg.to_string_lossy().contains(marker))
                    .then_some(pid)
            })
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("pgrep")
            .args(["-f", marker])
            .output()
            .expect("run pgrep");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = marker;
        panic!("unsupported platform probe: pids_with_arg has no reader for this OS");
    }
}

/// The slave index of every pty master `pid` holds open, or `None` where the
/// platform will not say.
///
/// The index, not the descriptor number: two processes holding the same
/// terminal give it different descriptor numbers and the same index, which is
/// what makes "this server is holding *my* terminal" answerable at all. Linux
/// reads `tty-index` out of `/proc/<pid>/fdinfo`; macOS has no unprivileged
/// reader without `lsof`(8), and `None` says so rather than return an empty
/// list a caller would read as "it holds none".
#[must_use]
pub fn pty_masters(pid: u32) -> Option<Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        let entries = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
        let mut found = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            if std::fs::read_link(entry.path()).ok()? != std::path::Path::new("/dev/ptmx") {
                continue;
            }
            let name = entry.file_name();
            let Ok(info) =
                std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{}", name.to_string_lossy()))
            else {
                continue;
            };
            found.extend(
                info.lines()
                    .filter_map(|line| line.strip_prefix("tty-index:"))
                    .filter_map(|value| value.trim().parse::<u32>().ok()),
            );
        }
        found.sort_unstable();
        Some(found)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// What every thread of `pid` is doing, as one human-readable block.
///
/// The diagnosis a shutdown that never finished owes: "the server ignored
/// SIGTERM" names the symptom, and the JoinSet drain the shutdown wedge
/// (R-M1-2) lives in is only distinguishable from a busy server by where its
/// threads are parked. Linux reads each task's name, run state and kernel wait
/// channel out of `/proc`; macOS has no unprivileged equivalent the rig can
/// read without `sample`(1) attaching to the process, so it says so rather
/// than return an empty string a reader would mistake for "no threads".
#[must_use]
pub fn thread_states(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return format!("pid {pid} has no /proc entry — it is already gone");
        };
        let mut lines = Vec::new();
        for task in tasks.filter_map(Result::ok) {
            let dir = task.path();
            let read = |name: &str| {
                std::fs::read_to_string(dir.join(name))
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            let comm = read("comm");
            let wchan = read("wchan");
            // `stat`'s third field is the run state, and the second is the
            // command in parentheses — which may itself hold spaces, so the
            // split starts after the closing one.
            let stat = read("stat");
            let state = stat
                .rsplit_once(") ")
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .unwrap_or("?")
                .to_owned();
            lines.push(format!(
                "  {} [{}] state={state} wchan={}",
                task.file_name().to_string_lossy(),
                if comm.is_empty() { "?" } else { &comm },
                if wchan.is_empty() { "0" } else { &wchan },
            ));
        }
        lines.sort();
        format!("threads of pid {pid}:\n{}", lines.join("\n"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!(
            "no unprivileged per-thread state reader on this platform; \
             run `sample {pid}` by hand against a server caught in this state"
        )
    }
}

/// A `/proc` file of NUL-separated strings.
#[cfg(target_os = "linux")]
fn nul_separated(path: &std::path::Path) -> Vec<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt as _;

    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| std::ffi::OsString::from_vec(part.to_vec()))
        .collect()
}
