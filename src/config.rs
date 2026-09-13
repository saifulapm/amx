//! `~/.config/amx/config.toml` — twenty-two keys and a table per harness — with a
//! project's own `<project>/.amx/config.toml` laid over it.
//!
//! Config is a convenience, never a gate: a file that cannot be read or
//! parsed degrades to the defaults with a warning on stderr, because losing
//! an agent to a stray comma is a worse outcome than running with defaults.
//! A project's file is a layer rather than a replacement, so the same holds one
//! file at a time: the keys of a file amx cannot use are all it costs.

use crate::registry;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every key the file may carry, beside the harness tables. Anything else is
/// warned about and ignored.
pub const KNOWN_KEYS: [&str; 22] = [
    "agent",
    "max_agents",
    "max_total",
    "worktrees",
    "notifications",
    "trust",
    "model",
    "permission",
    "effort",
    "summary_command",
    "theme",
    "park_after",
    "copy",
    "link",
    "setup",
    "base",
    "diff",
    "on_waiting",
    "on_idle",
    "on_done",
    "on_failed",
    "on_stopped",
];

/// How far a notice about a transition goes.
///
/// The key was a bool before it was these four words, and both are still
/// written: `true` is the desktop and `false` is nothing, so no file anybody
/// has already written has changed its meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Off,
    Desktop,
    Terminal,
    Both,
}

impl Delivery {
    /// Post through the desktop's own notifier.
    pub fn desktop(&self) -> bool {
        matches!(self, Self::Desktop | Self::Both)
    }

    /// Write the notice to the terminals the person is sitting at.
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Terminal | Self::Both)
    }

    /// Whether a notice is delivered at all, by either road. What the callers
    /// that have a notice to post ask before they go to the cost of one.
    pub fn tells(&self) -> bool {
        self.desktop() || self.terminal()
    }
}

impl<'de> Deserialize<'de> for Delivery {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Wanted;

        impl serde::de::Visitor<'_> for Wanted {
            type Value = Delivery;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(r#"a bool, or "off", "desktop", "terminal" or "both""#)
            }

            fn visit_bool<E: serde::de::Error>(self, yes: bool) -> Result<Delivery, E> {
                Ok(if yes {
                    Delivery::Desktop
                } else {
                    Delivery::Off
                })
            }

            fn visit_str<E: serde::de::Error>(self, word: &str) -> Result<Delivery, E> {
                match word {
                    "off" => Ok(Delivery::Off),
                    "desktop" => Ok(Delivery::Desktop),
                    "terminal" => Ok(Delivery::Terminal),
                    "both" => Ok(Delivery::Both),
                    // A word amx has no delivery for is an error, the way a
                    // key of the wrong type is: which of the four somebody
                    // meant is not worth a guess.
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(word), &self)),
                }
            }
        }

        deserializer.deserialize_any(Wanted)
    }
}

/// What one harness says about itself, in a table of its own named after the
/// program it runs.
///
/// Two lists and a table, all empty until somebody writes one, and a harness
/// that sets none of them is a harness the file has said nothing about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    /// The models this harness is the one to run. A model nobody lists here is
    /// left to whatever the registry entry says the harness takes.
    pub models: Vec<String>,
    /// What every agent this harness runs carries on its argv, however it was
    /// picked.
    pub args: Vec<String>,
    /// What every agent this harness runs has in its environment, laid over
    /// the one the spawn was typed in.
    ///
    /// Two accounts of one vendor, or a proxy in front of it, are settled by a
    /// variable rather than by a flag, and without this they are settled by a
    /// wrapper script somebody has to keep on the PATH. Values are literal,
    /// with `~` at the front of one spelled out: there is no shell between
    /// this file and the pane to do it, and a path is what somebody writing
    /// one means.
    ///
    /// A sub-table, `[claude.env]`, so it reads the way the harness table it
    /// belongs to reads.
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The vendor command a new agent runs.
    pub agent: String,
    /// How many live agents `new` will allow before it refuses.
    pub max_agents: usize,
    /// How many live agents there may be on the machine, over every project at
    /// once. Absent is no ceiling of its own: a machine is as busy as the
    /// projects on it ask between them.
    pub max_total: Option<usize>,
    /// Give new agents their own git worktree.
    pub worktrees: bool,
    /// Where a notice about the transitions worth interrupting for goes: the
    /// desktop's notifier, the terminals the person is sitting at, both or
    /// neither.
    pub notifications: Delivery,
    /// Answer the vendor's folder-trust screen for the agents amx starts, so
    /// that one begins on its task instead of on a question nobody has to
    /// think about. Off until the person says so, the way the hooks stand
    /// behind doctor --fix's yes.
    ///
    /// One key, two answers, because the vendors answer it two ways and the
    /// difference is worth knowing. For claude it writes an entry in the
    /// vendor's own trust store, for the worktree amx cut and nothing else,
    /// and the entry outlives the agent: the store is the person's file, and
    /// this is the consent that write stands behind. For pi it puts
    /// `--approve` on the argv of the pane, which trusts that folder for that
    /// one run and writes nothing anywhere.
    ///
    /// What both say to the vendor is the same, and it is what the key is
    /// really about: load what this repository keeps in it — its settings, its
    /// extensions, its skills, its prompts — without asking first.
    pub trust: bool,
    /// Where the model dial starts. Absent is the vendor's own choice, which
    /// amx says by passing no flag, so there is no value here that means
    /// "default" and an `Option` is the honest shape.
    pub model: Option<String>,
    /// Where the permission dial starts, under the same rule.
    pub permission: Option<String>,
    /// Where the effort dial starts, under the same rule.
    pub effort: Option<String>,
    /// What writes the one line a finished turn is worth. Absent is nothing
    /// run and nothing spent, and a row that says what the agent said rather
    /// than what somebody would have written about it.
    pub summary_command: Option<String>,
    /// Which palette the view paints in: a theme amx ships, a file in
    /// `~/.config/amx/themes`, or a path to one. A name rather than an
    /// `Option`, because there is a palette amx paints in when nobody has
    /// chosen and it has a name of its own.
    pub theme: String,
    /// How many seconds an idle agent nobody is attached to keeps its pane.
    ///
    /// A vendor at its prompt holds a couple of hundred megabytes to do
    /// nothing with, and a wall of them is the machine's memory spent on turns
    /// that ended hours ago. So the pane goes and the record stays, and the
    /// next enter, attach or resume starts the agent again.
    ///
    /// A number rather than an `Option`, because zero has an answer of its own
    /// here — never let a pane go — and an hour is what amx does where nobody
    /// has said otherwise.
    pub park_after: u64,
    /// Files copied from the repository root into a tree amx cuts, before the
    /// agent's pane starts.
    ///
    /// A checkout git has just made is missing whatever git is right not to
    /// carry — a `.env`, a key, a local override — and without them an agent's
    /// first turn goes on a failure nobody learns anything from. Exact paths,
    /// relative to the repository root: what belongs here is the two or three
    /// files somebody can name, and a glob is how a secret nobody meant to
    /// copy ends up in a tree.
    pub copy: Vec<String>,
    /// Directories in a tree that point at the repository's own, under the
    /// same rule. A `node_modules` an agent shares is an install it does not
    /// spend its first turn on.
    pub link: Vec<String>,
    /// What runs in a fresh tree before the pane starts, in order, each
    /// through `sh -c`. The first that fails refuses the spawn and the tree is
    /// removed, because a tree an agent cannot work in is worse than no tree.
    pub setup: Vec<String>,
    /// What a tree is cut from: anything git resolves to a commit. Absent is
    /// HEAD where `new` was typed, which is what a checkout somebody is
    /// standing in already means, so there is no value here that says it and
    /// an `Option` is the honest shape.
    pub base: Option<String>,
    /// What reads a patch when there is a terminal to read it on: a shell
    /// command handed the unified diff on stdin, `delta --paging=always` or
    /// another of its kind.
    ///
    /// Absent is git's own patch, which is also what a pipe gets whatever this
    /// says, so `amx diff fix-login-a1b | head` reads the same either way.
    pub diff: Option<String>,
    /// What runs when an agent stops on a question somebody has to answer.
    ///
    /// One of five moments, each its own flat key rather than a table under
    /// one name, so a project file lays its own over the person's a moment at
    /// a time. Each is a shell command, run detached and never waited for in
    /// the agent's tree, or where the agent runs when it has none, with what
    /// moved the agent on its stdin.
    pub on_waiting: Option<String>,
    /// What runs when an agent's turn ends and it goes back to its prompt.
    pub on_idle: Option<String>,
    /// What runs when an agent's command finishes.
    pub on_done: Option<String>,
    /// What runs when an agent's command exits non-zero.
    pub on_failed: Option<String>,
    /// What runs when somebody stops an agent.
    pub on_stopped: Option<String>,
    /// What each harness the file names says about itself, keyed by the
    /// program that harness runs.
    ///
    /// Which names there are is the registry's answer rather than a list here:
    /// a harness amx has an entry for may have a table of its own, and a table
    /// naming anything else is warned about and dropped. Read off the tables
    /// of the file rather than by serde, so there is no key of this name to
    /// write — and serde is told so, or a `[harnesses]` table somebody wrote
    /// would land here before the tables are read.
    #[serde(skip)]
    pub harnesses: BTreeMap<String, HarnessConfig>,
}

impl Config {
    /// What the file says about the harness `program` runs, which is the empty
    /// table where it says nothing.
    pub fn harness(&self, program: &str) -> HarnessConfig {
        self.harnesses.get(program).cloned().unwrap_or_default()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: "claude".to_string(),
            max_agents: 5,
            max_total: None,
            worktrees: true,
            notifications: Delivery::Desktop,
            trust: false,
            model: None,
            permission: None,
            effort: None,
            summary_command: None,
            theme: "default".to_string(),
            park_after: 3600,
            copy: Vec::new(),
            link: Vec::new(),
            setup: Vec::new(),
            base: None,
            diff: None,
            on_waiting: None,
            on_idle: None,
            on_done: None,
            on_failed: None,
            on_stopped: None,
            harnesses: BTreeMap::new(),
        }
    }
}

/// Parse config text, returning the config and any warnings about it.
///
/// An unknown key is a warning: config files outlive the versions that wrote
/// them. A key with the wrong *type* is an error, because guessing what
/// `max_agents = "five"` meant is worse than saying so.
///
/// A dial the vendor would not take is a warning too, and the same fallback:
/// the file names its own agent, so which launch values are legal is a
/// question the text can answer on its own.
pub fn parse(text: &str) -> Result<(Config, Vec<String>)> {
    let table: toml::Table = text.parse().context("not valid TOML")?;
    let mut warnings = unknown_keys(&table);
    let mut config: Config = table.clone().try_into()?;
    config.harnesses = harness_tables(&table)?;
    warnings.extend(check_dials(&mut config));
    Ok((config, warnings))
}

/// The keys of a file amx has never heard of, one warning each.
///
/// A table is a harness's own, so what amx has heard of there is whatever the
/// registry has an entry for, and it is named the way the file writes it.
fn unknown_keys(table: &toml::Table) -> Vec<String> {
    table
        .iter()
        .filter(|(key, _)| !KNOWN_KEYS.contains(&key.as_str()))
        .filter_map(|(key, value)| {
            if !value.is_table() {
                Some(format!("ignoring unknown key `{key}`"))
            } else if registry::entry(key).is_none() {
                Some(format!(
                    "ignoring [{key}]: amx runs no harness called {key}"
                ))
            } else {
                None
            }
        })
        .collect()
}

/// What the file's harness tables say, keyed by the program each names.
///
/// A table naming no harness is [`unknown_keys`]' business and is passed over
/// here. A table setting nothing is not kept, so a file that names a harness
/// without saying anything about it says nothing at all — which is what lets
/// the shipped file show every table there is with the lists commented out.
fn harness_tables(table: &toml::Table) -> Result<BTreeMap<String, HarnessConfig>> {
    let mut harnesses = BTreeMap::new();
    for (name, value) in table.iter().filter(|(_, value)| value.is_table()) {
        if registry::entry(name).is_none() {
            continue;
        }
        let mut harness: HarnessConfig = value
            .clone()
            .try_into()
            .with_context(|| format!("in [{name}]"))?;
        for value in harness.env.values_mut() {
            *value = spelled_out(value);
        }
        if harness != HarnessConfig::default() {
            harnesses.insert(name.clone(), harness);
        }
    }
    Ok(harnesses)
}

/// An env value with `~` at the front of it replaced by the home directory,
/// which is the one thing about a value that is not literal.
///
/// The front alone: a `~` anywhere else in a value is a character somebody
/// wrote, and `~work` names no home amx can spell. A machine with no home
/// directory leaves the value as it was typed, because the value somebody
/// wrote is a better guess than half of it.
fn spelled_out(value: &str) -> String {
    let Some(home) = std::env::home_dir() else {
        return value.to_string();
    };
    let path = match value {
        "~" => home,
        _ => match value.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => return value.to_string(),
        },
    };
    path.to_string_lossy().into_owned()
}

/// Drop any dial the configured agent would not take, saying which and why.
///
/// Dropped means back to absent, which is the only way to say "leave it to
/// the vendor". An agent amx has no entry for declares no dials at all, so
/// every dial set for it goes the same way.
fn check_dials(config: &mut Config) -> Vec<String> {
    let agent = registry::program(&config.agent);
    let entry = registry::entry(&config.agent);

    [
        ("model", entry.and_then(|e| e.model), &mut config.model),
        (
            "permission",
            entry.and_then(|e| e.permission),
            &mut config.permission,
        ),
        ("effort", entry.and_then(|e| e.effort), &mut config.effort),
    ]
    .into_iter()
    .filter_map(|(key, dial, value)| {
        let set = value.as_deref()?;
        let warning = match dial {
            Some(spec) if registry::accepts(&spec, set) => return None,
            // Only a closed dial ever refuses, so its cycle is the whole list
            // of what the vendor takes and is worth printing in full.
            Some(spec) => format!(
                "ignoring {key} = {set:?}: {agent} takes {}",
                spec.cycle.join(", ")
            ),
            None => format!("ignoring {key} = {set:?}: amx knows no {key} dial for {agent}"),
        };
        *value = None;
        Some(warning)
    })
    .collect()
}

/// Read `path`, degrading to defaults with a warning rather than failing.
pub fn load_from(path: &Path) -> (Config, Vec<String>) {
    let text = match read(path) {
        Ok(Some(text)) => text,
        Ok(None) => return (Config::default(), Vec::new()),
        Err(e) => return (Config::default(), vec![format!("using defaults: {e:#}")]),
    };

    match parse(&text) {
        Ok((config, warnings)) => (
            config,
            warnings
                .into_iter()
                .map(|w| format!("{}: {w}", path.display()))
                .collect(),
        ),
        Err(e) => (
            Config::default(),
            vec![format!("ignoring {}, using defaults: {e}", path.display())],
        ),
    }
}

/// The config as a reader reaches it, read once for the life of the process.
///
/// Every other caller is handed the config `main` read at startup. A reader is
/// not: `ls`, `status`, the view and the card all reach [`crate::derive`]
/// without one, and threading a config through four surfaces to reach a single
/// key would change more than the key is worth.
///
/// Read once because a reading is taken every second, and a file that nobody
/// is editing does not want opening that often. A file somebody has just
/// edited is picked up by the next amx they run, which is how every other key
/// here behaves already. Warnings go unsaid here on purpose: `main` prints
/// them from its own read, and saying them twice would put a parse error on
/// the screen once a second.
pub fn current() -> &'static Config {
    static CURRENT: std::sync::OnceLock<Config> = std::sync::OnceLock::new();
    CURRENT.get_or_init(|| load().0)
}

/// The config the work in one project is read under, read once per project for
/// the life of the process.
///
/// [`current`]'s reason, a project at a time. A reader takes a reading every
/// second, and the finished agents in it belong to a handful of projects
/// between them: opening two files per agent per second to answer one key is
/// more of the disk than the key is worth. What a file somebody has just
/// edited says is picked up by the next amx they run, which is how the person's
/// own file behaves already.
///
/// Keyed by the project as the filesystem spells it, so one project reached by
/// two paths is one entry rather than two readings of one file. It is the
/// project that is asked for rather than the tree an agent happens to run in —
/// [`crate::spawn::project_dir`] is what answers that — so a repository and
/// every worktree amx cut from it share the entry, which is the same law the
/// file itself is kept under.
///
/// Warnings go unsaid here, as they are in [`current`] and in the verbs that
/// spawn into a project: this is one key being asked about on a reading nobody
/// asked for, and a file read once could only say what is wrong with it once,
/// at whatever moment the first reader happened to look.
pub fn for_project(project: &Path) -> &'static Config {
    static READ: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, &'static Config>>,
    > = std::sync::OnceLock::new();
    let read = READ.get_or_init(Default::default);

    let key = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    if let Some(config) = read.lock().ok().and_then(|read| read.get(&key).copied()) {
        return config;
    }

    // Kept for the life of the process rather than behind the lock, because
    // what a reader is handed it holds for as long as it is drawing with it,
    // and there is one of these per project somebody has run an agent in. Two
    // readers arriving at once make one read apiece and agree on which of the
    // two the map keeps: they read the same file and got the same answer.
    let config: &'static Config = Box::leak(Box::new(for_dir(&key).0));
    match read.lock() {
        Ok(mut read) => read.entry(key).or_insert(config),
        // A map nothing can reach is a memory, not an answer.
        Err(_) => config,
    }
}

/// The config as amx runs with it, with warnings for the caller to print.
pub fn load() -> (Config, Vec<String>) {
    match crate::paths::config_file() {
        Ok(path) => load_from(&path),
        Err(e) => (Config::default(), vec![format!("using defaults: {e}")]),
    }
}

/// The config for work in `dir`: the person's file with that project's own
/// file over it.
///
/// A project says the two or three keys the work there wants — the agent it is
/// written for, the cap the machine can afford it — and every other key is
/// still whatever the person chose. Which file is the project's is
/// [`crate::paths::project_config`]'s question, and it is the repository's
/// rather than any one tree of it.
pub fn for_dir(dir: &Path) -> (Config, Vec<String>) {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    match crate::paths::config_file() {
        Ok(path) => files.push(path),
        Err(e) => warnings.push(format!("using defaults: {e}")),
    }
    files.extend(crate::paths::project_config(dir));

    let (config, said) = layered(&files);
    warnings.extend(said);
    (config, warnings)
}

/// What the project in `dir` says about one string key, or nothing where it
/// says nothing.
///
/// The whole config laid up for one key, where the caller has neither and
/// wants neither: a hook holding the person's config already asks that, and
/// what is left to ask is whether the project has changed its mind. So no
/// file but the project's own is read, and what is read of it is one key.
///
/// Proved a config first, the way [`keys_of`] proves it: one key is never
/// taken out of a file every other key of which was thrown away.
pub fn project_key(dir: &Path, key: &str) -> Option<String> {
    let path = crate::paths::project_config(dir)?;
    let keys = usable(&path).ok()??;
    Some(keys.get(key)?.as_str()?.to_string())
}

/// Read the files in order, every key one sets replacing that key from the
/// files before it.
///
/// Key by key rather than file by file: a project file holding one line has
/// changed its mind about one key, not thrown away everything the person set.
///
/// The dials are settled once, at the end, because which dials a vendor takes
/// turns on the `agent` key and either file may be the one that named it.
fn layered(files: &[PathBuf]) -> (Config, Vec<String>) {
    let mut keys = toml::Table::new();
    let mut warnings = Vec::new();
    for path in files {
        let (set, said) = keys_of(path);
        keys.extend(set);
        warnings.extend(said);
    }

    // Every file that got this far parses as a config on its own, and a key
    // laid over another is that key entire — a harness table included, which
    // is why a project naming one has said what that harness is, args and all.
    // So what the files make between them parses too.
    let mut config: Config = keys.clone().try_into().unwrap_or_default();
    config.harnesses = harness_tables(&keys).unwrap_or_default();
    warnings.extend(check_dials(&mut config));
    (config, warnings)
}

/// The keys `path` sets, with anything worth saying about it naming it.
///
/// A file amx cannot use is no keys and a warning: it is one layer of a
/// config, and the layers under it stand whatever it says. Which is also why
/// it is proved a config here rather than after the layering — a key of the
/// wrong type belongs to the file that holds it, and that is the file to name.
fn keys_of(path: &Path) -> (toml::Table, Vec<String>) {
    match usable(path) {
        Ok(Some(keys)) => {
            let said = unknown_keys(&keys)
                .into_iter()
                .map(|warning| format!("{}: {warning}", path.display()))
                .collect();
            (keys, said)
        }
        Ok(None) => (toml::Table::new(), Vec::new()),
        Err(e) => (
            toml::Table::new(),
            vec![format!("ignoring {}: {}", path.display(), e.root_cause())],
        ),
    }
}

/// The keys `path` sets, proved to describe a config on their own, or `None`
/// when there is no such file.
fn usable(path: &Path) -> Result<Option<toml::Table>> {
    let Some(text) = read(path)? else {
        return Ok(None);
    };
    let keys: toml::Table = text.parse()?;
    let _: Config = keys.clone().try_into()?;
    harness_tables(&keys)?;
    Ok(Some(keys))
}

/// Read a file, distinguishing "not there" (fine) from "unreadable" (worth
/// saying out loud).
fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let c = Config::default();
        assert_eq!(c.agent, "claude");
        assert_eq!(c.max_agents, 5);
        // No ceiling over the projects until somebody puts one there.
        assert_eq!(c.max_total, None);
        assert!(c.worktrees);
        // A notice goes to the desktop until somebody asks for the terminal
        // too, which is what the key meant when it was a bool alone.
        assert_eq!(c.notifications, Delivery::Desktop);
        assert!(!c.trust, "the vendor's own file wants a yes before a write");
        // Absent, not the word default: a dial nobody has turned is one amx
        // passes no flag for, and there is no value that says that.
        assert_eq!(c.model, None);
        assert_eq!(c.permission, None);
        assert_eq!(c.effort, None);
        // Nothing is run at the end of a turn until somebody says what to run.
        assert_eq!(c.summary_command, None);
        // A theme, unlike a dial, has a value that means the default one, so
        // there is a name here rather than an absence.
        assert_eq!(c.theme, "default");
        // An hour of sitting idle with nobody attached, and the pane goes.
        assert_eq!(c.park_after, 3600);
        // A fresh tree is furnished with nothing until somebody says what, and
        // is cut from HEAD until somebody names a base.
        assert!(c.copy.is_empty());
        assert!(c.link.is_empty());
        assert!(c.setup.is_empty());
        assert_eq!(c.base, None);
        // A patch is git's own until somebody names something to read it with.
        assert_eq!(c.diff, None);
        // Nothing is run at a moment until somebody says what to run there.
        assert_eq!(c.on_waiting, None);
        assert_eq!(c.on_idle, None);
        assert_eq!(c.on_done, None);
        assert_eq!(c.on_failed, None);
        assert_eq!(c.on_stopped, None);
        // No harness says anything about itself until a table of its own does.
        assert!(c.harnesses.is_empty());
    }

    #[test]
    fn the_shipped_file_is_the_defaults_written_out() {
        // assets/config.toml is the file to copy and edit: every key, each
        // explained, at the value amx uses when the key is left out — so a
        // copy nobody edits runs as no file at all would. A key with no value
        // that means "left out" is there as a comment, and a key missing from
        // the file altogether is a key nobody copying it will learn of.
        let shipped = include_str!("../assets/config.toml");
        let (c, w) = parse(shipped).unwrap();
        assert_eq!(c, Config::default());
        assert!(w.is_empty(), "{w:?}");
        for key in KNOWN_KEYS {
            let named = shipped.lines().any(|line| {
                line.trim_start()
                    .trim_start_matches("# ")
                    .starts_with(&format!("{key} ="))
            });
            assert!(named, "{key} is not in assets/config.toml");
        }
        // A harness is one entry in the registry and one table here, so a new
        // entry nobody gave a table is a table nobody copying this will learn
        // of. The lists are comments: an empty table is no table at all.
        for entry in registry::entries() {
            let named = shipped
                .lines()
                .any(|line| line == format!("[{}]", entry.name));
            assert!(named, "[{}] is not in assets/config.toml", entry.name);
        }
    }

    #[test]
    fn the_theme_the_defaults_name_is_one_amx_ships() {
        // The one config value that has to mean something to another module.
        // A default naming a theme nobody has would warn on every start.
        assert!(crate::theme::shipped(&Config::default().theme).is_some());
    }

    #[test]
    fn an_empty_file_is_the_defaults_and_says_nothing() {
        let (c, warnings) = parse("").unwrap();
        assert_eq!(c, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn every_key_overrides_its_own_default_and_leaves_the_others_alone() {
        let (c, w) = parse("agent = \"my-agent --flag\"").unwrap();
        assert_eq!(c.agent, "my-agent --flag");
        assert_eq!(c.max_agents, Config::default().max_agents);
        assert!(w.is_empty());

        let (c, _) = parse("max_agents = 12").unwrap();
        assert_eq!(c.max_agents, 12);
        assert_eq!(c.agent, Config::default().agent);

        let (c, _) = parse("max_total = 12").unwrap();
        assert_eq!(c.max_total, Some(12));
        assert_eq!(c.max_agents, Config::default().max_agents);

        let (c, _) = parse("worktrees = false").unwrap();
        assert!(!c.worktrees);
        assert_eq!(c.notifications, Delivery::Desktop);

        let (c, _) = parse("notifications = false").unwrap();
        assert_eq!(c.notifications, Delivery::Off);
        assert!(c.worktrees);

        let (c, _) = parse("trust = true").unwrap();
        assert!(c.trust);
        assert!(c.worktrees);

        let (c, w) = parse("model = \"opus\"").unwrap();
        assert_eq!(c.model.as_deref(), Some("opus"));
        assert_eq!(c.permission, None);
        assert_eq!(c.effort, None);
        assert!(w.is_empty(), "{w:?}");

        let (c, _) = parse("permission = \"plan\"").unwrap();
        assert_eq!(c.permission.as_deref(), Some("plan"));
        assert_eq!(c.model, None);

        let (c, _) = parse("effort = \"high\"").unwrap();
        assert_eq!(c.effort.as_deref(), Some("high"));
        assert_eq!(c.permission, None);

        let (c, w) = parse("summary_command = \"claude -p 'in eight words'\"").unwrap();
        assert_eq!(
            c.summary_command.as_deref(),
            Some("claude -p 'in eight words'")
        );
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("theme = \"terminal\"").unwrap();
        assert_eq!(c.theme, "terminal");
        assert_eq!(c.agent, Config::default().agent);
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("park_after = 900").unwrap();
        assert_eq!(c.park_after, 900);
        assert_eq!(c.theme, Config::default().theme);
        assert!(w.is_empty(), "{w:?}");

        // Zero is a value of its own — never let a pane go — rather than an
        // absence that falls back to the hour.
        let (c, w) = parse("park_after = 0").unwrap();
        assert_eq!(c.park_after, 0);
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("copy = [\".env\", \"config/local.toml\"]").unwrap();
        assert_eq!(c.copy, [".env", "config/local.toml"]);
        assert!(c.link.is_empty());
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("link = [\"node_modules\"]").unwrap();
        assert_eq!(c.link, ["node_modules"]);
        assert!(c.copy.is_empty());
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("setup = [\"pnpm install\", \"pnpm build\"]").unwrap();
        assert_eq!(c.setup, ["pnpm install", "pnpm build"]);
        assert!(c.link.is_empty());
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("base = \"main\"").unwrap();
        assert_eq!(c.base.as_deref(), Some("main"));
        assert!(c.setup.is_empty());
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("diff = \"delta --paging=always\"").unwrap();
        assert_eq!(c.diff.as_deref(), Some("delta --paging=always"));
        assert_eq!(c.base, None);
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("on_waiting = \"say-so\"").unwrap();
        assert_eq!(c.on_waiting.as_deref(), Some("say-so"));
        assert_eq!(c.on_idle, None);
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("on_stopped = \"log-it\"").unwrap();
        assert_eq!(c.on_stopped.as_deref(), Some("log-it"));
        assert_eq!(c.on_waiting, None);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn notifications_takes_a_bool_or_one_of_the_four_words() {
        // The key was a bool before the words were there, so a file somebody
        // wrote then still means what it meant when they wrote it.
        assert_eq!(
            parse("notifications = true").unwrap().0.notifications,
            Delivery::Desktop
        );
        assert_eq!(
            parse("notifications = false").unwrap().0.notifications,
            Delivery::Off
        );

        for (word, delivery) in [
            ("off", Delivery::Off),
            ("desktop", Delivery::Desktop),
            ("terminal", Delivery::Terminal),
            ("both", Delivery::Both),
        ] {
            let (c, w) = parse(&format!("notifications = {word:?}")).unwrap();
            assert_eq!(c.notifications, delivery, "{word}");
            assert!(w.is_empty(), "{w:?}");
        }

        // A word amx has no delivery for is an error, the way a key of the
        // wrong type is: which of the four was meant is not worth a guess.
        assert!(parse("notifications = \"loud\"").is_err());
        assert!(parse("notifications = 3").is_err());
    }

    #[test]
    fn a_delivery_says_which_of_the_two_it_reaches() {
        for (delivery, desktop, terminal) in [
            (Delivery::Off, false, false),
            (Delivery::Desktop, true, false),
            (Delivery::Terminal, false, true),
            (Delivery::Both, true, true),
        ] {
            assert_eq!(delivery.desktop(), desktop, "{delivery:?}");
            assert_eq!(delivery.terminal(), terminal, "{delivery:?}");
            assert_eq!(delivery.tells(), desktop || terminal, "{delivery:?}");
        }
    }

    #[test]
    fn every_key_there_is_parses_beside_all_the_others() {
        let (c, w) = parse(
            r#"
                agent = "claude --dangerously-skip-permissions"
                max_agents = 3
                max_total = 8
                worktrees = false
                notifications = "both"
                trust = true
                model = "opus"
                permission = "plan"
                effort = "xhigh"
                summary_command = "summarise"
                theme = "terminal"
                park_after = 900
                copy = [".env"]
                link = ["node_modules"]
                setup = ["pnpm install"]
                base = "main"
                diff = "delta --paging=always"
                on_waiting = "say waiting"
                on_idle = "say idle"
                on_done = "say done"
                on_failed = "say failed"
                on_stopped = "say stopped"
            "#,
        )
        .unwrap();
        assert_eq!(c.agent, "claude --dangerously-skip-permissions");
        assert_eq!(c.max_agents, 3);
        assert_eq!(c.max_total, Some(8));
        assert!(!c.worktrees);
        assert_eq!(c.notifications, Delivery::Both);
        assert!(c.trust);
        assert_eq!(c.model.as_deref(), Some("opus"));
        assert_eq!(c.permission.as_deref(), Some("plan"));
        assert_eq!(c.effort.as_deref(), Some("xhigh"));
        assert_eq!(c.summary_command.as_deref(), Some("summarise"));
        assert_eq!(c.theme, "terminal");
        assert_eq!(c.park_after, 900);
        assert_eq!(c.copy, [".env"]);
        assert_eq!(c.link, ["node_modules"]);
        assert_eq!(c.setup, ["pnpm install"]);
        assert_eq!(c.base.as_deref(), Some("main"));
        assert_eq!(c.diff.as_deref(), Some("delta --paging=always"));
        assert_eq!(c.on_waiting.as_deref(), Some("say waiting"));
        assert_eq!(c.on_idle.as_deref(), Some("say idle"));
        assert_eq!(c.on_done.as_deref(), Some("say done"));
        assert_eq!(c.on_failed.as_deref(), Some("say failed"));
        assert_eq!(c.on_stopped.as_deref(), Some("say stopped"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            KNOWN_KEYS.len(),
            22,
            "a key this file does not name is a key nothing here proves"
        );
    }

    #[test]
    fn an_unknown_key_is_named_in_a_warning_and_the_rest_still_applies() {
        let (c, warnings) = parse("max_agents = 2\nwardrobe = true\n").unwrap();
        assert_eq!(c.max_agents, 2);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("wardrobe"), "{warnings:?}");
    }

    #[test]
    fn a_table_named_after_a_harness_is_kept_under_that_name() {
        // Which names there are is the registry's answer, so the test asks it
        // rather than spelling a harness out here, as the code does.
        for entry in registry::entries() {
            let (c, w) = parse(&format!(
                "[{}]\nmodels = [\"opus\"]\nargs = [\"--add-dir\", \"/tmp\"]\n",
                entry.name
            ))
            .unwrap();
            assert!(w.is_empty(), "{w:?}");
            assert_eq!(c.harnesses.len(), 1, "{:?}", c.harnesses);
            let harness = c.harness(entry.name);
            assert_eq!(harness.models, ["opus"]);
            assert_eq!(harness.args, ["--add-dir", "/tmp"]);
        }
    }

    #[test]
    fn a_harness_no_table_names_is_the_empty_table() {
        let harness = Config::default().harness("some-other-agent");
        assert_eq!(harness, HarnessConfig::default());
        assert!(harness.models.is_empty());
        assert!(harness.args.is_empty());
        assert!(harness.env.is_empty());
    }

    #[test]
    fn a_harness_tables_env_is_a_sub_table_of_its_own() {
        for entry in registry::entries() {
            let (c, w) = parse(&format!(
                "[{0}]\nargs = [\"--add-dir\", \"/tmp\"]\n\n[{0}.env]\nSOME_PROXY = \"http://localhost:8080\"\nSOME_TOKEN = \"abc\"\n",
                entry.name
            ))
            .unwrap();
            assert!(w.is_empty(), "{w:?}");
            let harness = c.harness(entry.name);
            assert_eq!(harness.args, ["--add-dir", "/tmp"]);
            assert_eq!(harness.env.len(), 2, "{:?}", harness.env);
            assert_eq!(
                harness.env.get("SOME_PROXY").unwrap(),
                "http://localhost:8080"
            );
            assert_eq!(harness.env.get("SOME_TOKEN").unwrap(), "abc");
        }
    }

    #[test]
    fn a_table_that_sets_only_env_is_kept() {
        // A table says something the moment it says anything, and an env
        // table on its own is a person putting one harness somewhere else.
        let name = registry::entries()[0].name;
        let (c, w) = parse(&format!("[{name}.env]\nSOME_CONFIG_DIR = \"/srv/work\"\n")).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.harnesses.len(), 1, "{:?}", c.harnesses);
        assert_eq!(
            c.harness(name).env.get("SOME_CONFIG_DIR").unwrap(),
            "/srv/work"
        );
    }

    #[test]
    fn a_tilde_at_the_front_of_an_env_value_is_spelled_out() {
        // The one thing that is not literal about a value: a path somebody
        // writes as `~/...` is the path they mean, and the vendor being
        // launched reads it as a directory rather than as a shell would.
        let name = registry::entries()[0].name;
        let (c, w) = parse(&format!(
            "[{name}.env]\nUNDER_HOME = \"~/.claude-work\"\nHOME_ITSELF = \"~\"\nNOT_A_PATH = \"~work/x\"\nMID = \"/srv/~/x\"\n",
        ))
        .unwrap();
        assert!(w.is_empty(), "{w:?}");
        let env = c.harness(name).env;
        match std::env::home_dir() {
            Some(home) => {
                assert_eq!(
                    env.get("UNDER_HOME").unwrap(),
                    &home.join(".claude-work").to_string_lossy().into_owned()
                );
                assert_eq!(
                    env.get("HOME_ITSELF").unwrap(),
                    &home.to_string_lossy().into_owned()
                );
            }
            // No home to spell it with leaves the value as it was typed.
            None => {
                assert_eq!(env.get("UNDER_HOME").unwrap(), "~/.claude-work");
                assert_eq!(env.get("HOME_ITSELF").unwrap(), "~");
            }
        }
        assert_eq!(env.get("NOT_A_PATH").unwrap(), "~work/x", "the front of it");
        assert_eq!(env.get("MID").unwrap(), "/srv/~/x", "and only the front");
    }

    #[test]
    fn an_env_value_that_is_not_a_string_is_an_error_not_a_guess() {
        let name = registry::entries()[0].name;
        assert!(parse(&format!("[{name}.env]\nSOME_PORT = 8080\n")).is_err());
        assert!(parse(&format!("[{name}]\nenv = \"SOME_PORT=8080\"\n")).is_err());
    }

    #[test]
    fn a_table_naming_no_harness_is_warned_about_by_name_and_the_rest_applies() {
        let (c, w) = parse("max_agents = 2\n\n[wardrobe]\nmodels = [\"opus\"]\n").unwrap();
        assert_eq!(c.max_agents, 2);
        assert!(c.harnesses.is_empty(), "{:?}", c.harnesses);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("ignoring [wardrobe]"), "{w:?}");
    }

    #[test]
    fn a_table_that_sets_neither_list_is_not_kept() {
        // Which is what lets the shipped file name every harness there is with
        // both lists commented out and still read as no file at all.
        let name = registry::entries()[0].name;
        let (c, w) = parse(&format!("[{name}]\n")).unwrap();
        assert_eq!(c, Config::default());
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_list_of_the_wrong_type_is_an_error_not_a_guess() {
        let name = registry::entries()[0].name;
        assert!(parse(&format!("[{name}]\nmodels = 3\n")).is_err());
        assert!(parse(&format!("[{name}]\nargs = \"--add-dir\"\n")).is_err());
    }

    #[test]
    fn a_dial_the_registry_refuses_warns_by_name_and_falls_back_to_the_default() {
        // The vendor would refuse this mode itself, at spawn, in a pane that
        // may already have scrolled. Saying so at the file it was read from is
        // the same answer somewhere a person can act on it.
        let (c, w) = parse("permission = \"sudo\"").unwrap();
        assert_eq!(c.permission, None, "falls back to no flag at all");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("permission"), "{w:?}");
        assert!(w[0].contains("sudo"), "{w:?}");
        assert!(w[0].contains("bypassPermissions"), "names what it takes");

        let (c, w) = parse("effort = \"hard\"").unwrap();
        assert_eq!(c.effort, None);
        assert!(w[0].contains("xhigh"), "{w:?}");
    }

    #[test]
    fn an_open_dial_takes_a_value_the_registry_never_lists() {
        // model's cycle spells out the aliases alone, and the dial is open, so
        // a full model name is carried through without a word.
        let (c, w) = parse("model = \"claude-fable-5\"").unwrap();
        assert_eq!(c.model.as_deref(), Some("claude-fable-5"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_agent_with_no_registry_entry_ignores_every_dial_it_was_given() {
        // No entry, no dials, so a dial key set for it is not obeyed. Saying
        // so per key is what separates ignored from obeyed wrongly.
        let (c, w) = parse(
            r#"
                agent = "some-other-agent"
                model = "opus"
                permission = "plan"
                effort = "high"
            "#,
        )
        .unwrap();
        assert_eq!((c.model, c.permission, c.effort), (None, None, None));
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w.iter().all(|w| w.contains("some-other-agent")), "{w:?}");
    }

    #[test]
    fn the_registry_is_asked_about_the_program_the_agent_command_runs() {
        // `agent` holds a command line, and the flags on it do not change
        // which vendor is being launched.
        let (c, w) = parse(
            r#"
                agent = "claude --add-dir /tmp"
                model = "opus"
            "#,
        )
        .unwrap();
        assert_eq!(c.model.as_deref(), Some("opus"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_key_of_the_wrong_type_is_an_error_not_a_guess() {
        assert!(parse("max_agents = \"five\"").is_err());
        assert!(parse("worktrees = \"yes\"").is_err());
        assert!(parse("theme = 3").is_err());
        // A path is one of a list, however few the list holds.
        assert!(parse("copy = \".env\"").is_err());
        assert!(parse("base = 3").is_err());
    }

    #[test]
    fn malformed_toml_is_an_error() {
        assert!(parse("agent = ").is_err());
    }

    #[test]
    fn a_missing_file_is_the_defaults_without_a_word() {
        let dir = TempDir::new().unwrap();
        let (c, warnings) = load_from(&dir.path().join("nothing-here.toml"));
        assert_eq!(c, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn an_unparseable_file_falls_back_to_defaults_and_names_itself() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "max_agents = \"five\"").unwrap();

        let (c, warnings) = load_from(&path);
        assert_eq!(c, Config::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("config.toml"), "{warnings:?}");
    }

    #[test]
    fn a_readable_file_is_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "agent = \"other\"\nmax_agents = 1\n").unwrap();

        let (c, warnings) = load_from(&path);
        assert_eq!(c.agent, "other");
        assert_eq!(c.max_agents, 1);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn reading_tells_a_missing_file_from_an_unreadable_one() {
        let dir = TempDir::new().unwrap();
        assert!(read(&dir.path().join("absent")).unwrap().is_none());
        // A directory is present but is not a file amx can read.
        assert!(read(dir.path()).is_err());
    }

    /// A file with `text` in it, at `name` under `dir`.
    fn wrote(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_project_file_replaces_the_keys_it_sets_and_leaves_the_rest_alone() {
        let dir = TempDir::new().unwrap();
        let person = wrote(
            dir.path(),
            "person.toml",
            "agent = \"other\"\nmax_agents = 9\ntheme = \"terminal\"\n",
        );
        let project = wrote(dir.path(), "project.toml", "max_agents = 2\n");

        let (c, w) = layered(&[person, project]);
        assert_eq!(c.max_agents, 2, "the key the project sets is the project's");
        assert_eq!(c.agent, "other", "and the rest is still the person's");
        assert_eq!(c.theme, "terminal");
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_project_files_table_replaces_the_persons_whole_table() {
        // A table is one key, and a key laid over another is that key entire:
        // a project that names a harness has said what that harness is here,
        // args and all, rather than edited the list of models under it.
        let dir = TempDir::new().unwrap();
        let name = registry::entries()[0].name;
        let person = wrote(
            dir.path(),
            "person.toml",
            &format!("[{name}]\nmodels = [\"opus\"]\nargs = [\"--add-dir\", \"/tmp\"]\n"),
        );
        let project = wrote(
            dir.path(),
            "project.toml",
            &format!("[{name}]\nmodels = [\"sonnet\"]\n"),
        );

        let (c, w) = layered(&[person, project]);
        assert_eq!(c.harness(name).models, ["sonnet"]);
        assert!(
            c.harness(name).args.is_empty(),
            "the table, not a key of it"
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_project_file_lays_each_of_the_worktree_keys_over_the_persons() {
        // What a tree is furnished with and cut from is a fact about the
        // repository rather than about whoever is working in it, so these four
        // are the keys a project file is most likely to be written for.
        let dir = TempDir::new().unwrap();
        let person = wrote(
            dir.path(),
            "person.toml",
            "copy = [\".env\"]\nlink = [\"node_modules\"]\nsetup = [\"make\"]\nbase = \"main\"\n",
        );
        let project = wrote(
            dir.path(),
            "project.toml",
            "copy = [\".env.local\"]\nlink = [\"vendor\"]\nsetup = [\"pnpm install\"]\nbase = \"develop\"\n",
        );

        let (c, w) = layered(&[person.clone(), project]);
        assert_eq!(c.copy, [".env.local"]);
        assert_eq!(c.link, ["vendor"]);
        assert_eq!(c.setup, ["pnpm install"]);
        assert_eq!(c.base.as_deref(), Some("develop"));
        assert!(w.is_empty(), "{w:?}");

        // A list laid over another is that list entire, and the keys beside it
        // are still the person's.
        let one = wrote(dir.path(), "one-key.toml", "setup = [\"cargo build\"]\n");
        let (c, w) = layered(&[person, one]);
        assert_eq!(c.setup, ["cargo build"]);
        assert_eq!(c.copy, [".env"]);
        assert_eq!(c.link, ["node_modules"]);
        assert_eq!(c.base.as_deref(), Some("main"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_project_file_lays_each_moment_key_over_the_persons_one_at_a_time() {
        // Five flat keys rather than one table under `on`: a project that says
        // what to run when an agent stops has not thrown away what the person
        // set for the four moments beside it.
        let dir = TempDir::new().unwrap();
        let person = wrote(
            dir.path(),
            "person.toml",
            "on_waiting = \"a\"\non_idle = \"b\"\non_done = \"c\"\non_failed = \"d\"\non_stopped = \"e\"\n",
        );
        let project = wrote(dir.path(), "project.toml", "on_stopped = \"mine\"\n");

        let (c, w) = layered(&[person, project]);
        assert_eq!(c.on_stopped.as_deref(), Some("mine"));
        assert_eq!(c.on_waiting.as_deref(), Some("a"));
        assert_eq!(c.on_idle.as_deref(), Some("b"));
        assert_eq!(c.on_done.as_deref(), Some("c"));
        assert_eq!(c.on_failed.as_deref(), Some("d"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_project_with_no_file_of_its_own_is_the_persons_config_unchanged() {
        let dir = TempDir::new().unwrap();
        let person = wrote(dir.path(), "person.toml", "max_agents = 9\n");

        let (c, w) = layered(&[person, dir.path().join("nothing-here.toml")]);
        assert_eq!(c.max_agents, 9);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_unknown_key_in_a_project_file_names_that_file_and_the_rest_applies() {
        let dir = TempDir::new().unwrap();
        let person = wrote(dir.path(), "person.toml", "max_agents = 9\n");
        let project = wrote(
            dir.path(),
            "project.toml",
            "wardrobe = true\ntheme = \"terminal\"\n",
        );

        let (c, w) = layered(&[person, project.clone()]);
        assert_eq!(c.theme, "terminal", "the key beside it still applies");
        assert_eq!(c.max_agents, 9);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("wardrobe"), "{w:?}");
        assert!(w[0].contains(&project.display().to_string()), "{w:?}");
    }

    #[test]
    fn a_project_file_amx_cannot_use_names_that_file_and_the_person_stands() {
        let dir = TempDir::new().unwrap();
        let person = wrote(
            dir.path(),
            "person.toml",
            "max_agents = 9\ntheme = \"terminal\"\n",
        );

        // A key of the wrong type is the file's own business: the file goes,
        // and the one under it is untouched.
        let project = wrote(dir.path(), "project.toml", "max_agents = \"two\"\n");
        let (c, w) = layered(&[person.clone(), project.clone()]);
        assert_eq!(c.max_agents, 9);
        assert_eq!(c.theme, "terminal");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains(&project.display().to_string()), "{w:?}");

        // And a file that cannot be read at all says which one it was.
        let unreadable = dir.path().join("a-directory");
        std::fs::create_dir(&unreadable).unwrap();
        let (c, w) = layered(&[person, unreadable]);
        assert_eq!(c.max_agents, 9);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("a-directory"), "{w:?}");
    }

    #[test]
    fn the_dials_are_asked_of_the_agent_the_layering_settles_on() {
        // The project names the agent and the person named the model, so which
        // dials the vendor takes is a question neither file answers alone.
        let dir = TempDir::new().unwrap();
        let person = wrote(dir.path(), "person.toml", "model = \"opus\"\n");
        let project = wrote(dir.path(), "project.toml", "agent = \"some-other-agent\"\n");

        let (c, w) = layered(&[person, project]);
        assert_eq!(c.model, None);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("some-other-agent"), "{w:?}");
    }

    /// git as the tests run it: none of the developer's own configuration, and
    /// an identity of its own.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "amx tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "amx tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// The config file a project keeps, with one key in it.
    fn project_file(root: &Path) -> PathBuf {
        std::fs::create_dir_all(root.join(".amx")).unwrap();
        wrote(&root.join(".amx"), "config.toml", "max_agents = 42\n")
    }

    /// A repository with one commit and a config file of its own. The file is
    /// never committed: `.amx/` is kept out of the repository's status.
    fn a_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "before\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "first"]);
        project_file(dir.path());
        dir
    }

    /// What `dir` reads its project config from, as the file it names.
    fn config_of(dir: &Path) -> PathBuf {
        let found = crate::paths::project_config(dir).expect("a project config");
        std::fs::canonicalize(&found).unwrap_or(found)
    }

    fn same_file(got: PathBuf, expected: &Path) {
        assert_eq!(
            got,
            std::fs::canonicalize(expected).unwrap(),
            "{}",
            got.display()
        );
    }

    #[test]
    fn a_checkout_reads_the_project_config_at_its_own_root() {
        let repo = a_project();
        let expected = repo.path().join(".amx/config.toml");
        same_file(config_of(repo.path()), &expected);

        let deep = repo.path().join("src/deep");
        std::fs::create_dir_all(&deep).unwrap();
        same_file(config_of(&deep), &expected);
    }

    #[test]
    fn a_tree_amx_cut_reads_the_project_config_of_the_repository_behind_it() {
        // Every agent on one repository reads one file, wherever amx put the
        // tree it works in.
        let repo = a_project();
        let tree = crate::worktree::create(repo.path(), "fix-login-a1b", None).unwrap();
        same_file(config_of(&tree.path), &repo.path().join(".amx/config.toml"));
    }

    #[test]
    fn another_linked_worktree_reads_the_project_config_of_its_repository() {
        // Not a tree amx cut and not a project of its own: it is a tree of the
        // repository, wherever somebody put the directory.
        let repo = a_project();
        let elsewhere = TempDir::new().unwrap();
        let tree = elsewhere.path().join("review");
        git(
            repo.path(),
            &["worktree", "add", "-b", "review", &tree.to_string_lossy()],
        );

        same_file(config_of(&tree), &repo.path().join(".amx/config.toml"));
    }

    #[test]
    fn a_directory_outside_git_is_the_whole_of_its_own_project() {
        let dir = TempDir::new().unwrap();
        let expected = project_file(dir.path());
        same_file(config_of(dir.path()), &expected);
    }

    #[test]
    fn the_config_for_a_directory_is_the_project_file_of_the_project_it_is_in() {
        // The whole way through, from a directory to the key that file sets.
        let repo = a_project();
        assert_eq!(for_dir(repo.path()).0.max_agents, 42);
    }

    #[test]
    fn the_projects_own_file_answers_one_key_without_reading_the_persons() {
        // A moment key is asked for where no config has been read and none is
        // wanted, so it is this file and no other: a directory with nothing
        // else in it answers the same on any machine.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        wrote(
            &dir.path().join(".amx"),
            "config.toml",
            "on_stopped = \"log-it\"\nmax_agents = 42\n",
        );

        assert_eq!(
            project_key(dir.path(), "on_stopped").as_deref(),
            Some("log-it")
        );
        // A key the file leaves out, and one it sets to something that is not
        // a string, are both nothing to say.
        assert_eq!(project_key(dir.path(), "on_waiting"), None);
        assert_eq!(project_key(dir.path(), "max_agents"), None);
    }

    #[test]
    fn a_project_file_amx_cannot_use_answers_no_key_at_all() {
        // The same proving the layering does, so one key is never read out of
        // a file every other key of which was thrown away.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        wrote(
            &dir.path().join(".amx"),
            "config.toml",
            "max_agents = \"two\"\non_done = \"run-it\"\n",
        );
        assert_eq!(project_key(dir.path(), "on_done"), None);

        // And a project keeping no file of its own says nothing.
        let bare = TempDir::new().unwrap();
        assert_eq!(project_key(bare.path(), "on_done"), None);
    }

    #[test]
    fn a_projects_config_is_read_once_and_every_reading_after_is_that_one() {
        // A reader asks this of every finished agent it draws, every second it
        // is open, so what it costs is one read per project and no more.
        let repo = a_project();
        assert_eq!(for_project(repo.path()).max_agents, 42);

        std::fs::write(repo.path().join(".amx/config.toml"), "max_agents = 7\n").unwrap();
        assert_eq!(
            for_project(repo.path()).max_agents,
            42,
            "an edited file is what the next amx reads, not this one"
        );
    }
}
