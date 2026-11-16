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
        .filter(|it| !started_by_amx(it.pid))
        .collect()
}

/// Whether amx started this process: its environment names an agent. Such a
/// claude has its record, which names no session until its first hook.
pub fn started_by_amx(pid: u32) -> bool {
    environ_of(pid, crate::hook::ID_ENV).is_some()
}

/// A variable from a process's environment, `None` when unset, empty or
/// unreadable.
pub fn environ_of(pid: u32, name: &str) -> Option<String> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{name}=");
    environ
        .split(|byte| *byte == 0)
        .filter_map(|pair| std::str::from_utf8(pair).ok())
        .find_map(|pair| pair.strip_prefix(&prefix).map(str::to_string))
        .filter(|value| !value.is_empty())
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

/// The sessions file of a session whose process still runs. A file left by a
/// killed claude, or naming a pid now used by another process, says nothing.
/// Its status is `idle`, `busy` or `shell`; see [`phase_of`].
pub fn live_file(
    dirs: &[PathBuf],
    session: &str,
    running: impl Fn(u32) -> Option<String>,
) -> Option<Loose> {
    files(dirs)
        .into_iter()
        .find(|it| it.session == session && it.runs(&running))
}

/// The phase a sessions file's status means: `idle` at the prompt, `busy`
/// in a turn. `shell` and anything newer say nothing amx can rely on.
pub fn phase_of(status: &str) -> Option<crate::store::Phase> {
    match status {
        "idle" => Some(crate::store::Phase::Idle),
        "busy" => Some(crate::store::Phase::Working),
        _ => None,
    }
}

impl Loose {
    /// Whether the process the file names still runs: its pid is alive with
    /// the start time the file gives.
    pub fn runs(&self, running: impl Fn(u32) -> Option<String>) -> bool {
        running(self.pid).as_deref() == Some(self.proc_start.as_str())
    }
}

/// The most of a sessions file amx reads. claude's are a few hundred bytes.
const LARGEST: u64 = 64 * 1024;

/// One sessions file, or `None` when a field amx needs is missing.
///
/// Only a regular file no larger than [`LARGEST`] is read: a link or a device
/// put in its place would otherwise be read without end.
fn read(path: &Path, config_dir: &Path) -> Option<Loose> {
    use std::io::Read;
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file() || opened.len() > LARGEST {
        return None;
    }
    let mut text = String::new();
    file.take(LARGEST).read_to_string(&mut text).ok()?;
    let file: Value = serde_json::from_str(&text).ok()?;
    // Background and spare claudes (`bg`) write files too; only one a person
    // runs counts. An older claude wrote no kind.
    if file["kind"]
        .as_str()
        .is_some_and(|kind| kind != "interactive")
    {
        return None;
    }
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
        // A file that does not say is not taken as idle.
        status: file["status"].as_str().unwrap_or("unknown").to_string(),
        proc_start,
        started: file["startedAt"].as_u64().map(|ms| ms / 1000),
        config_dir: config_dir.to_path_buf(),
    })
}

/// [`proc_start`] for a pid that runs claude: its command line names claude,
/// as the program or as the script an interpreter runs. A sessions file can
/// name any pid, and only a claude may be listed or signalled.
pub fn claude_start(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let names_claude = cmdline
        .split(|byte| *byte == 0)
        .take(2)
        .any(|word| String::from_utf8_lossy(word).contains("claude"));
    names_claude.then(|| proc_start(pid)).flatten()
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
    fn a_sessions_status_is_read_only_while_its_process_runs() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = [dir.path().to_path_buf()];
        a_file(dir.path(), 41, idle(41, "s-1"));
        let status = |session, running: fn(u32) -> Option<String>| {
            live_file(&dirs, session, running).map(|it| it.status)
        };
        assert_eq!(status("s-1", all_running).as_deref(), Some("idle"));
        assert_eq!(status("s-2", all_running), None, "another session");
        // The claude exited and its pid went to another process, which started
        // later; or nothing runs on it at all.
        assert_eq!(status("s-1", |_| Some("900".to_string())), None);
        assert_eq!(status("s-1", |_| None), None);

        use crate::store::Phase;
        assert_eq!(phase_of("idle"), Some(Phase::Idle));
        assert_eq!(phase_of("busy"), Some(Phase::Working));
        assert_eq!(phase_of("shell"), None);
        assert_eq!(phase_of("some_future_status"), None);
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
    fn a_background_claude_and_a_file_with_no_status_are_not_taken_as_idle() {
        let dir = tempfile::tempdir().unwrap();
        let mut bg = idle(41, "s-1");
        bg["kind"] = "bg".into();
        a_file(dir.path(), 41, bg);
        let mut older = idle(42, "s-2");
        older.as_object_mut().unwrap().remove("status");
        a_file(dir.path(), 42, older);
        let found = find(&[dir.path().to_path_buf()], &BTreeSet::new(), all_running);
        let read: Vec<(u32, &str)> = found
            .iter()
            .map(|it| (it.pid, it.status.as_str()))
            .collect();
        assert_eq!(read, [(42, "unknown")]);
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
    fn a_link_a_device_or_an_oversized_file_in_its_place_is_skipped_alone() {
        let dir = tempfile::tempdir().unwrap();
        a_file(dir.path(), 41, idle(41, "s-1"));
        let sessions = dir.path().join("sessions");
        std::os::unix::fs::symlink("/dev/zero", sessions.join("42.json")).unwrap();
        let elsewhere = dir.path().join("elsewhere.json");
        std::fs::write(&elsewhere, idle(43, "s-3").to_string()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, sessions.join("43.json")).unwrap();
        let mut huge = idle(44, "s-4");
        huge["name"] = "x".repeat(LARGEST as usize).into();
        std::fs::write(sessions.join("44.json"), huge.to_string()).unwrap();

        let found = find(&[dir.path().to_path_buf()], &BTreeSet::new(), all_running);
        let pids: Vec<u32> = found.iter().map(|loose| loose.pid).collect();
        assert_eq!(pids, [41]);
    }

    #[test]
    fn only_a_process_whose_command_line_names_claude_counts_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("claude");
        std::os::unix::fs::symlink("/bin/sleep", &named).unwrap();
        let mut claude = std::process::Command::new(&named)
            .arg("30")
            .spawn()
            .unwrap();
        let mut other = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();

        let (theirs, started) = (claude_start(claude.id()), proc_start(claude.id()));
        let plain = claude_start(other.id());
        let _ = (claude.kill(), other.kill(), claude.wait(), other.wait());
        assert!(started.is_some());
        assert_eq!(theirs, started);
        assert_eq!(plain, None);
    }

    #[test]
    fn proc_start_reads_this_process_and_nothing_for_a_pid_that_is_gone() {
        let mine = proc_start(std::process::id()).expect("this process runs");
        assert!(mine.parse::<u64>().is_ok(), "{mine}");
        assert_eq!(proc_start(u32::MAX), None);
    }
}
