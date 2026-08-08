//! The process table, which is the only witness some claims have.
//!
//! "Exactly one server survives the race" is a claim about processes: a socket
//! has only one peer whether the loser exited or is sitting there unreachable,
//! so nothing short of the process table can tell the two apart.

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::path::Path;
#[cfg(not(target_os = "linux"))]
use std::process::Command;

/// How many `amx server` processes for `session` are alive.
///
/// Test binaries run their tests in parallel threads of one process, so every
/// test's servers are the same executable and only the session separates them —
/// which is why every server this counts is started with an explicit
/// `--session` in its argv.
///
/// Linux reads `/proc` and checks `AMX_SESSION` in the environment as well;
/// macOS has no `/proc` and another process's environment is not readable
/// there, so it matches the argv `ps` reports.
#[cfg(target_os = "linux")]
pub fn server_processes(exe: &Path, session: &str) -> usize {
    let marker = OsString::from(format!("AMX_SESSION={session}"));
    let entries = std::fs::read_dir("/proc").expect("/proc is readable");
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let argv = nul_separated(&entry.path().join("cmdline"));
            let is_server = argv.len() >= 2 && argv[0] == exe.as_os_str() && argv[1] == "server";
            is_server && nul_separated(&entry.path().join("environ")).contains(&marker)
        })
        .count()
}

/// See the Linux twin above; `ps -o command=` is the process table here.
#[cfg(not(target_os = "linux"))]
pub fn server_processes(exe: &Path, session: &str) -> usize {
    let out = Command::new("ps")
        .args(["-axww", "-o", "command="])
        .output()
        .expect("run ps");
    assert!(out.status.success(), "ps failed: {out:?}");
    let server = format!("{} server --session {session}", exe.display());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.trim_end() == server)
        .count()
}

/// A `/proc` file of NUL-separated strings.
#[cfg(target_os = "linux")]
fn nul_separated(path: &Path) -> Vec<OsString> {
    use std::os::unix::ffi::OsStringExt as _;

    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| OsString::from_vec(part.to_vec()))
        .collect()
}
