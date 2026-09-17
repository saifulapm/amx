//! Wiring amx into the vendor's hooks, and taking it back out.
//!
//! Everything amx knows about a running agent arrives through the vendor's own
//! hooks, and every vendor now loads them out of files of amx's own: pi an
//! extension, claude a plugin directory. Which files, and where, is the
//! vendor's entry to say; this file writes what the table holds and knows none
//! of the words itself.
//!
//! amx edits nobody's settings. It did once, merging seven entries into
//! `~/.claude/settings.json` and carrying the rest of the document through a
//! JSON round trip, and the round trip cost a person their hand formatting
//! every time. Writing files amx owns outright costs them nothing, and there
//! is no document to fail to parse.
//!
//! What survives from that door is the one rule worth keeping: **nothing of
//! somebody's is written over without a copy kept beside it.** Whose a file is
//! gets asked differently by each wire — an extension by the first line amx
//! writes into it, a plugin by the manifest in its directory — because a first
//! line cannot tell amx's `SKILL.md` from anybody else's.

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::vendor::{Hooks, Wire};

/// The events amx listens to, under the vendor's own names for them, in
/// wiring order.
///
/// The names come off the vendor's entry: this file writes what the table says
/// and knows none of the words itself. Nothing in the crate proper asks any
/// more — the wires ship written — but the tests that hold a shipped file to
/// its entry ask event by event, and that is the whole of what keeps the two
/// from drifting.
#[cfg_attr(not(test), expect(dead_code, reason = "reached by the tests alone"))]
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

/// The person's home directory, which every wire is written under.
pub fn home() -> Result<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().context("no home directory")
}

/// Where one wire goes, given a home directory: the settings file a vendor
/// reads its hooks out of, the file amx writes where it loads extensions from,
/// or the directory of a plugin amx wrote.
pub fn wire_path(wire: &Wire, home: &Path) -> PathBuf {
    home.join(wire.path())
}

/// The one line a person is asked to agree to before amx writes under their
/// home: what it will write, where, and whether a copy is kept.
pub fn consent_line(wire: &Wire, path: &Path, backup: bool) -> String {
    let and_backup = if backup {
        ", keeping a copy of the file as it is now"
    } else {
        ""
    };
    match *wire {
        Wire::File { .. } => format!(
            "amx will write its extension to {}{and_backup}.",
            path.display()
        ),
        Wire::Plugin { .. } => format!(
            "amx will write its plugin to {}{and_backup}.",
            path.display()
        ),
    }
}

/// What is wired on this machine for a vendor, read without changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wired {
    /// The vendor reports nothing, so there is nothing to be wired.
    Nothing,
    /// A file wire: whether a file stands at the path, and whether it is the
    /// one this amx ships.
    File { present: bool, current: bool },
}

/// Read what is wired at one wire, under `home`.
pub fn wired(wire: &Wire, home: &Path) -> Wired {
    let path = wire_path(wire, home);
    match *wire {
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
        // A plugin is wired when every file it ships is there and is the one
        // this amx ships. Half a plugin is not half wired: claude reads the
        // directory whole, so a missing manifest is a plugin it never loads
        // and a stale hooks file is events amx never hears.
        Wire::Plugin { files, .. } => {
            let mut present = true;
            let mut current = true;
            for (name, body) in files {
                match std::fs::read_to_string(path.join(name)) {
                    Ok(text) => current &= text == *body,
                    Err(_) => {
                        present = false;
                        current = false;
                    }
                }
            }
            Wired::File { present, current }
        }
    }
}

/// Wire one wire under `home`, whichever shape it is.
pub fn install_wire(wire: &Wire, home: &Path, now: u64) -> Result<Report> {
    let path = wire_path(wire, home);
    match *wire {
        Wire::File { body, .. } => install_file(&path, body, now),
        Wire::Plugin { files, .. } => install_plugin(&path, files, now),
    }
}

/// Whether wiring `wire` under `home` would keep a copy of what stands there.
///
/// The consent line is printed before the write, so it cannot read the report:
/// it has to know the rule [`install_file`] and [`install_plugin`] apply. A
/// file that is amx's own, a directory whose manifest says it is amx's, a name
/// with nothing at it, and a file already equal to what amx ships are each
/// written over or skipped with no copy kept; anything else standing there is
/// copied aside first.
pub fn would_keep_a_copy(wire: &Wire, home: &Path) -> bool {
    let path = wire_path(wire, home);
    match *wire {
        Wire::File { body, .. } => std::fs::read_to_string(&path)
            .is_ok_and(|text| text != body && !is_amx_file(&text, body)),
        Wire::Plugin { files, .. } => {
            !is_amx_plugin(&path)
                && files.iter().any(|(name, body)| {
                    std::fs::read_to_string(path.join(name)).is_ok_and(|text| text != *body)
                })
        }
    }
}

/// The file whose contents say a plugin directory is amx's.
///
/// A plugin wire writes several files, and a first line is not enough to tell
/// whose any of them is: amx's `SKILL.md` and somebody's own open with the
/// same three lines. So ownership is asked of the directory once, through the
/// manifest amx writes, and every file under it is answered the same way.
pub const MANIFEST: &str = ".claude-plugin/plugin.json";

/// Whether the plugin directory at `dir` is one amx wrote.
///
/// A manifest that does not parse, or names somebody else, is somebody else's
/// directory: amx will write its own files into it and keep copies of whatever
/// it wrote over, which is what it does for a directory with no manifest at
/// all.
fn is_amx_plugin(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join(MANIFEST)) else {
        return false;
    };
    serde_json::from_str::<Value>(&text).is_ok_and(|manifest| manifest["name"] == "amx")
}

/// Write amx's plugin into `dir`, and answer whether anything needed writing.
///
/// A directory already amx's is amx's to rewrite: an older amx's files are
/// replaced where they differ and nothing is kept, the way an older amx's
/// extension is. A directory that is not amx's may hold a file of the
/// person's at one of these names — theirs is copied aside before amx's goes
/// over it.
pub fn install_plugin(dir: &Path, files: &[(&str, &str)], now: u64) -> Result<Report> {
    let ours = is_amx_plugin(dir);
    let mut report = Report {
        path: dir.to_path_buf(),
        backup: None,
        changed: false,
    };
    for (name, body) in files {
        let path = dir.join(name);
        let existing = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if existing.as_deref() == Some(*body) {
            continue;
        }
        if !ours && existing.is_some() {
            report.backup = back_up(&path, now, true)?.or(report.backup.take());
        }
        write_bytes(&path, body.as_bytes())?;
        report.changed = true;
    }
    Ok(report)
}

/// Take amx's plugin away again, and put back whatever it was written over.
///
/// A directory whose manifest is not amx's is not amx's to empty, however many
/// of these names it happens to carry.
pub fn uninstall_plugin(dir: &Path, files: &[(&str, &str)], _now: u64) -> Result<Report> {
    let mut report = Report {
        path: dir.to_path_buf(),
        backup: None,
        changed: false,
    };
    if !is_amx_plugin(dir) {
        return Ok(report);
    }
    for (name, _) in files {
        let path = dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => report.changed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
        if let Some(kept) = latest_backup(&path)? {
            std::fs::copy(&kept, &path)
                .with_context(|| format!("putting {} back", path.display()))?;
            report.backup = Some(kept);
        }
    }
    prune(dir, files);
    Ok(report)
}

/// Take away what amx's plugin left empty behind it, and nothing else.
///
/// `remove_dir` refuses a directory with anything in it, which is the whole of
/// the rule: a directory still holding a file of somebody's stays, and so does
/// the one their file was put back into.
fn prune(dir: &Path, files: &[(&str, &str)]) {
    for (name, _) in files {
        let mut at = dir.join(name);
        while at.pop() && at.starts_with(dir) && at != dir {
            let _ = std::fs::remove_dir(&at);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Take one wire back out from under `home`, whichever shape it is.
pub fn uninstall_wire(wire: &Wire, home: &Path, now: u64) -> Result<Report> {
    let path = wire_path(wire, home);
    match *wire {
        Wire::File { body, .. } => uninstall_file(&path, body, now),
        Wire::Plugin { files, .. } => uninstall_plugin(&path, files, now),
    }
}

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
/// Write a file, making the directories above it on the way.
///
/// Staged beside the target and renamed over it, rather than written to the
/// path directly: a rename replaces whatever is at that name without ever
/// opening it, so a symlink standing at the path is replaced and never
/// written through.
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
        let path = wire_path(&FILE.wire, home.path());
        assert_eq!(
            wired(&FILE.wire, home.path()),
            Wired::File {
                present: false,
                current: false
            }
        );

        let report = install_wire(&FILE.wire, home.path(), 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "nothing was there to keep");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());
        assert_eq!(
            wired(&FILE.wire, home.path()),
            Wired::File {
                present: true,
                current: true
            }
        );

        // Installed twice is installed once.
        let again = install_wire(&FILE.wire, home.path(), 2).unwrap();
        assert!(!again.changed);
        assert_eq!(again.backup, None);
    }

    #[test]
    fn install_replaces_an_older_amx_file_without_keeping_it() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE.wire, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "// installed by amx\n// an older one\n").unwrap();
        assert_eq!(
            wired(&FILE.wire, home.path()),
            Wired::File {
                present: true,
                current: false
            }
        );

        let report = install_wire(&FILE.wire, home.path(), 1).unwrap();
        assert!(report.changed);
        assert_eq!(
            report.backup, None,
            "an older amx's file is amx's to replace"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());
    }

    #[test]
    fn install_knows_whether_it_would_keep_a_copy_before_it_writes() {
        // The consent line is printed before the write and cannot read the
        // report, so the prediction has to agree with what install_file and
        // install_plugin would do: a missing file, amx's own file, and a file
        // already equal to amx's are written over or skipped with no copy;
        // somebody else's is copied aside.
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE.wire, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert!(
            !would_keep_a_copy(&FILE.wire, home.path()),
            "nothing stands there"
        );

        std::fs::write(&path, file_body()).unwrap();
        assert!(
            !would_keep_a_copy(&FILE.wire, home.path()),
            "already what amx ships"
        );

        std::fs::write(&path, "// installed by amx\n// an older one\n").unwrap();
        assert!(
            !would_keep_a_copy(&FILE.wire, home.path()),
            "an older amx's file is amx's to replace"
        );

        std::fs::write(&path, "// somebody else's\n").unwrap();
        assert!(
            would_keep_a_copy(&FILE.wire, home.path()),
            "somebody else's file is copied aside"
        );

        // And a plugin's: a directory with amx's manifest is rewritten whole,
        // an empty one holds nothing to copy, and somebody else's is copied
        // file by file before amx's goes over it.
        let ours = TempDir::new().unwrap();
        install_wire(&claude::HOOKS.wire, ours.path(), 1).unwrap();
        assert!(!would_keep_a_copy(&claude::HOOKS.wire, ours.path()));

        let theirs = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, theirs.path());
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            !would_keep_a_copy(&claude::HOOKS.wire, theirs.path()),
            "an empty directory holds nothing to copy"
        );
        std::fs::write(dir.join("SKILL.md"), "their own copy\n").unwrap();
        assert!(would_keep_a_copy(&claude::HOOKS.wire, theirs.path()));
    }

    #[test]
    fn install_keeps_a_copy_of_a_file_that_is_somebody_elses_and_uninstall_puts_it_back() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE.wire, home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let theirs = "// somebody else's\n";
        std::fs::write(&path, theirs).unwrap();

        let report = install_wire(&FILE.wire, home.path(), 7).unwrap();
        let backup = report.backup.expect("their file was copied aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());

        let report = uninstall_wire(&FILE.wire, home.path(), 8).unwrap();
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
        let path = wire_path(&FILE.wire, home.path());

        // Nothing there is nothing to do.
        let report = uninstall_wire(&FILE.wire, home.path(), 1).unwrap();
        assert!(!report.changed);

        install_wire(&FILE.wire, home.path(), 1).unwrap();
        let report = uninstall_wire(&FILE.wire, home.path(), 2).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None);
        assert!(!path.exists());

        // A file that is not amx's is not amx's to remove.
        std::fs::write(&path, "// theirs\n").unwrap();
        let report = uninstall_wire(&FILE.wire, home.path(), 3).unwrap();
        assert!(!report.changed);
        assert!(path.exists());
    }

    /// claude's own plugin, and the three files it ships.
    fn plugin() -> (&'static str, &'static [(&'static str, &'static str)]) {
        let Wire::Plugin { dir, files } = claude::HOOKS.wire else {
            panic!("claude reports through a plugin");
        };
        (dir, files)
    }

    #[test]
    fn install_writes_a_plugin_whole_and_reads_it_back() {
        let home = TempDir::new().unwrap();
        let (_, files) = plugin();
        let dir = wire_path(&claude::HOOKS.wire, home.path());
        assert_eq!(
            wired(&claude::HOOKS.wire, home.path()),
            Wired::File {
                present: false,
                current: false
            }
        );

        let report = install_wire(&claude::HOOKS.wire, home.path(), 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "nothing was there to keep");
        for (name, body) in files {
            assert_eq!(&std::fs::read_to_string(dir.join(name)).unwrap(), body);
        }
        assert_eq!(
            wired(&claude::HOOKS.wire, home.path()),
            Wired::File {
                present: true,
                current: true
            }
        );

        // Written twice is written once.
        let again = install_wire(&claude::HOOKS.wire, home.path(), 2).unwrap();
        assert!(!again.changed);
        assert_eq!(again.backup, None);
    }

    #[test]
    fn install_replaces_an_older_amxs_plugin_without_keeping_it() {
        // The manifest is what says the directory is amx's, so a file beside
        // one an older amx wrote is amx's to overwrite. Keeping a copy of
        // every one of those would leave a backup behind at every upgrade.
        let home = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, home.path());
        install_wire(&claude::HOOKS.wire, home.path(), 1).unwrap();
        std::fs::write(dir.join("SKILL.md"), "an older amx's skill\n").unwrap();
        assert_eq!(
            wired(&claude::HOOKS.wire, home.path()),
            Wired::File {
                present: true,
                current: false
            }
        );

        let report = install_wire(&claude::HOOKS.wire, home.path(), 2).unwrap();
        assert!(report.changed);
        assert_eq!(
            report.backup, None,
            "an older amx's file is amx's to replace"
        );
        assert_eq!(latest_backup(&dir.join("SKILL.md")).unwrap(), None);
    }

    #[test]
    fn install_keeps_a_file_in_the_plugins_directory_that_is_somebody_elses() {
        // A person's own skill stands at the same name amx ships one under,
        // and opens with the same three lines, so nothing about the file
        // itself tells them apart. The directory answers instead: one with no
        // manifest of amx's in it is not amx's, and what is in it is kept.
        let home = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, home.path());
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = "---\nname: amx\n---\n\ntheir own copy\n";
        std::fs::write(dir.join("SKILL.md"), theirs).unwrap();

        let report = install_wire(&claude::HOOKS.wire, home.path(), 7).unwrap();
        let backup = report.backup.expect("their file was copied aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);
        assert_ne!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            theirs,
            "and amx's own is what stands there now"
        );

        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), 8).unwrap();
        assert!(report.changed);
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            theirs,
            "their file is back where it was"
        );
    }

    #[test]
    fn uninstall_removes_amxs_plugin_and_leaves_anyone_elses() {
        let home = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, home.path());

        // Nothing there is nothing to do.
        assert!(
            !uninstall_wire(&claude::HOOKS.wire, home.path(), 1)
                .unwrap()
                .changed
        );

        install_wire(&claude::HOOKS.wire, home.path(), 1).unwrap();
        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), 2).unwrap();
        assert!(report.changed);
        assert!(!dir.exists(), "and the directory it emptied goes too");

        // A directory whose manifest is somebody else's is not amx's to empty,
        // however many of these names it carries.
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(dir.join(MANIFEST), "{\"name\": \"theirs\"}\n").unwrap();
        std::fs::write(dir.join("SKILL.md"), "theirs\n").unwrap();
        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), 3).unwrap();
        assert!(!report.changed);
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "theirs\n"
        );
    }

    #[test]
    fn install_asks_before_it_writes_anything() {
        // The file is the vendor's, under the person's home, and the sentence
        // names it in full because that is the thing being agreed to.
        let table = claude::VENDOR.hooks.expect("claude reports through hooks");
        let plugin = Path::new("/home/dev").join(table.wire.path());
        assert_eq!(wire_path(&table.wire, Path::new("/home/dev")), plugin);

        let asked = consent_line(&table.wire, &plugin, true);
        assert!(asked.contains(&plugin.display().to_string()), "{asked}");
        assert!(asked.contains("plugin"), "{asked}");
        assert!(
            asked.contains("copy"),
            "a person is told about the backup: {asked}"
        );
        assert!(!consent_line(&table.wire, &plugin, false).contains("copy"));

        // A file wire is a different sentence about a different write.
        let extension = Path::new("/home/dev").join(crate::vendor::pi::HOOKS.wire.path());
        let asked = consent_line(&crate::vendor::pi::HOOKS.wire, &extension, false);
        assert!(asked.contains("extension"), "{asked}");
        assert!(asked.contains(&extension.display().to_string()), "{asked}");
    }

    #[test]
    fn the_plugin_wires_exactly_the_events_the_vendors_entry_names() {
        // The plugin is the same wiring by another door, so it is held to the
        // same table `install` writes from rather than to a list spelled here:
        // an event claude's entry stops naming, or starts, is an event the
        // plugin would go on being loaded with while amx heard nothing of it.
        let table = claude::VENDOR.hooks.expect("claude reports through hooks");
        let Wire::Plugin { files, .. } = table.wire else {
            panic!("claude reports through a plugin");
        };
        let body = |wanted: &str| {
            files
                .iter()
                .find_map(|(name, body)| (*name == wanted).then_some(*body))
                .unwrap_or_else(|| panic!("the plugin ships {wanted}"))
        };

        let plugin: Value = serde_json::from_str(body(MANIFEST)).unwrap();
        assert_eq!(
            plugin["name"], "amx",
            "the name is what says the directory is amx's"
        );
        assert!(
            plugin.get("hooks").is_none(),
            "Claude Code loads hooks/hooks.json by convention and refuses a manifest naming it twice"
        );

        let wiring_file: Value = serde_json::from_str(body("hooks/hooks.json")).unwrap();
        let by_event = wiring_file["hooks"]
            .as_object()
            .expect("one entry per event");
        assert_eq!(
            by_event.len(),
            table.events.len(),
            "the plugin wires every event the entry names and nothing else"
        );

        // On the PATH, which is the one amx doctor already insists on: no
        // wire carries the path this amx happens to stand at.
        let command = "amx _hook";
        for wiring in table.events {
            assert_eq!(
                hooks(&wiring_file, wiring.event),
                [command],
                "{}",
                wiring.event
            );
            let group = wiring_file["hooks"][wiring.event][0].clone();
            assert_eq!(group["hooks"][0]["type"], "command", "{}", wiring.event);
            if wiring.matched {
                assert_eq!(group["matcher"], table.matcher, "{}", wiring.event);
            } else {
                assert!(group.get("matcher").is_none(), "{}", wiring.event);
            }
        }
    }

    #[test]
    fn uninstall_takes_the_most_recent_backup() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}\n").unwrap();
        assert_eq!(latest_backup(&path).unwrap(), None);

        install_file(&path, "theirs\n", 100).unwrap();
        install_file(&path, "and theirs again\n", 200).unwrap();

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

        let report = install_file(&path, "// installed by amx\n", 1).unwrap();
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
