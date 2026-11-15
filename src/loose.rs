//! Loose claudes: claude processes on this machine that no record owns.
//!
//! claude keeps a sessions file, `<config dir>/sessions/<pid>.json`, for each
//! live process and removes it on exit. It is undocumented; the fields read
//! here are pid, sessionId, cwd, procStart, status and startedAt. A file is
//! loose when its pid still runs with the same procStart and no record holds
//! its session. A killed claude leaves its file behind, and the pid can be
//! reused, which procStart tells apart.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::Config;

/// The variable naming a claude account directory.
pub const CONFIG_ENV: &str = "CLAUDE_CONFIG_DIR";

/// One claude process outside amx, as its sessions file describes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Loose {
    pub pid: u32,
    pub session: String,
    /// The directory the claude runs in.
    pub cwd: PathBuf,
    /// `idle`, `busy` or `shell`, as claude wrote it.
    pub status: String,
    /// The process start time in clock ticks since boot, as /proc gives it.
    pub proc_start: String,
    /// When the process started, in seconds since the epoch, if the file says.
    pub started: Option<u64>,
    /// The account directory the file was found under.
    pub config_dir: PathBuf,
}

/// The account directories whose sessions files are read: `$CLAUDE_CONFIG_DIR`,
/// `~/.claude`, then any `CLAUDE_CONFIG_DIR` in `[claude.env]`, each once.
pub fn config_dirs(env: &BTreeMap<String, String>, home: &Path, config: &Config) -> Vec<PathBuf> {
    let configured = config.harness("claude").env;
    let named = [env.get(CONFIG_ENV), None, configured.get(CONFIG_ENV)];
    let mut dirs: Vec<PathBuf> = Vec::new();
    for (place, dir) in named.into_iter().enumerate() {
        let dir = match (place, dir) {
            (1, _) => home.join(".claude"),
            (_, Some(dir)) if !dir.is_empty() => PathBuf::from(dir),
            _ => continue,
        };
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Every loose claude under `dirs`, one per pid, in pid order.
///
/// `held` holds the sessions records name. `running` answers a pid's
/// procStart, or `None` for a pid that is gone. A file that does not read is
/// skipped on its own.
pub fn find(
    dirs: &[PathBuf],
    held: &BTreeSet<String>,
    running: impl Fn(u32) -> Option<String>,
) -> Vec<Loose> {
    files(dirs)
        .into_iter()
        .filter(|it| !held.contains(&it.session) && it.runs(&running))
        .collect()
}

/// Every sessions file under `dirs` that reads, live or not, one per pid: the
/// first account dir's wins.
pub fn files(dirs: &[PathBuf]) -> Vec<Loose> {
    let mut found: BTreeMap<u32, Loose> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("sessions")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if let Some(loose) = read(&path, dir) {
                found.entry(loose.pid).or_insert(loose);
            }
        }
    }
    found.into_values().collect()
}

/// The pid of a running claude on `session`, held by a record or not.
pub fn live_on(
    dirs: &[PathBuf],
    session: &str,
    running: impl Fn(u32) -> Option<String>,
) -> Option<u32> {
    files(dirs)
        .into_iter()
        .find(|it| it.session == session && it.runs(&running))
        .map(|it| it.pid)
}

impl Loose {
    /// Whether the process the file names still runs: its pid is alive with
    /// the start time the file gives.
    pub fn runs(&self, running: impl Fn(u32) -> Option<String>) -> bool {
        running(self.pid).as_deref() == Some(self.proc_start.as_str())
    }
}

/// One sessions file, or `None` when a field amx needs is missing.
fn read(path: &Path, config_dir: &Path) -> Option<Loose> {
    let text = std::fs::read_to_string(path).ok()?;
    let file: Value = serde_json::from_str(&text).ok()?;
    let pid = u32::try_from(file["pid"].as_u64()?).ok()?;
    let proc_start = match &file["procStart"] {
        Value::String(start) => start.clone(),
        Value::Number(start) => start.to_string(),
        _ => return None,
    };
    Some(Loose {
        pid,
        session: file["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())?
            .to_string(),
        cwd: PathBuf::from(file["cwd"].as_str()?),
        status: file["status"].as_str().unwrap_or("idle").to_string(),
        proc_start,
        started: file["startedAt"].as_u64().map(|ms| ms / 1000),
        config_dir: config_dir.to_path_buf(),
    })
}

/// The start time of a running pid in clock ticks since boot, field 22 of
/// `/proc/<pid>/stat`, or `None` when the pid is gone.
pub fn proc_start(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces or `)`, so the
    // fields are counted from the last `)`. Field 3 follows it.
    let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace();
    // A zombie has ended; only its parent has yet to reap it.
    if matches!(fields.next(), Some("Z" | "X")) {
        return None;
    }
    fields.nth(18).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a_file(dir: &Path, pid: u32, fields: Value) {
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join(format!("{pid}.json")), fields.to_string()).unwrap();
    }

    fn idle(pid: u32, session: &str) -> Value {
        json!({
            "pid": pid, "sessionId": session, "cwd": "/work/app",
            "procStart": "777", "status": "idle", "startedAt": 1_700_000_000_000u64,
        })
    }

    /// Every pid runs, started at 777.
    fn all_running(_: u32) -> Option<String> {
        Some("777".to_string())
    }

    #[test]
    fn a_live_claude_no_record_holds_is_loose() {
        let dir = tempfile::tempdir().unwrap();
        a_file(dir.path(), 41, idle(41, "s-1"));
        let found = find(&[dir.path().to_path_buf()], &BTreeSet::new(), all_running);
        assert_eq!(
            found,
            vec![Loose {
                pid: 41,
                session: "s-1".into(),
                cwd: "/work/app".into(),
                status: "idle".into(),
                proc_start: "777".into(),
                started: Some(1_700_000_000),
                config_dir: dir.path().to_path_buf(),
            }]
        );
    }

    #[test]
    fn a_gone_pid_a_reused_pid_and_a_held_session_are_not_loose() {
        let dir = tempfile::tempdir().unwrap();
        a_file(dir.path(), 41, idle(41, "s-1"));
        a_file(dir.path(), 42, idle(42, "s-2"));
        a_file(dir.path(), 43, idle(43, "s-3"));
        let held = BTreeSet::from(["s-3".to_string()]);
        let running = |pid: u32| match pid {
            41 => None,
            42 => Some("999".to_string()),
            _ => Some("777".to_string()),
        };
        assert_eq!(find(&[dir.path().to_path_buf()], &held, running), vec![]);
    }

    #[test]
    fn the_same_pid_under_two_account_dirs_is_one_claude() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        a_file(first.path(), 41, idle(41, "s-1"));
        a_file(second.path(), 41, idle(41, "s-1"));
        a_file(second.path(), 52, idle(52, "s-2"));
        let dirs = [first.path().to_path_buf(), second.path().to_path_buf()];
        let found = find(&dirs, &BTreeSet::new(), all_running);
        let pids: Vec<u32> = found.iter().map(|loose| loose.pid).collect();
        assert_eq!(pids, [41, 52]);
        assert_eq!(found[0].config_dir, first.path());
    }

    #[test]
    fn a_sessions_file_that_does_not_read_costs_that_file_and_not_the_listing() {
        let dir = tempfile::tempdir().unwrap();
        a_file(dir.path(), 41, idle(41, "s-1"));
        a_file(
            dir.path(),
            42,
            json!({ "pid": "forty-two", "sessionId": "s-2" }),
        );
        std::fs::write(dir.path().join("sessions/43.json"), "{ not json").unwrap();
        a_file(dir.path(), 44, idle(44, "s-4"));
        let found = find(&[dir.path().to_path_buf()], &BTreeSet::new(), all_running);
        let pids: Vec<u32> = found.iter().map(|loose| loose.pid).collect();
        assert_eq!(pids, [41, 44]);
    }

    #[test]
    fn the_account_dirs_are_the_variable_home_then_the_config_each_once() {
        let home = Path::new("/home/me");
        let mut env = BTreeMap::new();
        let none = Config::default();
        assert_eq!(config_dirs(&env, home, &none), [home.join(".claude")]);

        env.insert(CONFIG_ENV.to_string(), "/acc/work".to_string());
        let (config, _) =
            crate::config::parse("[claude.env]\nCLAUDE_CONFIG_DIR = \"/acc/home\"\n").unwrap();
        assert_eq!(
            config_dirs(&env, home, &config),
            [
                PathBuf::from("/acc/work"),
                home.join(".claude"),
                PathBuf::from("/acc/home")
            ]
        );

        env.insert(CONFIG_ENV.to_string(), "/home/me/.claude".to_string());
        assert_eq!(config_dirs(&env, home, &none), [home.join(".claude")]);
    }

    #[test]
    fn proc_start_reads_this_process_and_nothing_for_a_pid_that_is_gone() {
        let mine = proc_start(std::process::id()).expect("this process runs");
        assert!(mine.parse::<u64>().is_ok(), "{mine}");
        assert_eq!(proc_start(u32::MAX), None);
    }
}
