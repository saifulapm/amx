//! Wiring amx into the vendor's hooks, and taking it back out.
//!
//! Everything amx knows about a running agent arrives through the vendor's own
//! hooks. pi and claude load them out of files of amx's own: pi an extension,
//! claude a plugin directory. codex has no such door, so amx merges its groups
//! into codex's `hooks.json` and trusts each in `config.toml`. Which files, and
//! where, is the vendor's entry to say; this file writes what the table holds
//! and knows none of the words itself.
//!
//! amx edited claude's settings once, merging seven entries into
//! `~/.claude/settings.json` and carrying the rest of the document through a
//! JSON round trip, and the round trip cost a person their hand formatting
//! every time. Writing files amx owns outright costs them nothing. codex's two
//! files are the exception: the hooks file goes through the same round trip,
//! the config is edited in place with `toml_edit`, and each is copied aside
//! before amx first edits it so uninstall can put it back.
//!
//! What survives from that door is the one rule worth keeping: **nothing of
//! somebody's is written over without a copy kept beside it.** Whose a file is
//! gets asked differently by each wire — an extension by the first line amx
//! writes into it, a plugin by the manifest in its directory — because a first
//! line cannot tell amx's `SKILL.md` from anybody else's.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
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

/// How a wire asks for a variable of the environment: the process's own in
/// the verbs, one of the test's making in a test, so no test ever reads where
/// the person running it keeps a vendor.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// The environment this process was started in.
pub fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// An environment with nothing set in it, for the tests.
#[cfg(test)]
pub fn no_env(_: &str) -> Option<OsString> {
    None
}

/// A hooks wire of the tests' own, shaped like codex's: the body is the one
/// codex's entry ships.
#[cfg(test)]
pub const HOOKS_WIRE: Wire = Wire::Hooks {
    dir_env: "CODEX_HOME",
    dir: ".codex",
    body: include_str!("../assets/codex/hooks.json"),
};

/// Where a vendor that is told its directory by a variable keeps its files:
/// the variable's value resolved to its real path when it is set and not
/// empty, else `dir` under the home. codex's own rule for `CODEX_HOME`, which
/// is also the path its trust keys are spelled with.
pub fn vendor_dir(set: Option<OsString>, dir: &str, home: &Path) -> PathBuf {
    match set.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = PathBuf::from(value);
            path.canonicalize().unwrap_or(path)
        }
        None => home.join(dir),
    }
}

/// Where one wire goes, given a home directory and the environment: the file
/// amx writes where a vendor loads extensions from, the directory of a plugin
/// amx wrote, the directory holding the hooks file amx merges into, or the
/// file placed in a directory its variable may move.
///
/// Every verb asks this, so setup, doctor, uninstall and the trust keys all
/// agree on where codex lives.
pub fn wire_path(wire: &Wire, home: &Path, env: Env) -> PathBuf {
    match *wire {
        Wire::Hooks { dir_env, dir, .. } => vendor_dir(env(dir_env), dir, home),
        Wire::Placed {
            dir_env, dir, path, ..
        } => vendor_dir(env(dir_env), dir, home).join(path),
        _ => home.join(wire.path()),
    }
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
        Wire::Plugin { .. } | Wire::Placed { .. } => format!(
            "amx will write its plugin to {}{and_backup}.",
            path.display()
        ),
        Wire::Hooks { .. } => format!(
            "amx will add its hooks to {} and trust them in {}{}.",
            path.join(HOOKS_FILE).display(),
            path.join(CONFIG_FILE).display(),
            if backup {
                ", keeping a copy of each file as it is now"
            } else {
                ""
            }
        ),
    }
}

/// What is wired on this machine for a vendor, read without changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wired {
    /// The vendor reports nothing, so there is nothing to be wired.
    Nothing,
    /// A file wire: whether a file stands at the path, and whether it is the
    /// one this amx ships. For a hooks wire, whether amx's groups are in the
    /// hooks file, and whether each is trusted as it stands.
    File { present: bool, current: bool },
}

/// Read what is wired at one wire, under `home`.
pub fn wired(wire: &Wire, home: &Path, env: Env) -> Wired {
    let path = wire_path(wire, home, env);
    match *wire {
        Wire::File { body, .. } | Wire::Placed { body, .. } => match std::fs::read_to_string(&path)
        {
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
        Wire::Hooks { body, .. } => {
            let Ok(read) = Merge::read(&path, body) else {
                return Wired::File {
                    present: false,
                    current: false,
                };
            };
            let present = read.hooks.as_ref() == Some(&read.merged);
            Wired::File {
                present,
                current: present && read.config_unchanged(),
            }
        }
    }
}

/// Wire one wire under `home`, whichever shape it is.
pub fn install_wire(wire: &Wire, home: &Path, env: Env, now: u64) -> Result<Report> {
    let path = wire_path(wire, home, env);
    match *wire {
        Wire::File { body, .. } | Wire::Placed { body, .. } => install_file(&path, body, now),
        Wire::Plugin { files, .. } => install_plugin(&path, files, now),
        Wire::Hooks { body, .. } => install_hooks(&path, body, now),
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
pub fn would_keep_a_copy(wire: &Wire, home: &Path, env: Env) -> bool {
    let path = wire_path(wire, home, env);
    match *wire {
        Wire::File { body, .. } | Wire::Placed { body, .. } => std::fs::read_to_string(&path)
            .is_ok_and(|text| text != body && !is_amx_file(&text, body)),
        Wire::Plugin { files, .. } => {
            !is_amx_plugin(&path)
                && files.iter().any(|(name, body)| {
                    std::fs::read_to_string(path.join(name)).is_ok_and(|text| text != *body)
                })
        }
        Wire::Hooks { body, .. } => {
            Merge::read(&path, body).is_ok_and(|read| read.copies_hooks() || read.copies_config())
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
/// person's at one of these names — theirs is copied aside before amx writes
/// anything, and the manifest goes in last, so a run that stops partway
/// leaves a directory the next run still reads as not amx's.
pub fn install_plugin(dir: &Path, files: &[(&str, &str)], now: u64) -> Result<Report> {
    let ours = is_amx_plugin(dir);
    let mut report = Report {
        path: dir.to_path_buf(),
        backup: None,
        changed: false,
    };
    let mut writes = Vec::new();
    for (name, body) in manifest_last(files) {
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
        writes.push((path, body));
    }
    for (path, body) in writes {
        write_bytes(&path, body.as_bytes())?;
        report.changed = true;
    }
    Ok(report)
}

/// A plugin's files with its manifest moved to the end.
///
/// The manifest is what says the directory is amx's, so it is the last thing
/// written and the last thing removed: until it stands, the directory is still
/// somebody else's and a foreign file in it still gets a copy kept, and until
/// it goes, a run that stopped partway can still be finished.
fn manifest_last<'a>(
    files: &'a [(&'a str, &'a str)],
) -> impl Iterator<Item = &'a (&'a str, &'a str)> {
    let (manifest, rest): (Vec<_>, Vec<_>) = files.iter().partition(|(name, _)| *name == MANIFEST);
    rest.into_iter().chain(manifest)
}

/// Take amx's plugin away again, and put back whatever it was written over.
///
/// A directory whose manifest is not amx's is not amx's to empty, however many
/// of these names it happens to carry. The manifest goes last, so a run that
/// stops partway leaves one the next run can finish.
pub fn uninstall_plugin(dir: &Path, files: &[(&str, &str)], _now: u64) -> Result<Report> {
    let mut report = Report {
        path: dir.to_path_buf(),
        backup: None,
        changed: false,
    };
    if !is_amx_plugin(dir) {
        return Ok(report);
    }
    for (name, _) in manifest_last(files) {
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
pub fn uninstall_wire(wire: &Wire, home: &Path, env: Env, now: u64) -> Result<Report> {
    let path = wire_path(wire, home, env);
    match *wire {
        Wire::File { body, .. } | Wire::Placed { body, .. } => uninstall_file(&path, body, now),
        Wire::Plugin { files, .. } => uninstall_plugin(&path, files, now),
        Wire::Hooks { body, .. } => uninstall_hooks(&path, body),
    }
}

/// The hooks file a hooks wire merges amx's groups into, in the vendor's
/// directory.
pub const HOOKS_FILE: &str = "hooks.json";
/// The config beside it that says which handlers the person trusts.
pub const CONFIG_FILE: &str = "config.toml";

/// The hash codex keeps in `trusted_hash` for a group's one handler, and lists
/// as its `currentHash`.
///
/// codex hashes what it made of the handler rather than what was written: the
/// event under its snake-case name, the matcher where one is written (never
/// for `user_prompt_submit` or `stop`, which take none), and the handler's
/// type, command, timeout (600 when none is given) and async (false). The
/// JSON is serialised compactly with every object's keys sorted — which
/// `serde_json`'s map is, built without `preserve_order` — and the sha256 of
/// it is written out byte by byte in lower-case hex.
pub fn trusted_hash(event_snake: &str, group: &Value) -> String {
    let handler = &group["hooks"][0];
    let mut identity = json!({
        "event_name": event_snake,
        "hooks": [{
            "async": handler.get("async").cloned().unwrap_or(json!(false)),
            "command": handler["command"],
            "timeout": handler.get("timeout").cloned().unwrap_or(json!(600)),
            "type": "command",
        }],
    });
    if let Some(matcher) = group.get("matcher").filter(|matcher| !matcher.is_null())
        && !matches!(event_snake, "user_prompt_submit" | "stop")
    {
        identity["matcher"] = matcher.clone();
    }
    let digest = Sha256::digest(identity.to_string().as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// An event's name as codex spells it in a trust key: `PreToolUse` is
/// `pre_tool_use`.
fn snake(event: &str) -> String {
    let mut out = String::new();
    for (at, c) in event.char_indices() {
        if c.is_ascii_uppercase() {
            if at > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The command amx's groups run: the one the shipped hooks file names.
fn amx_command(shipped: &Value) -> &str {
    shipped["hooks"]
        .as_object()
        .and_then(|events| events.values().next())
        .and_then(|groups| groups[0]["hooks"][0]["command"].as_str())
        .unwrap_or_default()
}

/// Whether a group is amx's: its one handler runs amx's hook command.
fn is_amx_group(group: &Value, command: &str) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|handlers| handlers.len() == 1 && handlers[0]["command"] == command)
}

/// What a hooks wire finds in the vendor's directory, and what amx would make
/// of it: both files as they stand, and as they would stand with amx's groups
/// merged in and trusted.
struct Merge {
    hooks_path: PathBuf,
    config_path: PathBuf,
    hooks: Option<Value>,
    merged: Value,
    config: Option<toml_edit::DocumentMut>,
    trusted: toml_edit::DocumentMut,
    /// Whether the hooks file held a group of amx's before the merge.
    had_groups: bool,
    /// Whether the config held a trust entry under one of amx's keys.
    had_trust: bool,
}

impl Merge {
    /// Read both files in `dir`, and merge amx's groups in `body` into them.
    ///
    /// Each event's array gets amx's group appended, unless a group of amx's
    /// is already in it; the trust key is the index the group stands at.
    fn read(dir: &Path, body: &str) -> Result<Merge> {
        let hooks_path = dir.join(HOOKS_FILE);
        let config_path = dir.join(CONFIG_FILE);
        let shipped: Value = serde_json::from_str(body).context("amx's own hooks file")?;
        let command = amx_command(&shipped);
        let hooks = match read_text(&hooks_path)? {
            Some(text) => Some(
                serde_json::from_str::<Value>(&text)
                    .with_context(|| format!("reading {}", hooks_path.display()))?,
            ),
            None => None,
        };
        let config = match read_text(&config_path)? {
            Some(text) => Some(
                text.parse::<toml_edit::DocumentMut>()
                    .with_context(|| format!("reading {}", config_path.display()))?,
            ),
            None => None,
        };

        let mut merged = hooks.clone().unwrap_or_else(|| json!({}));
        let mut had_groups = false;
        let mut trust = Vec::new();
        {
            let events = merged
                .as_object_mut()
                .context("the hooks file is not an object")?
                .entry("hooks")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("its `hooks` is not an object")?;
            for (event, groups) in shipped["hooks"].as_object().into_iter().flatten() {
                let list = events
                    .entry(event.clone())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .with_context(|| format!("its `{event}` is not a list"))?;
                for group in groups.as_array().into_iter().flatten() {
                    let at = match list.iter().position(|it| is_amx_group(it, command)) {
                        Some(at) => {
                            had_groups = true;
                            at
                        }
                        None => {
                            list.push(group.clone());
                            list.len() - 1
                        }
                    };
                    let snake = snake(event);
                    trust.push((
                        format!("{}:{snake}:{at}:0", hooks_path.display()),
                        trusted_hash(&snake, &list[at]),
                    ));
                }
            }
        }

        let mut trusted = config.clone().unwrap_or_default();
        let state = trust_table(&mut trusted)?;
        let had_trust = trust.iter().any(|(key, _)| state.contains_key(key));
        for (key, hash) in trust {
            let entry = state
                .entry(&key)
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_like_mut()
                .with_context(|| format!("[hooks.state.\"{key}\"] is not a table"))?;
            if entry.get("trusted_hash").and_then(|it| it.as_str()) != Some(hash.as_str()) {
                entry.insert("trusted_hash", toml_edit::value(hash));
            }
        }

        Ok(Merge {
            hooks_path,
            config_path,
            hooks,
            merged,
            config,
            trusted,
            had_groups,
            had_trust,
        })
    }

    /// Whether writing the hooks file would copy it aside first: it is there,
    /// and amx has never put a group in it.
    fn copies_hooks(&self) -> bool {
        self.hooks.is_some() && !self.had_groups
    }

    /// Whether the config already trusts every group of amx's as it stands.
    fn config_unchanged(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|config| config.to_string() == self.trusted.to_string())
    }

    /// The same of the config: it is there, and holds no trust of amx's.
    fn copies_config(&self) -> bool {
        self.config.is_some() && !self.had_trust
    }
}

/// `[hooks.state]` in a codex config, made where it is missing. A table amx
/// makes is implicit, so a config holding nothing else prints no empty
/// `[hooks]` header.
fn trust_table(doc: &mut toml_edit::DocumentMut) -> Result<&mut toml_edit::Table> {
    let implicit = || {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    };
    doc.entry("hooks")
        .or_insert_with(implicit)
        .as_table_mut()
        .context("[hooks] is not a table")?
        .entry("state")
        .or_insert_with(implicit)
        .as_table_mut()
        .context("[hooks.state] is not a table")
}

/// A file's text, or `None` when there is no file.
fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Merge amx's groups into the hooks file in `dir` and trust each in the
/// config beside it, copying either file aside before amx first edits it.
///
/// Run again with nothing moved, it writes nothing: both files are compared
/// with what they would become before either is touched.
pub fn install_hooks(dir: &Path, body: &str, now: u64) -> Result<Report> {
    let merge = Merge::read(dir, body)?;
    let mut report = Report {
        path: merge.hooks_path.clone(),
        backup: None,
        changed: false,
    };
    if merge.hooks.as_ref() != Some(&merge.merged) {
        report.backup = back_up(&merge.hooks_path, now, merge.copies_hooks())?;
        let text = serde_json::to_string_pretty(&merge.merged)? + "\n";
        write_bytes(&merge.hooks_path, text.as_bytes())?;
        report.changed = true;
    }
    if !merge.config_unchanged() {
        let kept = back_up(&merge.config_path, now, merge.copies_config())?;
        report.backup = report.backup.or(kept);
        write_bytes(&merge.config_path, merge.trusted.to_string().as_bytes())?;
        report.changed = true;
    }
    Ok(report)
}

/// Take amx's groups and their trust back out of the files in `dir`.
///
/// A file that holds nothing now but what amx added is put back as it was: the
/// copy kept before amx's first edit goes back byte for byte, and a file amx
/// made is removed. A file somebody has changed since keeps their change, and
/// only amx's groups, their `[hooks.state]` tables, and a `[hooks.state]` or
/// `[hooks]` table that leaves empty come out.
///
/// Taking a group out shifts every later group in its event down by one, and
/// the trust keys are indices: codex asks again about the groups the person
/// added after amx's, because their keys now name different places.
pub fn uninstall_hooks(dir: &Path, body: &str) -> Result<Report> {
    let hooks_path = dir.join(HOOKS_FILE);
    let config_path = dir.join(CONFIG_FILE);
    let mut report = Report {
        path: hooks_path.clone(),
        backup: None,
        changed: false,
    };
    let shipped: Value = serde_json::from_str(body).context("amx's own hooks file")?;
    let command = amx_command(&shipped);
    let Some(text) = read_text(&hooks_path)? else {
        return Ok(report);
    };
    let hooks: Value =
        serde_json::from_str(&text).with_context(|| format!("reading {}", hooks_path.display()))?;
    let shown = hooks_path.display().to_string();
    let keys: Vec<String> = hooks["hooks"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(event, groups)| {
            let snake = snake(event);
            let shown = &shown;
            groups
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter(|(_, group)| is_amx_group(group, command))
                .map(move |(at, _)| format!("{shown}:{snake}:{at}:0"))
        })
        .collect();
    if keys.is_empty() {
        return Ok(report);
    }

    if let Some(text) = read_text(&config_path)? {
        let mut config = text
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("reading {}", config_path.display()))?;
        if untrust(&mut config, &keys) {
            let backup = latest_backup(&config_path)?.filter(|kept| {
                std::fs::read_to_string(kept)
                    .ok()
                    .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
                    .is_some_and(|mut theirs| {
                        untrust(&mut theirs, &keys);
                        same_config(&theirs, &config)
                    })
            });
            let empty = config.as_table().is_empty();
            restore(
                &config_path,
                backup.as_deref(),
                empty,
                config.to_string().as_bytes(),
            )?;
            report.backup = backup;
        }
    }

    let stripped = strip(&hooks, command);
    let backup = latest_backup(&hooks_path)?.filter(|kept| {
        std::fs::read_to_string(kept)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|theirs| strip(&theirs, command) == stripped)
    });
    let empty = stripped == json!({}) || stripped == json!({"hooks": {}});
    let text = serde_json::to_string_pretty(&stripped)? + "\n";
    restore(&hooks_path, backup.as_deref(), empty, text.as_bytes())?;
    report.backup = backup.or(report.backup);
    report.changed = true;
    Ok(report)
}

/// Put a file amx edited back: the copy kept before its first edit when there
/// is one that still holds everything else the file does, nothing at all when
/// nothing else is left in it, and otherwise the file without amx's part.
fn restore(path: &Path, kept: Option<&Path>, empty: bool, rest: &[u8]) -> Result<()> {
    match kept {
        Some(kept) => {
            let bytes =
                std::fs::read(kept).with_context(|| format!("reading {}", kept.display()))?;
            write_bytes(path, &bytes)
        }
        None if empty => {
            std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
        }
        None => write_bytes(path, rest),
    }
}

/// A hooks file without amx's groups, and without an event amx's group was
/// the last of. Whether an event's list is empty or missing says the same
/// thing to codex, so neither is kept.
fn strip(hooks: &Value, command: &str) -> Value {
    let mut stripped = hooks.clone();
    if let Some(events) = stripped["hooks"].as_object_mut() {
        for groups in events.values_mut() {
            if let Some(list) = groups.as_array_mut() {
                list.retain(|group| !is_amx_group(group, command));
            }
        }
        events.retain(|_, groups| groups.as_array().is_none_or(|list| !list.is_empty()));
    }
    stripped
}

/// Take the trust tables under `keys` out of a config, and `[hooks.state]` and
/// `[hooks]` with them where that leaves them empty. Answers whether there was
/// any to take.
fn untrust(config: &mut toml_edit::DocumentMut, keys: &[String]) -> bool {
    let Some(hooks) = config
        .get_mut("hooks")
        .and_then(|it| it.as_table_like_mut())
    else {
        return false;
    };
    let Some(state) = hooks.get_mut("state").and_then(|it| it.as_table_like_mut()) else {
        return false;
    };
    let mut removed = false;
    for key in keys {
        removed |= state.remove(key).is_some();
    }
    if !removed {
        return false;
    }
    if state.is_empty() {
        hooks.remove("state");
        if hooks.is_empty() {
            config.remove("hooks");
        }
    }
    true
}

/// Whether two configs say the same thing, an empty table being the same as
/// none: codex writes an empty `[hooks.state]` that amx's removal takes away.
fn same_config(a: &toml_edit::DocumentMut, b: &toml_edit::DocumentMut) -> bool {
    fn said(doc: &toml_edit::DocumentMut) -> Option<toml::Table> {
        let mut table = doc.to_string().parse::<toml::Table>().ok()?;
        prune(&mut table);
        Some(table)
    }
    fn prune(table: &mut toml::Table) {
        for (_, value) in table.iter_mut() {
            if let toml::Value::Table(inner) = value {
                prune(inner);
            }
        }
        table.retain(|_, value| !matches!(value, toml::Value::Table(inner) if inner.is_empty()));
    }
    said(a).is_some_and(|a| Some(a) == said(b))
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
        let path = wire_path(&FILE.wire, home.path(), &no_env);
        assert_eq!(
            wired(&FILE.wire, home.path(), &no_env),
            Wired::File {
                present: false,
                current: false
            }
        );

        let report = install_wire(&FILE.wire, home.path(), &no_env, 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "nothing was there to keep");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());
        assert_eq!(
            wired(&FILE.wire, home.path(), &no_env),
            Wired::File {
                present: true,
                current: true
            }
        );

        // Installed twice is installed once.
        let again = install_wire(&FILE.wire, home.path(), &no_env, 2).unwrap();
        assert!(!again.changed);
        assert_eq!(again.backup, None);
    }

    #[test]
    fn install_replaces_an_older_amx_file_without_keeping_it() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE.wire, home.path(), &no_env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "// installed by amx\n// an older one\n").unwrap();
        assert_eq!(
            wired(&FILE.wire, home.path(), &no_env),
            Wired::File {
                present: true,
                current: false
            }
        );

        let report = install_wire(&FILE.wire, home.path(), &no_env, 1).unwrap();
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
        let path = wire_path(&FILE.wire, home.path(), &no_env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert!(
            !would_keep_a_copy(&FILE.wire, home.path(), &no_env),
            "nothing stands there"
        );

        std::fs::write(&path, file_body()).unwrap();
        assert!(
            !would_keep_a_copy(&FILE.wire, home.path(), &no_env),
            "already what amx ships"
        );

        std::fs::write(&path, "// installed by amx\n// an older one\n").unwrap();
        assert!(
            !would_keep_a_copy(&FILE.wire, home.path(), &no_env),
            "an older amx's file is amx's to replace"
        );

        std::fs::write(&path, "// somebody else's\n").unwrap();
        assert!(
            would_keep_a_copy(&FILE.wire, home.path(), &no_env),
            "somebody else's file is copied aside"
        );

        // And a plugin's: a directory with amx's manifest is rewritten whole,
        // an empty one holds nothing to copy, and somebody else's is copied
        // file by file before amx's goes over it.
        let ours = TempDir::new().unwrap();
        install_wire(&claude::HOOKS.wire, ours.path(), &no_env, 1).unwrap();
        assert!(!would_keep_a_copy(
            &claude::HOOKS.wire,
            ours.path(),
            &no_env
        ));

        let theirs = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, theirs.path(), &no_env);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            !would_keep_a_copy(&claude::HOOKS.wire, theirs.path(), &no_env),
            "an empty directory holds nothing to copy"
        );
        std::fs::write(dir.join("SKILL.md"), "their own copy\n").unwrap();
        assert!(would_keep_a_copy(
            &claude::HOOKS.wire,
            theirs.path(),
            &no_env
        ));
    }

    #[test]
    fn install_keeps_a_copy_of_a_file_that_is_somebody_elses_and_uninstall_puts_it_back() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&FILE.wire, home.path(), &no_env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let theirs = "// somebody else's\n";
        std::fs::write(&path, theirs).unwrap();

        let report = install_wire(&FILE.wire, home.path(), &no_env, 7).unwrap();
        let backup = report.backup.expect("their file was copied aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), file_body());

        let report = uninstall_wire(&FILE.wire, home.path(), &no_env, 8).unwrap();
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
        let path = wire_path(&FILE.wire, home.path(), &no_env);

        // Nothing there is nothing to do.
        let report = uninstall_wire(&FILE.wire, home.path(), &no_env, 1).unwrap();
        assert!(!report.changed);

        install_wire(&FILE.wire, home.path(), &no_env, 1).unwrap();
        let report = uninstall_wire(&FILE.wire, home.path(), &no_env, 2).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None);
        assert!(!path.exists());

        // A file that is not amx's is not amx's to remove.
        std::fs::write(&path, "// theirs\n").unwrap();
        let report = uninstall_wire(&FILE.wire, home.path(), &no_env, 3).unwrap();
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
        let dir = wire_path(&claude::HOOKS.wire, home.path(), &no_env);
        assert_eq!(
            wired(&claude::HOOKS.wire, home.path(), &no_env),
            Wired::File {
                present: false,
                current: false
            }
        );

        let report = install_wire(&claude::HOOKS.wire, home.path(), &no_env, 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "nothing was there to keep");
        for (name, body) in files {
            assert_eq!(&std::fs::read_to_string(dir.join(name)).unwrap(), body);
        }
        assert_eq!(
            wired(&claude::HOOKS.wire, home.path(), &no_env),
            Wired::File {
                present: true,
                current: true
            }
        );

        // Written twice is written once.
        let again = install_wire(&claude::HOOKS.wire, home.path(), &no_env, 2).unwrap();
        assert!(!again.changed);
        assert_eq!(again.backup, None);
    }

    #[test]
    fn install_replaces_an_older_amxs_plugin_without_keeping_it() {
        // The manifest is what says the directory is amx's, so a file beside
        // one an older amx wrote is amx's to overwrite. Keeping a copy of
        // every one of those would leave a backup behind at every upgrade.
        let home = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, home.path(), &no_env);
        install_wire(&claude::HOOKS.wire, home.path(), &no_env, 1).unwrap();
        std::fs::write(dir.join("SKILL.md"), "an older amx's skill\n").unwrap();
        assert_eq!(
            wired(&claude::HOOKS.wire, home.path(), &no_env),
            Wired::File {
                present: true,
                current: false
            }
        );

        let report = install_wire(&claude::HOOKS.wire, home.path(), &no_env, 2).unwrap();
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
        let dir = wire_path(&claude::HOOKS.wire, home.path(), &no_env);
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = "---\nname: amx\n---\n\ntheir own copy\n";
        std::fs::write(dir.join("SKILL.md"), theirs).unwrap();

        let report = install_wire(&claude::HOOKS.wire, home.path(), &no_env, 7).unwrap();
        let backup = report.backup.expect("their file was copied aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);
        assert_ne!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            theirs,
            "and amx's own is what stands there now"
        );

        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 8).unwrap();
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
        let dir = wire_path(&claude::HOOKS.wire, home.path(), &no_env);

        // Nothing there is nothing to do.
        assert!(
            !uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 1)
                .unwrap()
                .changed
        );

        install_wire(&claude::HOOKS.wire, home.path(), &no_env, 1).unwrap();
        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 2).unwrap();
        assert!(report.changed);
        assert!(!dir.exists(), "and the directory it emptied goes too");

        // A directory whose manifest is somebody else's is not amx's to empty,
        // however many of these names it carries.
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(dir.join(MANIFEST), "{\"name\": \"theirs\"}\n").unwrap();
        std::fs::write(dir.join("SKILL.md"), "theirs\n").unwrap();
        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 3).unwrap();
        assert!(!report.changed);
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "theirs\n"
        );
    }

    #[test]
    fn install_that_failed_after_the_first_file_still_keeps_their_file_when_run_again() {
        // The manifest is what makes the directory amx's. Written first, a run
        // that died after it would leave a directory the next run took for
        // amx's own, and the person's skill would go under without a copy.
        let home = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, home.path(), &no_env);
        std::fs::create_dir_all(&dir).unwrap();
        let theirs = "---\nname: amx\n---\n\ntheir own copy\n";
        std::fs::write(dir.join("SKILL.md"), theirs).unwrap();
        // A file where the hooks directory goes makes that write fail.
        std::fs::write(dir.join("hooks"), "in the way\n").unwrap();

        install_wire(&claude::HOOKS.wire, home.path(), &no_env, 1)
            .expect_err("the hooks file cannot go in");
        std::fs::remove_file(dir.join("hooks")).unwrap();
        install_wire(&claude::HOOKS.wire, home.path(), &no_env, 2).unwrap();

        let kept = latest_backup(&dir.join("SKILL.md"))
            .unwrap()
            .expect("their file was copied aside");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), theirs);
        uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 3).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            theirs,
            "their file is back where it was"
        );
    }

    #[test]
    fn uninstall_that_failed_midway_removes_what_is_left_when_run_again() {
        // The manifest is what makes the directory amx's to empty. Removed
        // first, a run that died after it would leave the rest of amx's files
        // in a directory no later run would touch.
        let home = TempDir::new().unwrap();
        let dir = wire_path(&claude::HOOKS.wire, home.path(), &no_env);
        install_wire(&claude::HOOKS.wire, home.path(), &no_env, 1).unwrap();
        // A directory with something in it where amx's skill was cannot be
        // removed as a file.
        std::fs::remove_file(dir.join("SKILL.md")).unwrap();
        std::fs::create_dir_all(dir.join("SKILL.md/in-the-way")).unwrap();

        uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 2)
            .expect_err("the skill cannot go");
        std::fs::remove_dir_all(dir.join("SKILL.md")).unwrap();
        std::fs::write(dir.join("SKILL.md"), "amx's skill\n").unwrap();
        let report = uninstall_wire(&claude::HOOKS.wire, home.path(), &no_env, 3).unwrap();

        assert!(report.changed);
        assert!(!dir.exists(), "nothing of amx's is left behind");
    }

    #[test]
    fn install_asks_before_it_writes_anything() {
        // The file is the vendor's, under the person's home, and the sentence
        // names it in full because that is the thing being agreed to.
        let table = claude::VENDOR.hooks.expect("claude reports through hooks");
        let plugin = Path::new("/home/dev").join(table.wire.path());
        assert_eq!(
            wire_path(&table.wire, Path::new("/home/dev"), &no_env),
            plugin
        );

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

    #[test]
    fn trusted_hash_is_the_hash_codex_lists_for_each_of_amxs_groups() {
        // The oracle is codex's own `hooks/list` answer for a hooks file in
        // which amx's four groups sit at index 1 of their events.
        let hooks: Value =
            serde_json::from_str(include_str!("../tests/codex/trust/hooks.json")).unwrap();
        for (event, snake, hash) in [
            (
                "SessionStart",
                "session_start",
                "sha256:4add63b3f2cf92a907d37f2e9b7b5d7972234d4357db8dd35851c32c02603c53",
            ),
            (
                "UserPromptSubmit",
                "user_prompt_submit",
                "sha256:ddebddeb874f57494d55880d6f70adbee7136bb60988e2a9616f5d59ab304738",
            ),
            (
                "PreToolUse",
                "pre_tool_use",
                "sha256:62ca2d9301c902140298733d9e0be308345dda945b627cb0385a231adebdc012",
            ),
            (
                "Stop",
                "stop",
                "sha256:4de4283a553cfcda6ea020eec3ae1d66555d7f3bce089906e6723de26975e6d7",
            ),
        ] {
            assert_eq!(
                trusted_hash(snake, &hooks["hooks"][event][1]),
                hash,
                "{event}"
            );
        }
        // And a recorder group of the person's, one with a command of its own.
        assert_eq!(
            trusted_hash("stop", &hooks["hooks"]["Stop"][0]),
            "sha256:2281b6dd8a3f60dcc689419f83d97a2095e52bafa7c998eccf3f5c66d863ac71"
        );
    }

    /// The oracle's hooks file with amx's groups taken out: seven events, each
    /// holding one recorder group of the person's.
    fn their_hooks() -> String {
        let mut theirs: Value =
            serde_json::from_str(include_str!("../tests/codex/trust/hooks.json")).unwrap();
        for groups in theirs["hooks"].as_object_mut().unwrap().values_mut() {
            groups.as_array_mut().unwrap().truncate(1);
        }
        serde_json::to_string_pretty(&theirs).unwrap() + "\n"
    }

    /// An environment naming `dir` as codex's.
    fn codex_home(dir: &Path) -> impl Fn(&str) -> Option<OsString> {
        let dir = dir.as_os_str().to_owned();
        move |name| (name == "CODEX_HOME").then(|| dir.clone())
    }

    #[test]
    fn the_hooks_directory_is_the_variables_real_path_or_the_one_under_home() {
        let home = TempDir::new().unwrap();
        let real = TempDir::new().unwrap();
        let link = home.path().join("link");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        let env = codex_home(&link);
        assert_eq!(
            wire_path(&HOOKS_WIRE, home.path(), &env),
            real.path().canonicalize().unwrap()
        );
        assert_eq!(
            wire_path(&HOOKS_WIRE, home.path(), &no_env),
            home.path().join(".codex")
        );
        // codex reads an empty CODEX_HOME as none.
        assert_eq!(
            vendor_dir(Some(OsString::new()), ".codex", home.path()),
            home.path().join(".codex")
        );
    }

    #[test]
    fn install_merges_beside_their_groups_and_uninstall_puts_both_files_back() {
        let home = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let env = codex_home(&dir);
        let hooks_path = dir.join(HOOKS_FILE);
        let config_path = dir.join(CONFIG_FILE);
        let theirs = their_hooks();
        let shown = hooks_path.display().to_string();
        let their_config = format!(
            "model = \"gpt-5.5\"  # theirs\n\n[hooks.state]\n\n[hooks.state.\"{shown}:stop:0:0\"]\ntrusted_hash = \"sha256:2281b6dd8a3f60dcc689419f83d97a2095e52bafa7c998eccf3f5c66d863ac71\"\n"
        );
        std::fs::write(&hooks_path, &theirs).unwrap();
        std::fs::write(&config_path, &their_config).unwrap();
        assert!(would_keep_a_copy(&HOOKS_WIRE, home.path(), &env));
        assert_eq!(
            wired(&HOOKS_WIRE, home.path(), &env),
            Wired::File {
                present: false,
                current: false
            }
        );

        let report = install_wire(&HOOKS_WIRE, home.path(), &env, 7).unwrap();
        assert!(report.changed);
        assert!(report.backup.is_some());
        assert!(!home.path().join(".codex").exists(), "the variable wins");

        // Their groups keep their places and amx's land after them, where the
        // oracle's stood, so the trust amx writes is what codex wrote there.
        let merged: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        let oracle: Value =
            serde_json::from_str(include_str!("../tests/codex/trust/hooks.json")).unwrap();
        assert_eq!(merged, oracle);
        let config = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            config.starts_with("model = \"gpt-5.5\"  # theirs\n"),
            "{config}"
        );
        let written: toml::Table = config.parse().unwrap();
        let codex_wrote: toml::Table = include_str!("../tests/codex/trust/config.toml")
            .replace("/tmp/codexmeasure.ohGh/home/hooks.json", &shown)
            .parse()
            .unwrap();
        for event in [
            "session_start",
            "user_prompt_submit",
            "pre_tool_use",
            "stop",
        ] {
            let key = format!("{shown}:{event}:1:0");
            assert_eq!(
                written["hooks"]["state"][&key], codex_wrote["hooks"]["state"][&key],
                "{event}"
            );
            assert!(
                config.contains(&format!("[hooks.state.\"{key}\"]")),
                "a table of its own, not an inline one: {config}"
            );
        }
        assert_eq!(
            wired(&HOOKS_WIRE, home.path(), &env),
            Wired::File {
                present: true,
                current: true
            }
        );
        assert!(!would_keep_a_copy(&HOOKS_WIRE, home.path(), &env));

        // Installed twice is installed once, to the byte.
        let hooks_then = std::fs::read(&hooks_path).unwrap();
        let config_then = std::fs::read(&config_path).unwrap();
        let again = install_wire(&HOOKS_WIRE, home.path(), &env, 8).unwrap();
        assert!(!again.changed);
        assert_eq!(again.backup, None);
        assert_eq!(std::fs::read(&hooks_path).unwrap(), hooks_then);
        assert_eq!(std::fs::read(&config_path).unwrap(), config_then);

        let report = uninstall_wire(&HOOKS_WIRE, home.path(), &env, 9).unwrap();
        assert!(report.changed);
        assert_eq!(std::fs::read_to_string(&hooks_path).unwrap(), theirs);
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), their_config);
    }

    #[test]
    fn uninstall_removes_the_files_amx_made() {
        let home = TempDir::new().unwrap();
        let dir = wire_path(&HOOKS_WIRE, home.path(), &no_env);
        assert!(!would_keep_a_copy(&HOOKS_WIRE, home.path(), &no_env));

        let report = install_wire(&HOOKS_WIRE, home.path(), &no_env, 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None, "nothing was there to keep");
        let config = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(
            config.starts_with("[hooks.state."),
            "no empty table above amx's: {config}"
        );

        let report = uninstall_wire(&HOOKS_WIRE, home.path(), &no_env, 2).unwrap();
        assert!(report.changed);
        assert!(!dir.join(HOOKS_FILE).exists());
        assert!(!dir.join(CONFIG_FILE).exists());

        // And nothing of amx's is nothing to do.
        assert!(
            !uninstall_wire(&HOOKS_WIRE, home.path(), &no_env, 3)
                .unwrap()
                .changed
        );
    }

    #[test]
    fn uninstall_after_they_changed_the_files_takes_out_amxs_part_alone() {
        let home = TempDir::new().unwrap();
        let dir = wire_path(&HOOKS_WIRE, home.path(), &no_env);
        let hooks_path = dir.join(HOOKS_FILE);
        let config_path = dir.join(CONFIG_FILE);
        install_wire(&HOOKS_WIRE, home.path(), &no_env, 1).unwrap();

        // A group of their own after amx's, and a line of config.
        let mut hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        let theirs = json!({"hooks": [{"type": "command", "command": "notify-send done"}]});
        hooks["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(theirs.clone());
        std::fs::write(&hooks_path, serde_json::to_string(&hooks).unwrap()).unwrap();
        let config = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(&config_path, format!("model = \"gpt-5.5\"\n{config}")).unwrap();

        uninstall_wire(&HOOKS_WIRE, home.path(), &no_env, 2).unwrap();

        let left: Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert_eq!(left, json!({"hooks": {"Stop": [theirs]}}));
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "model = \"gpt-5.5\"\n"
        );
    }

    #[test]
    fn a_hook_trusted_under_another_hash_or_not_at_all_is_not_wired() {
        let home = TempDir::new().unwrap();
        let dir = wire_path(&HOOKS_WIRE, home.path(), &no_env);
        let config_path = dir.join(CONFIG_FILE);
        install_wire(&HOOKS_WIRE, home.path(), &no_env, 1).unwrap();
        let config = std::fs::read_to_string(&config_path).unwrap();
        let stale = Wired::File {
            present: true,
            current: false,
        };

        let hash = trusted_hash(
            "stop",
            &json!({"hooks": [{"type": "command", "command": "amx _hook"}]}),
        );
        std::fs::write(&config_path, config.replace(&hash, "sha256:00")).unwrap();
        assert_eq!(wired(&HOOKS_WIRE, home.path(), &no_env), stale);

        std::fs::remove_file(&config_path).unwrap();
        assert_eq!(wired(&HOOKS_WIRE, home.path(), &no_env), stale);

        // And setup puts it right without a copy of amx's own edit.
        let report = install_wire(&HOOKS_WIRE, home.path(), &no_env, 2).unwrap();
        assert!(report.changed);
        assert_eq!(report.backup, None);
        assert_eq!(
            wired(&HOOKS_WIRE, home.path(), &no_env),
            Wired::File {
                present: true,
                current: true
            }
        );

        std::fs::write(dir.join(HOOKS_FILE), "{\"hooks\": {}}\n").unwrap();
        assert_eq!(
            wired(&HOOKS_WIRE, home.path(), &no_env),
            Wired::File {
                present: false,
                current: false
            }
        );
    }

    #[test]
    fn the_consent_line_names_both_files_a_hooks_wire_edits() {
        let dir = Path::new("/home/dev/.codex");
        let asked = consent_line(&HOOKS_WIRE, dir, true);
        assert!(asked.contains("/home/dev/.codex/hooks.json"), "{asked}");
        assert!(asked.contains("/home/dev/.codex/config.toml"), "{asked}");
        assert!(asked.contains("copy"), "{asked}");
        assert!(!consent_line(&HOOKS_WIRE, dir, false).contains("copy"));
    }

    /// A placed wire of the tests' own, shaped like opencode's.
    const PLACED: Wire = Wire::Placed {
        dir_env: "VENDOR_CONFIG_DIR",
        dir: ".config/vendor",
        path: "plugins/amx/tui.js",
        body: "// installed by amx\nexport default {};\n",
    };

    /// An environment naming `dir` as the placed wire's.
    fn placed_dir(dir: &Path) -> impl Fn(&str) -> Option<OsString> {
        let dir = dir.as_os_str().to_owned();
        move |name| (name == "VENDOR_CONFIG_DIR").then(|| dir.clone())
    }

    #[test]
    fn a_placed_wire_goes_under_its_variables_dir_or_the_one_under_home() {
        let home = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        assert_eq!(
            wire_path(&PLACED, home.path(), &placed_dir(&dir)),
            dir.join("plugins/amx/tui.js")
        );
        assert_eq!(
            wire_path(&PLACED, home.path(), &no_env),
            home.path().join(".config/vendor/plugins/amx/tui.js")
        );
    }

    #[test]
    fn a_placed_wire_installs_drifts_and_uninstalls_under_its_variables_dir() {
        let home = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let env = placed_dir(&dir);
        let path = dir.join("plugins/amx/tui.js");
        let absent = Wired::File {
            present: false,
            current: false,
        };
        assert_eq!(wired(&PLACED, home.path(), &env), absent);

        let report = install_wire(&PLACED, home.path(), &env, 1).unwrap();
        assert!(report.changed);
        assert_eq!(report.path, path);
        assert_eq!(report.backup, None);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "// installed by amx\nexport default {};\n"
        );
        assert!(
            !home.path().join(".config").exists(),
            "nothing is written under the home the variable replaced"
        );
        assert_eq!(
            wired(&PLACED, home.path(), &env),
            Wired::File {
                present: true,
                current: true
            }
        );
        assert_eq!(
            wired(&PLACED, home.path(), &no_env),
            absent,
            "read where the variable points, not under the home"
        );

        // An older amx's file is a file that drifted.
        std::fs::write(&path, "// installed by amx\n// an older one\n").unwrap();
        assert_eq!(
            wired(&PLACED, home.path(), &env),
            Wired::File {
                present: true,
                current: false
            }
        );
        assert!(!would_keep_a_copy(&PLACED, home.path(), &env));

        let report = uninstall_wire(&PLACED, home.path(), &env, 2).unwrap();
        assert!(report.changed);
        assert!(!path.exists());
        assert_eq!(wired(&PLACED, home.path(), &env), absent);
    }

    #[test]
    fn a_placed_wire_keeps_a_copy_of_somebody_elses_file_and_puts_it_back() {
        let home = TempDir::new().unwrap();
        let path = wire_path(&PLACED, home.path(), &no_env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "their plugin\n").unwrap();
        assert!(would_keep_a_copy(&PLACED, home.path(), &no_env));
        assert_eq!(
            consent_line(&PLACED, &path, true),
            format!(
                "amx will write its plugin to {}, keeping a copy of the file as it is now.",
                path.display()
            )
        );

        let report = install_wire(&PLACED, home.path(), &no_env, 1).unwrap();
        assert!(report.backup.is_some(), "somebody else's file is kept");
        uninstall_wire(&PLACED, home.path(), &no_env, 2).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "their plugin\n");
    }
}
