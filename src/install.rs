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
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::vendor::{Hooks, Wire};

/// The events amx listens to, under the vendor's own names for them, in
/// wiring order.
///
/// The names come off the vendor's entry: this file writes what the table says
/// and knows none of the words itself. The mapping each event drives lives
/// with the hook command.
pub fn events(hooks: &Hooks) -> impl Iterator<Item = &'static str> {
    hooks.events.iter().map(|wiring| wiring.event)
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

/// The person's home directory, which every wire is written under.
pub fn home() -> Result<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().context("no home directory")
}

/// Where this vendor's wiring goes, given a home directory: its settings file,
/// or the file amx writes where it loads extensions from.
pub fn wire_path(hooks: &Hooks, home: &Path) -> PathBuf {
    home.join(hooks.wire.path())
}

/// The one line a person is asked to agree to before amx writes under their
/// home: what it will write, where, and whether a copy is kept.
pub fn consent_line(hooks: &Hooks, path: &Path, backup: bool) -> String {
    let and_backup = if backup {
        ", keeping a copy of the file as it is now"
    } else {
        ""
    };
    match hooks.wire {
        Wire::Settings(_) => format!(
            "amx will add its {} hooks to {}{and_backup}.",
            hooks.events.len(),
            path.display()
        ),
        Wire::File { .. } => format!(
            "amx will write its extension to {}{and_backup}.",
            path.display()
        ),
    }
}

/// What is wired on this machine for a vendor, read without changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wired {
    /// The vendor reports nothing, so there is nothing to be wired.
    Nothing,
    /// A settings wire: which of amx's events the file wires to the hook
    /// command, and why the file could not be read if it could not.
    Settings {
        events: Vec<String>,
        error: Option<String>,
    },
    /// A file wire: whether a file stands at the path, and whether it is the
    /// one this amx ships.
    File { present: bool, current: bool },
}

/// Read what is wired for `hooks` under `home`.
pub fn wired(hooks: Option<&Hooks>, home: &Path, command: &str) -> Wired {
    let Some(hooks) = hooks else {
        return Wired::Nothing;
    };
    let path = wire_path(hooks, home);
    match hooks.wire {
        Wire::Settings(_) => match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => Wired::Settings {
                    events: installed_events(hooks, &value, command),
                    error: None,
                },
                Err(e) => Wired::Settings {
                    events: Vec::new(),
                    error: Some(e.to_string()),
                },
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Wired::Settings {
                events: Vec::new(),
                error: None,
            },
            Err(e) => Wired::Settings {
                events: Vec::new(),
                error: Some(e.to_string()),
            },
        },
        Wire::File { body, .. } => match std::fs::read_to_string(&path) {
            Ok(text) => Wired::File {
                present: true,
                current: text == body,
            },
            Err(_) => Wired::File {
                present: false,
                current: false,
            },
        },
    }
}

/// Wire `hooks` under `home`, whichever shape the wire is.
pub fn install_hooks(hooks: &Hooks, home: &Path, command: &str, now: u64) -> Result<Report> {
    let path = wire_path(hooks, home);
    match hooks.wire {
        Wire::Settings(_) => install(hooks, &path, command, now),
        Wire::File { body, .. } => install_file(&path, body, now),
    }
}

/// Take `hooks` back out from under `home`, whichever shape the wire is.
pub fn uninstall_hooks(hooks: &Hooks, home: &Path, now: u64) -> Result<Report> {
    let path = wire_path(hooks, home);
    match hooks.wire {
        Wire::Settings(_) => uninstall(hooks, &path, now),
        Wire::File { body, .. } => uninstall_file(&path, body, now),
    }
}

/// Which of amx's events this settings document already wires to `command`.
pub fn installed_events(hooks: &Hooks, settings: &Value, command: &str) -> Vec<String> {
    events(hooks)
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
pub fn merge(hooks: &Hooks, settings: &mut Value, command: &str) -> bool {
    let mut changed = remove_hooks(settings, &|found| is_amx_hook(found) && found != command);

    for wiring in hooks.events {
        let groups = event_groups(settings, wiring.event);
        if groups.iter().any(|group| runs(group, command)) {
            continue;
        }
        let mut group = json!({ "hooks": [{ "type": "command", "command": command }] });
        if wiring.matched {
            group["matcher"] = json!(hooks.matcher);
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

/// Install the hooks into a settings file, backing the file up first.
pub fn install(hooks: &Hooks, path: &Path, command: &str, now: u64) -> Result<Report> {
    let existing = read(path)?;
    let mut settings = existing.clone().unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        bail!("{} is not settings amx can read", path.display());
    }

    if !merge(hooks, &mut settings, command) {
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

/// Write amx's own file where the vendor loads it from.
///
/// A file already holding these bytes is left as it is. A file of amx's own
/// from another version is written over — it is amx's to replace — and a file
/// that is somebody else's is copied aside first, the way a settings file is,
/// because the name is amx's to take and the bytes are not amx's to lose.
pub fn install_file(path: &Path, body: &str, now: u64) -> Result<Report> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if existing.as_deref() == Some(body) {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    }
    let foreign = existing
        .as_deref()
        .is_some_and(|text| !is_amx_file(text, body));
    let backup = back_up(path, now, foreign)?;
    write_bytes(path, body.as_bytes())?;
    Ok(Report {
        path: path.to_path_buf(),
        backup,
        changed: true,
    })
}

/// Take amx's own file away again, and put back whatever it was written over.
///
/// A file that is not amx's — one without amx's first line — is not amx's to
/// remove, and stays.
pub fn uninstall_file(path: &Path, body: &str, now: u64) -> Result<Report> {
    let _ = now;
    let current = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Report {
                path: path.to_path_buf(),
                backup: None,
                changed: false,
            });
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if !is_amx_file(&current, body) {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    }
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    let backup = latest_backup(path)?;
    if let Some(backup) = &backup {
        std::fs::copy(backup, path).with_context(|| {
            format!("putting {} back over {}", backup.display(), path.display())
        })?;
    }
    Ok(Report {
        path: path.to_path_buf(),
        backup,
        changed: true,
    })
}

/// Whether a file at a wire's path is amx's: it opens with the line amx's
/// own file opens with, whichever version wrote it.
fn is_amx_file(text: &str, body: &str) -> bool {
    text.lines()
        .next()
        .is_some_and(|first| Some(first) == body.lines().next())
}

/// Take the hooks out again.
///
/// When the file is exactly what amx left behind, the backup goes back
/// byte for byte and the person's own formatting with it. When it has been
/// edited since, only amx's entries are removed and everything else stays —
/// restoring a backup over a week of somebody's edits would be the worse
/// outcome by far.
pub fn uninstall(hooks: &Hooks, path: &Path, now: u64) -> Result<Report> {
    let Some(current) = read(path)? else {
        return Ok(Report {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    };

    if let Some(backup) = untouched_since(hooks, path, &current)? {
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
fn untouched_since(hooks: &Hooks, path: &Path, current: &Value) -> Result<Option<PathBuf>> {
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
        merge(hooks, &mut replayed, &command);
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
///
/// Opened with `create_new`, so a name a planted symlink already stands at is
/// refused rather than copied through: the backup path is predictable, and a
/// symlink waiting there before amx ever runs must not decide where the
/// person's settings end up.
fn back_up(path: &Path, now: u64, exists: bool) -> Result<Option<PathBuf>> {
    if !exists {
        return Ok(None);
    }
    let backup = backup_path(path, now);
    let mut source =
        std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut dest = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .with_context(|| format!("copying {} to {}", path.display(), backup.display()))?;
    std::io::copy(&mut source, &mut dest)
        .with_context(|| format!("copying {} to {}", path.display(), backup.display()))?;
    Ok(Some(backup))
}

/// The most recent backup taken of `path`, if any.
///
/// Ordered by each entry's own mtime, and only among regular files: the
/// number in a backup's name is easy to plant, but the filesystem's own clock
/// is not, and a symlink or directory under a backup's name is not a backup
/// amx wrote, whatever number follows it. `trust::back_up` asks this same
/// question to decide whether it still needs to take a copy, so a name that
/// could win the answer without amx ever having written it would leave that
/// copy untaken.
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

    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let is_backup = entry
            .file_name()
            .to_string_lossy()
            .strip_prefix(&prefix)
            .is_some_and(|stamp| stamp.parse::<u64>().is_ok());
        if !is_backup {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(seen, _)| modified > *seen) {
            newest = Some((modified, entry.path()));
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
///
/// Staged beside the target and renamed over it, rather than written to the
/// path directly: a rename replaces whatever is at that name without ever
/// opening it, so a symlink standing at the path is replaced and never
/// written through.
fn write(path: &Path, settings: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(settings).context("writing settings")?;
    text.push('\n');
    write_bytes(path, text.as_bytes())
}

/// Write a file the same way: staged beside its path and renamed over it.
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let part = path.with_file_name(format!("{name}.amx-{}", std::process::id()));
    match staged(&part, bytes).and_then(|()| {
        std::fs::rename(&part, path).with_context(|| format!("putting {} in place", path.display()))
    }) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// The half of the write that happens beside the file rather than to it.
fn staged(part: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)
        .with_context(|| format!("creating {}", part.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", part.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::claude;
    use tempfile::TempDir;

    const AMX: &str = "/home/dev/.cargo/bin/amx _hook";
    const CLAUDE: &Hooks = &claude::HOOKS;

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

    /// A file wire of the tests' own, shaped like pi's.
    const FILE: Hooks = Hooks {
        wire: Wire::File {
            path: ".vendor/extensions/amx.ts",
            body: "// installed by amx\nexport default () => {};\n",
        },
        ..claude::HOOKS
    };

    fn file_body() -> &'static str {
        let Wire::File { body, .. } = FILE.wire else {
            unreachable!()
        };
        body
    }

    #[test]
    fn install_writes_a_file_wire_whole_and_reads_it_back() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE, home.path());
        assert_eq!(
            wired(Some(&FILE), home.path(), AMX),
            Wired::File {
                present: false,
                current: false
            }
        );

        let report = install_hooks(&FILE, home.path(), AMX, 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "nothing was there to keep");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());
        assert_eq!(
            wired(Some(&FILE), home.path(), AMX),
            Wired::File {
                present: true,
                current: true
            }
        );

        // Installed twice is installed once.
        let again = install_hooks(&FILE, home.path(), AMX, 2).unwrap();
        assert!(!again.changed);
        assert_eq!(again.backup, None);
    }

    #[test]
    fn install_replaces_an_older_amx_file_without_keeping_it() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "// installed by amx\n// an older one\n").unwrap();
        assert_eq!(
            wired(Some(&FILE), home.path(), AMX),
            Wired::File {
                present: true,
                current: false
            }
        );

        let report = install_hooks(&FILE, home.path(), AMX, 1).unwrap();
        assert!(report.changed);
        assert_eq!(
            report.backup, None,
            "an older amx's file is amx's to replace"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());
    }

    #[test]
    fn install_keeps_a_copy_of_a_file_that_is_somebody_elses_and_uninstall_puts_it_back() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let theirs = "// somebody else's\n";
        std::fs::write(&path, theirs).unwrap();

        let report = install_hooks(&FILE, home.path(), AMX, 7).unwrap();
        let backup = report.backup.expect("their file was copied aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());

        let report = uninstall_hooks(&FILE, home.path(), 8).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, Some(backup));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            theirs,
            "their file is back where it was"
        );
    }

    #[test]
    fn uninstall_removes_amxs_file_and_leaves_anyone_elses() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE, home.path());

        // Nothing there is nothing to do.
        let report = uninstall_hooks(&FILE, home.path(), 1).unwrap();
        assert!(!report.changed);

        install_hooks(&FILE, home.path(), AMX, 1).unwrap();
        let report = uninstall_hooks(&FILE, home.path(), 2).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None);
        assert!(!path.exists());

        // A file that is not amx's is not amx's to remove.
        std::fs::write(&path, "// theirs\n").unwrap();
        let report = uninstall_hooks(&FILE, home.path(), 3).unwrap();
        assert!(!report.changed);
        assert!(path.exists());
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
        let settings = Path::new("/home/dev").join(table.wire.path());
        assert_eq!(wire_path(&table, Path::new("/home/dev")), settings);

        let asked = consent_line(&table, &settings, true);
        assert!(asked.contains(&settings.display().to_string()), "{asked}");
        assert!(asked.contains("hooks"), "{asked}");
        assert!(
            asked.contains("copy"),
            "a person is told about the backup: {asked}"
        );
        assert!(!consent_line(&table, &settings, false).contains("copy"));

        // A file wire is a different sentence about a different write.
        let extension = Path::new("/home/dev").join(crate::vendor::pi::HOOKS.wire.path());
        let asked = consent_line(&crate::vendor::pi::HOOKS, &extension, false);
        assert!(asked.contains("extension"), "{asked}");
        assert!(asked.contains(&extension.display().to_string()), "{asked}");
    }

    #[test]
    fn install_writes_the_shape_the_vendor_reads() {
        // Every event the vendor's entry names, each with a matcher exactly
        // where that entry asks for one. Nothing here knows what any of them
        // is called: an event this file spelled for itself is one that would
        // go on being wired after the vendor renamed it.
        let table = claude::VENDOR.hooks.expect("claude reports through hooks");
        let mut settings = json!({});
        assert!(merge(CLAUDE, &mut settings, AMX));

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
        assert!(merge(CLAUDE, &mut settings, AMX));
        let after_first = settings.clone();

        assert!(!merge(CLAUDE, &mut settings, AMX), "nothing left to do");
        assert_eq!(settings, after_first);
    }

    #[test]
    fn install_leaves_hooks_that_are_not_amxs_alone() {
        let mut settings = a_persons_settings();
        merge(CLAUDE, &mut settings, AMX);

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
        merge(CLAUDE, &mut settings, "/usr/local/bin/amx _hook");
        merge(CLAUDE, &mut settings, AMX);

        for event in events(CLAUDE) {
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
        assert!(installed_events(CLAUDE, &settings, AMX).is_empty());

        merge(CLAUDE, &mut settings, AMX);
        let mut wired = installed_events(CLAUDE, &settings, AMX);
        wired.sort();
        let mut expected: Vec<String> = events(CLAUDE).map(|e| e.to_string()).collect();
        expected.sort();
        assert_eq!(wired, expected);

        // An amx somewhere else is not this amx.
        assert!(installed_events(CLAUDE, &settings, "/opt/amx _hook").is_empty());
    }

    #[test]
    fn install_backs_the_file_up_before_it_changes_anything() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let before = "{\n    \"model\": \"opus\"\n}\n";
        std::fs::write(&path, before).unwrap();

        let report = install(CLAUDE, &path, AMX, 1_700_000_000).unwrap();
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

        let report = install(CLAUDE, &path, AMX, 1).unwrap();
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
        install(CLAUDE, &path, AMX, 1).unwrap();
        let after_first = std::fs::read_to_string(&path).unwrap();

        let report = install(CLAUDE, &path, AMX, 2).unwrap();
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

        let refused = install(CLAUDE, &path, AMX, 1).unwrap_err();
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

        install(CLAUDE, &path, AMX, 1).unwrap();
        let report = uninstall(CLAUDE, &path, 2).unwrap();

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
        install(CLAUDE, &path, AMX, 1).unwrap();

        // Somebody edits their settings after amx was installed.
        let mut settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        settings["env"] = json!({ "EDITOR": "emacsclient" });
        settings["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks": [{"type": "command", "command": "~/bin/chime"}]}));
        write(&path, &settings).unwrap();

        uninstall(CLAUDE, &path, 3).unwrap();

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            after["env"]["EDITOR"], "emacsclient",
            "a week of edits is not amx's to undo"
        );
        assert_eq!(hooks(&after, "Stop"), ["~/bin/chime"]);
        assert!(installed_events(CLAUDE, &after, AMX).is_empty());
    }

    #[test]
    fn uninstall_without_a_backup_still_takes_the_hooks_out() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = a_persons_settings();
        merge(CLAUDE, &mut settings, AMX);
        write(&path, &settings).unwrap();

        let report = uninstall(CLAUDE, &path, 1).unwrap();
        assert!(report.changed);

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(installed_events(CLAUDE, &after, AMX).is_empty());
        assert_eq!(hooks(&after, "PreToolUse"), ["~/bin/audit.sh"]);
        assert_eq!(after["model"], "opus");
    }

    #[test]
    fn uninstall_leaves_a_file_amx_never_touched_alone() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let before = "{\n  \"model\": \"opus\"\n}\n";
        std::fs::write(&path, before).unwrap();

        let report = uninstall(CLAUDE, &path, 1).unwrap();
        assert!(!report.changed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn uninstall_takes_the_most_recent_backup() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}\n").unwrap();
        assert_eq!(latest_backup(&path).unwrap(), None);

        install(CLAUDE, &path, AMX, 100).unwrap();
        install(CLAUDE, &path, "/opt/amx _hook", 200).unwrap();

        assert_eq!(
            latest_backup(&path).unwrap(),
            Some(backup_path(&path, 200)),
            "the newest, not whichever the filesystem lists first"
        );
    }

    /// Set a file's mtime to a moment in the past, the way `trust`'s own tests
    /// put an old stamp on a lock.
    fn stamp_the_past(path: &Path) {
        let past = nix::sys::time::TimeSpec::new(1, 0);
        nix::sys::stat::utimensat(
            nix::fcntl::AT_FDCWD,
            path,
            &past,
            &past,
            nix::sys::stat::UtimensatFlags::FollowSymlink,
        )
        .unwrap();
    }

    #[test]
    fn latest_backup_orders_by_the_files_own_mtime_not_the_number_in_its_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");

        // Planted first, and stamped with a number no real backup would ever
        // reach — but its mtime says it is old.
        let planted = backup_path(&path, u64::MAX);
        std::fs::write(&planted, "{}\n").unwrap();
        stamp_the_past(&planted);

        // Taken after it, under an ordinary timestamp.
        let real = backup_path(&path, 5);
        std::fs::write(&real, "{}\n").unwrap();

        assert_eq!(
            latest_backup(&path).unwrap(),
            Some(real),
            "the file taken most recently, not the largest number in a name"
        );
    }

    #[test]
    fn latest_backup_ignores_a_symlink_planted_under_the_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let outside = dir.path().join("outside.json");
        std::fs::write(&outside, "{}\n").unwrap();

        let planted = backup_path(&path, u64::MAX);
        std::os::unix::fs::symlink(&outside, &planted).unwrap();

        assert_eq!(
            latest_backup(&path).unwrap(),
            None,
            "a symlink is not a backup amx wrote, whatever number follows it"
        );
    }

    #[test]
    fn back_up_refuses_to_copy_through_a_planted_symlink() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}\n").unwrap();

        let outside = dir.path().join("outside.json");
        std::fs::write(&outside, "SENTINEL").unwrap();
        let backup = backup_path(&path, 1);
        std::os::unix::fs::symlink(&outside, &backup).unwrap();

        let refused = back_up(&path, 1, true).unwrap_err();
        assert!(
            format!("{refused:#}").contains("settings.json"),
            "{refused:#}"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "SENTINEL",
            "a planted symlink is refused, not written through"
        );
    }

    #[test]
    fn install_never_writes_through_a_symlink_standing_at_the_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let outside = dir.path().join("outside.json");
        std::fs::write(&outside, "{}\n").unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();

        let report = install(CLAUDE, &path, AMX, 1).unwrap();
        assert!(report.changed);

        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink was replaced, not written through"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "{}\n",
            "and whatever it pointed at is untouched"
        );
    }
}
