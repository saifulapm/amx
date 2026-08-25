//! Wiring amx into the vendor's hooks, and taking it back out.
//!
//! Everything amx knows about a running agent arrives through the vendor's own
//! hooks, which means one line in the vendor's settings file per event. Which
//! file, and which events, is the vendor's entry to say. That file belongs to
//! the person, not to amx, so three rules hold:
//!
//! * **Nothing is touched without a backup.** The pre-merge bytes are copied
//!   to a timestamped file beside the settings before a single byte changes.
//! * **Foreign hooks are never disturbed.** amx adds its own entries and
//!   removes only its own; anything else in the file is carried through by a
//!   plain JSON round trip.
//! * **A file amx cannot read is a file amx does not write.** Settings that do
//!   not parse are reported, never replaced — a person's own editing mistake
//!   must not become amx deleting their configuration.
//!
//! The round trip does cost hand formatting: JSON is re-printed the way serde
//! prints it. That is what the backup is for, and what makes `uninstall`
//! restore the original bytes when nothing else has changed since.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::vendor::claude;

/// The events amx listens to, under the vendor's own names for them.
///
/// The names, the matcher and the settings file are all claude's, and they are
/// all in claude's entry: this file writes what the table says and knows none
/// of the words itself. The mapping each event drives lives with the hook
/// command.
///
/// Flat, because a caller that only wants to say which of them are wired has
/// no use for the rest of the entry.
pub const EVENTS: [&str; WIRED] = named();

/// How many amx wires, which is a question about the vendor's table.
const WIRED: usize = claude::HOOKS.events.len();

/// The vendor's name for every moment amx listens for, in wiring order.
const fn named() -> [&'static str; WIRED] {
    let mut names = [""; WIRED];
    let mut at = 0;
    while at < WIRED {
        names[at] = claude::HOOKS.events[at].event;
        at += 1;
    }
    names
}

/// What was done to the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub path: PathBuf,
    /// Where the previous bytes went, when there were any.
    pub backup: Option<PathBuf>,
    /// Whether the file needed changing at all.
    pub changed: bool,
}

/// The command a hook entry runs.
pub fn hook_command(amx: &Path) -> String {
    let path = amx.to_string_lossy();
    // A path with a space in it is one argument, and the vendor runs this
    // through a shell.
    if path.contains(char::is_whitespace) {
        format!("'{path}' _hook")
    } else {
        format!("{path} _hook")
    }
}

/// Where the vendor keeps its settings, given a home directory.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(claude::HOOKS.settings)
}

/// The same, from the environment.
pub fn settings_file() -> Result<PathBuf> {
    #[allow(deprecated)]
    let home = std::env::home_dir().context("no home directory")?;
    Ok(settings_path(&home))
}

/// The one line a person is asked to agree to before amx edits their settings.
pub fn consent_line(path: &Path, backup: bool) -> String {
    let and_backup = if backup {
        ", keeping a copy of the file as it is now"
    } else {
        ""
    };
    format!(
        "amx will add its {} hooks to {}{and_backup}.",
        EVENTS.len(),
        path.display()
    )
}

/// Which of amx's events this settings document already wires to `command`.
pub fn installed_events(settings: &Value, command: &str) -> Vec<String> {
    EVENTS
        .iter()
        .filter(|event| {
            settings["hooks"][*event]
                .as_array()
                .is_some_and(|groups| groups.iter().any(|group| runs(group, command)))
        })
        .map(|event| event.to_string())
        .collect()
}

/// Add amx's hooks, and answer whether anything needed adding.
///
/// Entries amx recognises as its own are replaced rather than added to, so an
/// amx that has moved on disk leaves one working entry behind and not two.
pub fn merge(settings: &mut Value, command: &str) -> bool {
    let mut changed = remove_hooks(settings, &|found| is_amx_hook(found) && found != command);

    for wiring in claude::HOOKS.events {
        let groups = event_groups(settings, wiring.event);
        if groups.iter().any(|group| runs(group, command)) {
            continue;
        }
        let mut group = json!({ "hooks": [{ "type": "command", "command": command }] });
        if wiring.matched {
            group["matcher"] = json!(claude::HOOKS.matcher);
        }
        groups.push(group);
        changed = true;
    }
    changed
}

/// Take amx's hooks back out, and answer whether anything needed taking out.
pub fn strip(settings: &mut Value) -> bool {
    remove_hooks(settings, &is_amx_hook)
}

/// Whether a hook command is one of amx's.
///
/// The program's own name decides, not the string as a whole: an amx installed
/// somewhere else is still amx, and a person's script that merely mentions amx
/// is not.
fn is_amx_hook(command: &str) -> bool {
    let command = command.trim();
    let (program, rest) = match command.strip_prefix('\'') {
        Some(quoted) => match quoted.split_once('\'') {
            Some(split) => split,
            None => return false,
        },
        None => match command.split_once(char::is_whitespace) {
            Some(split) => split,
            None => return false,
        },
    };
    rest.trim() == "_hook"
        && Path::new(program)
            .file_name()
            .is_some_and(|name| name == "amx")
}

/// Whether one hook group runs `command`.
fn runs(group: &Value, command: &str) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|hook| hook["command"] == command))
}

/// The groups wired to one event, making the shape on the way if it is not
/// there. Anything already at these keys that is not the shape the vendor
/// documents is replaced — there is nothing else it could be.
fn event_groups<'a>(settings: &'a mut Value, event: &str) -> &'a mut Vec<Value> {
    if !settings.is_object() {
        *settings = json!({});
    }
    let root = settings.as_object_mut().expect("an object");
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let by_event = hooks.as_object_mut().expect("an object");
    let groups = by_event.entry(event).or_insert_with(|| json!([]));
    if !groups.is_array() {
        *groups = json!([]);
    }
    groups.as_array_mut().expect("an array")
}

/// Remove every hook entry whose command `doomed` claims, then tidy up what
/// that emptied. A document nothing was removed from is not touched at all.
fn remove_hooks(settings: &mut Value, doomed: &dyn Fn(&str) -> bool) -> bool {
    let Some(by_event) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };

    let mut removed = false;
    for (_event, groups) in by_event.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let before = hooks.len();
            hooks.retain(|hook| !hook["command"].as_str().is_some_and(doomed));
            removed |= hooks.len() != before;
        }
    }
    if !removed {
        return false;
    }

    // What amx left behind: groups with no hooks in them, and events with no
    // groups. An empty `hooks` goes too, so a file that had none before an
    // install has none after an uninstall.
    for (_event, groups) in by_event.iter_mut() {
        if let Some(groups) = groups.as_array_mut() {
            groups.retain(|group| !group["hooks"].as_array().is_some_and(Vec::is_empty));
        }
    }
    by_event.retain(|_event, groups| !groups.as_array().is_some_and(Vec::is_empty));
    if by_event.is_empty()
        && let Some(root) = settings.as_object_mut()
    {
        root.remove("hooks");
    }
    true
}

/// Install the hooks, backing the file up first.
pub fn install(path: &Path, command: &str, now: u64) -> Result<Report> {
    let existing = read(path)?;
    let mut settings = existing.clone().unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        bail!("{} is not settings amx can read", path.display());
    }

    if !merge(&mut settings, command) {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    }

    let backup = back_up(path, now, existing.is_some())?;
    write(path, &settings)?;
    Ok(Report {
        path: path.to_path_buf(),
        backup,
        changed: true,
    })
}

/// Take the hooks out again.
///
/// When the file is exactly what amx left behind, the backup goes back
/// byte for byte and the person's own formatting with it. When it has been
/// edited since, only amx's entries are removed and everything else stays —
/// restoring a backup over a week of somebody's edits would be the worse
/// outcome by far.
pub fn uninstall(path: &Path, now: u64) -> Result<Report> {
    let Some(current) = read(path)? else {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    };

    if let Some(backup) = untouched_since(path, &current)? {
        std::fs::copy(&backup, path).with_context(|| {
            format!("putting {} back over {}", backup.display(), path.display())
        })?;
        return Ok(Report {
            path: path.to_path_buf(),
            backup: Some(backup),
            changed: true,
        });
    }

    let mut settings = current.clone();
    if !strip(&mut settings) {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    }

    let backup = back_up(path, now, true)?;
    write(path, &settings)?;
    Ok(Report {
        path: path.to_path_buf(),
        backup,
        changed: true,
    })
}

/// The backup to put back, when the file is still exactly what amx left
/// behind.
///
/// "Exactly" is asked by re-running the merge over the backup: if that
/// reproduces what is on disk, then nothing but amx has written here since,
/// and the original bytes are safe to restore.
fn untouched_since(path: &Path, current: &Value) -> Result<Option<PathBuf>> {
    let Some(backup) = latest_backup(path)? else {
        return Ok(None);
    };
    let Ok(text) = std::fs::read_to_string(&backup) else {
        return Ok(None);
    };
    let Ok(mut replayed) = serde_json::from_str::<Value>(&text) else {
        return Ok(None);
    };

    for command in amx_commands(current) {
        merge(&mut replayed, &command);
    }
    Ok((replayed == *current).then_some(backup))
}

/// Every distinct amx command wired in this document.
fn amx_commands(settings: &Value) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let Some(by_event) = settings["hooks"].as_object() else {
        return found;
    };
    for groups in by_event.values() {
        for group in groups.as_array().into_iter().flatten() {
            for hook in group["hooks"].as_array().into_iter().flatten() {
                if let Some(command) = hook["command"].as_str()
                    && is_amx_hook(command)
                    && !found.iter().any(|seen| seen == command)
                {
                    found.push(command.to_string());
                }
            }
        }
    }
    found
}

/// Copy the file aside before changing it.
fn back_up(path: &Path, now: u64, exists: bool) -> Result<Option<PathBuf>> {
    if !exists {
        return Ok(None);
    }
    let backup = backup_path(path, now);
    std::fs::copy(path, &backup)
        .with_context(|| format!("copying {} to {}", path.display(), backup.display()))?;
    Ok(Some(backup))
}

/// The most recent backup taken of `path`, if any.
pub fn latest_backup(path: &Path) -> Result<Option<PathBuf>> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(None);
    };
    let prefix = format!("{}.amx-backup-", name.to_string_lossy());

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };

    let mut newest: Option<(u64, PathBuf)> = None;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let file = entry.file_name();
        let Some(taken) = file
            .to_string_lossy()
            .strip_prefix(&prefix)
            .and_then(|stamp| stamp.parse::<u64>().ok())
        else {
            continue;
        };
        if newest.as_ref().is_none_or(|(seen, _)| taken > *seen) {
            newest = Some((taken, entry.path()));
        }
    }
    Ok(newest.map(|(_, path)| path))
}

/// Where a backup of `path` taken at `now` goes.
fn backup_path(path: &Path, now: u64) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}.amx-backup-{now}"))
}

/// Read a settings document, telling "not there" from "not readable".
fn read(path: &Path) -> Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Some(json!({})));
    }
    let parsed = serde_json::from_str(&text)
        .with_context(|| format!("{} is not settings amx can read", path.display()))?;
    Ok(Some(parsed))
}

/// Write a settings document the way the vendor writes one.
fn write(path: &Path, settings: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(settings).context("writing settings")?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const AMX: &str = "/home/dev/.cargo/bin/amx _hook";

    fn hooks(settings: &Value, event: &str) -> Vec<String> {
        settings["hooks"][event]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .flat_map(|group| group["hooks"].as_array().cloned().unwrap_or_default())
                    .filter_map(|hook| hook["command"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Somebody's settings, with their own hook already in them.
    fn a_persons_settings() -> Value {
        json!({
            "model": "opus",
            "permissions": { "allow": ["Bash(git diff:*)"] },
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{ "type": "command", "command": "~/bin/audit.sh" }]
                    }
                ]
            }
        })
    }

    #[test]
    fn install_quotes_a_path_the_shell_would_split() {
        assert_eq!(
            hook_command(Path::new("/home/dev/.cargo/bin/amx")),
            "/home/dev/.cargo/bin/amx _hook"
        );
        let quoted = hook_command(Path::new("/home/dev/my tools/amx"));
        assert_eq!(quoted, "'/home/dev/my tools/amx' _hook");
        assert!(is_amx_hook(&quoted), "and it is still recognisably amx's");
    }

    #[test]
    fn install_asks_before_it_writes_anything() {
        // The file is the vendor's, under the person's home, and the sentence
        // names it in full because that is the thing being agreed to.
        let table = claude::VENDOR.hooks.expect("claude reports through hooks");
        let settings = Path::new("/home/dev").join(table.settings);
        assert_eq!(settings_path(Path::new("/home/dev")), settings);

        let asked = consent_line(&settings, true);
        assert!(asked.contains(&settings.display().to_string()), "{asked}");
        assert!(
            asked.contains("copy"),
            "a person is told about the backup: {asked}"
        );
        assert!(!consent_line(&settings, false).contains("copy"));
    }

    #[test]
    fn install_writes_the_shape_the_vendor_reads() {
        // Every event the vendor's entry names, each with a matcher exactly
        // where that entry asks for one. Nothing here knows what any of them
        // is called: an event this file spelled for itself is one that would
        // go on being wired after the vendor renamed it.
        let table = claude::VENDOR.hooks.expect("claude reports through hooks");
        let mut settings = json!({});
        assert!(merge(&mut settings, AMX));

        for wiring in table.events {
            assert_eq!(hooks(&settings, wiring.event), [AMX], "{}", wiring.event);
            let group = settings["hooks"][wiring.event][0].clone();
            assert_eq!(group["hooks"][0]["type"], "command", "{}", wiring.event);
            if wiring.matched {
                assert_eq!(group["matcher"], table.matcher, "{}", wiring.event);
            } else {
                assert!(group.get("matcher").is_none(), "{}", wiring.event);
            }
        }
        assert_eq!(
            settings["hooks"].as_object().map(serde_json::Map::len),
            Some(table.events.len()),
            "and amx wires nothing the table does not name"
        );
    }

    #[test]
    fn install_twice_is_install_once() {
        let mut settings = a_persons_settings();
        assert!(merge(&mut settings, AMX));
        let after_first = settings.clone();

        assert!(!merge(&mut settings, AMX), "nothing left to do");
        assert_eq!(settings, after_first);
    }

    #[test]
    fn install_leaves_hooks_that_are_not_amxs_alone() {
        let mut settings = a_persons_settings();
        merge(&mut settings, AMX);

        assert_eq!(settings["model"], "opus");
        assert_eq!(settings["permissions"]["allow"][0], "Bash(git diff:*)");
        let pre_tool = hooks(&settings, "PreToolUse");
        assert!(
            pre_tool.contains(&"~/bin/audit.sh".to_string()),
            "{pre_tool:?}"
        );
        assert!(pre_tool.contains(&AMX.to_string()), "{pre_tool:?}");
    }

    #[test]
    fn install_replaces_an_amx_that_has_moved_rather_than_adding_a_second() {
        let mut settings = a_persons_settings();
        merge(&mut settings, "/usr/local/bin/amx _hook");
        merge(&mut settings, AMX);

        for event in EVENTS {
            assert_eq!(
                hooks(&settings, event)
                    .iter()
                    .filter(|c| is_amx_hook(c))
                    .count(),
                1,
                "{event}"
            );
        }
        assert_eq!(hooks(&settings, "Stop"), [AMX]);
        assert!(hooks(&settings, "PreToolUse").contains(&"~/bin/audit.sh".to_string()));
    }

    #[test]
    fn install_knows_its_own_hooks_from_everyone_elses() {
        assert!(is_amx_hook("amx _hook"));
        assert!(is_amx_hook("/usr/local/bin/amx _hook"));
        assert!(is_amx_hook("'/home/dev/my tools/amx' _hook"));
        assert!(is_amx_hook("  amx   _hook  "));

        assert!(!is_amx_hook("amx-helper _hook"));
        assert!(!is_amx_hook("myamx _hook"));
        assert!(!is_amx_hook("amx ls"));
        assert!(!is_amx_hook("~/bin/audit.sh --note 'amx _hook'"));
        assert!(!is_amx_hook(""));
    }

    #[test]
    fn install_reports_which_events_are_wired() {
        let mut settings = json!({});
        assert!(installed_events(&settings, AMX).is_empty());

        merge(&mut settings, AMX);
        let mut wired = installed_events(&settings, AMX);
        wired.sort();
        let mut expected: Vec<String> = EVENTS.iter().map(|e| e.to_string()).collect();
        expected.sort();
        assert_eq!(wired, expected);

        // An amx somewhere else is not this amx.
        assert!(installed_events(&settings, "/opt/amx _hook").is_empty());
    }

    #[test]
    fn install_backs_the_file_up_before_it_changes_anything() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let before = "{\n    \"model\": \"opus\"\n}\n";
        std::fs::write(&path, before).unwrap();

        let report = install(&path, AMX, 1_700_000_000).unwrap();
        assert!(report.changed);
        let backup = report.backup.expect("a backup was taken");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), before);

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["model"], "opus");
        assert_eq!(hooks(&after, "Stop"), [AMX]);
    }

    #[test]
    fn install_creates_settings_that_were_not_there() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/settings.json");

        let report = install(&path, AMX, 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "there was nothing to back up");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(hooks(&written, "Stop"), [AMX]);
    }

    #[test]
    fn install_changes_nothing_when_there_is_nothing_to_change() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        install(&path, AMX, 1).unwrap();
        let after_first = std::fs::read_to_string(&path).unwrap();

        let report = install(&path, AMX, 2).unwrap();
        assert!(!report.changed);
        assert_eq!(report.backup, None, "an unchanged file needs no backup");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after_first);
    }

    #[test]
    fn install_refuses_settings_it_cannot_read_rather_than_replacing_them() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let broken = "{ \"model\": \"opus\",,, }";
        std::fs::write(&path, broken).unwrap();

        let refused = install(&path, AMX, 1).unwrap_err();
        assert!(format!("{refused:#}").contains("settings.json"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "somebody's editing mistake is not amx's to overwrite"
        );
    }

    #[test]
    fn uninstall_puts_the_original_bytes_back() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        // Hand formatting a round trip would not reproduce.
        let before = "{\n    \"model\":   \"opus\"\n}\n";
        std::fs::write(&path, before).unwrap();

        install(&path, AMX, 1).unwrap();
        let report = uninstall(&path, 2).unwrap();

        assert!(report.changed);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "byte for byte, formatting and all"
        );
    }

    #[test]
    fn uninstall_keeps_what_was_written_after_the_install() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{\"model\": \"opus\"}\n").unwrap();
        install(&path, AMX, 1).unwrap();

        // Somebody edits their settings after amx was installed.
        let mut settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        settings["env"] = json!({ "EDITOR": "emacsclient" });
        settings["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks": [{"type": "command", "command": "~/bin/chime"}]}));
        write(&path, &settings).unwrap();

        uninstall(&path, 3).unwrap();

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            after["env"]["EDITOR"], "emacsclient",
            "a week of edits is not amx's to undo"
        );
        assert_eq!(hooks(&after, "Stop"), ["~/bin/chime"]);
        assert!(installed_events(&after, AMX).is_empty());
    }

    #[test]
    fn uninstall_without_a_backup_still_takes_the_hooks_out() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = a_persons_settings();
        merge(&mut settings, AMX);
        write(&path, &settings).unwrap();

        let report = uninstall(&path, 1).unwrap();
        assert!(report.changed);

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(installed_events(&after, AMX).is_empty());
        assert_eq!(hooks(&after, "PreToolUse"), ["~/bin/audit.sh"]);
        assert_eq!(after["model"], "opus");
    }

    #[test]
    fn uninstall_leaves_a_file_amx_never_touched_alone() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let before = "{\n  \"model\": \"opus\"\n}\n";
        std::fs::write(&path, before).unwrap();

        let report = uninstall(&path, 1).unwrap();
        assert!(!report.changed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn uninstall_takes_the_most_recent_backup() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}\n").unwrap();
        assert_eq!(latest_backup(&path).unwrap(), None);

        install(&path, AMX, 100).unwrap();
        install(&path, "/opt/amx _hook", 200).unwrap();

        assert_eq!(
            latest_backup(&path).unwrap(),
            Some(backup_path(&path, 200)),
            "the newest, not whichever the filesystem lists first"
        );
    }
}
