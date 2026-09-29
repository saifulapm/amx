//! The config file, `~/.config/amx/config.toml`, with an allowed project's
//! `<project>/.amx/config.toml` laid over it key by key.
//!
//! Config never blocks a verb: a file that cannot be read or parsed is ignored
//! with a warning, leaving the defaults or the layers under it.

use crate::registry;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Every top-level key the file may set, besides `[keys]` and the harness
/// tables. Anything else is warned about and ignored.
pub const KNOWN_KEYS: [&str; 25] = [
    "agent",
    "max_agents",
    "max_total",
    "max_children",
    "subagent_depth",
    "subagents_may_escalate",
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

/// The `[keys]` table of view key bindings, which a project file merges entry
/// by entry instead of replacing.
const BOUND_KEYS: &str = "keys";

/// Where notices about agent transitions are delivered.
///
/// A bool is still accepted from older files: `true` is `Desktop` and `false`
/// is `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Off,
    Desktop,
    Terminal,
    Both,
}

impl Delivery {
    /// Whether notices go to the desktop notifier.
    pub fn desktop(&self) -> bool {
        matches!(self, Self::Desktop | Self::Both)
    }

    /// Whether notices are written to the person's terminals.
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Terminal | Self::Both)
    }

    /// Whether notices are delivered at all.
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
                    // An unknown word is an error, like a key of the wrong type.
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(word), &self)),
                }
            }
        }

        deserializer.deserialize_any(Wanted)
    }
}

/// A harness table such as `[claude]`, named after the program it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    /// Models that select this harness when named. A model no table lists is
    /// looked up in the vendor table.
    pub models: Vec<String>,
    /// Arguments added to every agent this harness runs.
    pub args: Vec<String>,
    /// Variables set for every agent this harness runs, written as
    /// `[<harness>.env]` and laid over the spawning environment.
    ///
    /// Values are literal except a leading `~`, which is expanded because no
    /// shell runs between this file and the pane.
    pub env: BTreeMap<String, String>,
}

/// The settings amx runs with, merged from the config files.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The agent command a new agent runs, arguments included.
    pub agent: String,
    /// How many live agents one project may have before `new` refuses.
    pub max_agents: usize,
    /// How many live agents the machine may have across all projects. Unset
    /// means no machine-wide cap.
    pub max_total: Option<usize>,
    /// Give new agents their own git worktree.
    pub worktrees: bool,
    /// How deep a chain of `amx sub` children may go: 0 allows none, 1 allows
    /// children, 2 also grandchildren.
    ///
    /// Checked before anything is claimed. The default of 2 lets a worker that
    /// was itself started as a child start one helper of its own.
    pub subagent_depth: usize,
    /// How many unfinished children one parent may have at once; 0 is no limit.
    ///
    /// Children do not count toward `max_agents` or `max_total`.
    pub max_children: usize,
    /// Whether `amx sub` accepts `--permission`.
    ///
    /// Children inherit the parent's model and effort, but widening a permission
    /// is the person's decision, so this is off by default.
    pub subagents_may_escalate: bool,
    /// Where notices about transitions that need attention are delivered.
    pub notifications: Delivery,
    /// Answer the vendor's folder-trust screen for agents amx starts. Off by
    /// default.
    ///
    /// For claude this writes an entry for the linked worktree into claude's own
    /// trust store, which outlives the agent. For pi it adds `--approve` to the
    /// pane's argv, trusting the folder for that run only. Either way the vendor
    /// then loads the repository's settings, extensions, skills and prompts
    /// without asking.
    pub trust: bool,
    /// The default model. Unset leaves the choice to the vendor: amx passes no
    /// flag.
    pub model: Option<String>,
    /// The default permission mode. Unset passes no flag.
    pub permission: Option<String>,
    /// The default reasoning effort. Unset passes no flag.
    pub effort: Option<String>,
    /// A shell command that writes a one-line summary of a finished turn. Unset
    /// runs nothing, and the row shows what the agent said.
    pub summary_command: Option<String>,
    /// The view's palette: a shipped theme, a file in `~/.config/amx/themes`, or
    /// a path to one.
    ///
    /// The default, [`crate::theme::AUTO`], picks a shipped theme to suit the
    /// terminal's background. Any other name is used as given.
    pub theme: String,
    /// Seconds an idle agent with nobody attached keeps its pane; 0 means never
    /// close it.
    ///
    /// An idle vendor still holds a few hundred megabytes. The record stays, and
    /// the next enter, attach or resume starts the agent again.
    pub park_after: u64,
    /// Files such as `.env` copied from the repository root into a new
    /// worktree before the agent starts.
    ///
    /// Exact paths relative to the repository root. No globs, so a secret is
    /// never copied by accident.
    pub copy: Vec<String>,
    /// Directories in a new worktree symlinked to the repository's own, such as
    /// `node_modules`. Exact paths, as for `copy`.
    pub link: Vec<String>,
    /// Commands run in order through `sh -c` in a new worktree before the agent
    /// starts. The first failure refuses the spawn and removes the worktree.
    pub setup: Vec<String>,
    /// The ref new worktrees are cut from. Unset uses HEAD where `new` runs.
    pub base: Option<String>,
    /// A shell command `amx diff` pipes the patch to on a terminal, such as
    /// `delta --paging=always`.
    ///
    /// Unset prints git's own patch, which is also what a pipe always gets.
    pub diff: Option<String>,
    /// A command run when an agent stops on a question.
    ///
    /// The five `on_*` keys are flat so a project file can set one without
    /// replacing the rest. Each runs detached through the shell, in the agent's
    /// worktree or directory, with the event that triggered it on stdin.
    pub on_waiting: Option<String>,
    /// A command run when an agent's turn ends and it returns to its prompt.
    pub on_idle: Option<String>,
    /// A command run when an agent's command finishes.
    pub on_done: Option<String>,
    /// A command run when an agent's command exits non-zero.
    pub on_failed: Option<String>,
    /// A command run when somebody stops an agent.
    pub on_stopped: Option<String>,
    /// View key bindings: a key spelling mapped to a shell command run on the
    /// agent under the cursor.
    ///
    /// A project file merges its bindings into the person's entry by entry.
    pub keys: BTreeMap<String, String>,
    /// The harness tables the file sets, keyed by program name.
    ///
    /// Filled from the tables that name a known vendor rather than by serde;
    /// `serde(skip)` keeps a literal `[harnesses]` table out.
    #[serde(skip)]
    pub harnesses: BTreeMap<String, HarnessConfig>,
}

impl Config {
    /// The table for `program`, or an empty one when the file has none.
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
            subagent_depth: 2,
            max_children: 8,
            subagents_may_escalate: false,
            notifications: Delivery::Desktop,
            trust: false,
            model: None,
            permission: None,
            effort: None,
            summary_command: None,
            theme: crate::theme::AUTO.to_string(),
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
            keys: BTreeMap::new(),
            harnesses: BTreeMap::new(),
        }
    }
}

/// Parse config text, returning the config and any warnings about it.
///
/// An unknown key, or a dial value the configured agent does not take, is a
/// warning and is ignored. A key of the wrong type is an error.
pub fn parse(text: &str) -> Result<(Config, Vec<String>)> {
    let table: toml::Table = text.parse().context("not valid TOML")?;
    let mut warnings = unknown_keys(&table);
    let mut config: Config = table.clone().try_into()?;
    config.harnesses = harness_tables(&table)?;
    warnings.extend(check_dials(&mut config));
    Ok((config, warnings))
}

/// One warning per unknown key. A table is known when it is `[keys]` or
/// names a vendor in the table.
fn unknown_keys(table: &toml::Table) -> Vec<String> {
    table
        .iter()
        .filter(|(key, _)| !KNOWN_KEYS.contains(&key.as_str()))
        .filter_map(|(key, value)| {
            if !value.is_table() {
                Some(format!("ignoring unknown key `{key}`"))
            } else if key == BOUND_KEYS {
                None
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

/// The file's harness tables, keyed by program name.
///
/// A table naming no vendor is skipped (see [`unknown_keys`]). An empty table
/// is dropped, so the shipped file can list every harness with its entries
/// commented out.
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

/// An env value with a leading `~` or `~/` expanded to the home directory.
///
/// A `~` anywhere else, or `~user`, is left as written, and so is every value
/// when there is no home directory.
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

/// Unset every dial value the configured agent does not accept, with a
/// warning for each.
///
/// An agent with no vendor entry has no dials, so every dial set for it is
/// dropped.
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
            // Only a closed dial refuses, so its cycle is the full list of
            // accepted values.
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

/// Read `path`, falling back to the defaults with a warning instead of
/// failing.
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

/// The person's config, read once per process, for code such as
/// [`crate::derive`] that is not handed the config `main` loaded.
///
/// An edit is picked up by the next amx process. Warnings are dropped because
/// `main` already printed them.
pub fn current() -> &'static Config {
    static CURRENT: std::sync::OnceLock<Config> = std::sync::OnceLock::new();
    CURRENT.get_or_init(|| load().0)
}

/// The config for work in `project`, read once per project per process.
///
/// The view asks this for every finished agent on every refresh, so each
/// project's files are read only the first time; an edit is picked up by the
/// next amx process. Keyed by canonical path, so two spellings of a project
/// share an entry. Callers pass the project rather than an agent's worktree
/// (see [`crate::spawn::project_dir`]). Warnings are dropped.
pub fn for_project(project: &Path) -> &'static Config {
    for_project_in(project, &crate::paths::state_root().unwrap_or_default())
}

/// [`for_project`] with the state directory that holds project consent.
fn for_project_in(project: &Path, root: &Path) -> &'static Config {
    static READ: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, &'static Config>>,
    > = std::sync::OnceLock::new();
    let read = READ.get_or_init(Default::default);
    let known = |path: &Path| read.lock().ok().and_then(|read| read.get(path).copied());

    // The path as given first: canonicalizing costs a syscall per component
    // on every call.
    if let Some(config) = known(project) {
        return config;
    }
    let key = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());

    // Leaked so callers can hold a `&'static` while drawing: one config per
    // project for the life of the process. Two racing readers each read the
    // same file, and the map keeps whichever lands first.
    let config = match known(&key) {
        Some(config) => config,
        None => Box::leak(Box::new(for_dir_in(&key, root).0)),
    };
    match read.lock() {
        Ok(mut read) => {
            let config = *read.entry(key).or_insert(config);
            read.insert(project.to_path_buf(), config);
            config
        }
        // A poisoned lock: answer without caching.
        Err(_) => config,
    }
}

/// The person's config, with warnings for the caller to print.
pub fn load() -> (Config, Vec<String>) {
    match crate::paths::config_file() {
        Ok(path) => load_from(&path),
        Err(e) => (Config::default(), vec![format!("using defaults: {e}")]),
    }
}

/// The config for work in `dir`: the person's file with the project's file
/// laid over it key by key.
///
/// The project file is the repository's (see [`crate::paths::project_config`])
/// and counts only while it is allowed as it stands (see [`crate::consent`]).
/// Even then it never sets [`NEVER_FROM_A_PROJECT`] or an unsafe env
/// variable.
pub fn for_dir(dir: &Path) -> (Config, Vec<String>) {
    for_dir_in(dir, &crate::paths::state_root().unwrap_or_default())
}

/// [`for_dir`] with the state directory that holds project consent.
pub fn for_dir_in(dir: &Path, root: &Path) -> (Config, Vec<String>) {
    let mut layers = Vec::new();
    let mut warnings = Vec::new();
    match crate::paths::config_file() {
        Ok(path) => layers.push(keys_of(&path)),
        Err(e) => warnings.push(format!("using defaults: {e}")),
    }
    if let Some(file) = crate::paths::project_config(dir).filter(|file| file.is_file()) {
        match crate::consent::allowed_in(root, &file) {
            true => {
                let (mut keys, mut said) = keys_of(&file);
                said.extend(only_what_a_project_may_set(&file, &mut keys));
                layers.push((keys, said));
            }
            false => warnings.push(not_allowed(&file)),
        }
    }

    let (config, said) = layered_tables(layers);
    warnings.extend(said);
    (config, warnings)
}

/// The warning for a project file nobody has allowed.
fn not_allowed(file: &Path) -> String {
    let project = file
        .parent()
        .and_then(Path::parent)
        .unwrap_or(file)
        .display();
    format!(
        "{} is not allowed: run `amx allow` in {project} to use it",
        file.display()
    )
}

/// Keys a project file may never set, even when allowed.
///
/// Each one lowers what the vendor asks before acting or writes to its trust
/// store, and a file that arrived with a clone must not decide that.
const NEVER_FROM_A_PROJECT: [&str; 3] = ["permission", "trust", "subagents_may_escalate"];

/// Whether a project's `[<harness>.env]` may set `name`.
///
/// Refuses variables that change what runs rather than how it is configured:
/// the search path, home and shell, a vendor's config location, dynamic
/// linker settings, and amx's own variables.
fn a_project_may_set_env(name: &str) -> bool {
    !matches!(
        name,
        "PATH"
            | "HOME"
            | "SHELL"
            | "CLAUDE_CONFIG_DIR"
            | "CODEX_HOME"
            | "OPENCODE_CONFIG_DIR"
            | "OPENCODE_CONFIG"
            | "OPENCODE_CONFIG_CONTENT"
    ) && !["LD_", "DYLD_", "AMX_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Remove the keys and env variables a project may not set, with a warning
/// for each.
fn only_what_a_project_may_set(file: &Path, keys: &mut toml::Table) -> Vec<String> {
    let mut said = Vec::new();
    for name in NEVER_FROM_A_PROJECT {
        if keys.remove(name).is_some() {
            said.push(format!(
                "{}: ignoring `{name}`: it is yours to set, not a project's",
                file.display()
            ));
        }
    }
    for (harness, value) in keys.iter_mut() {
        let Some(env) = value
            .as_table_mut()
            .and_then(|table| table.get_mut("env"))
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        let refused: Vec<String> = env
            .keys()
            .filter(|name| !a_project_may_set_env(name))
            .cloned()
            .collect();
        for name in refused {
            env.remove(&name);
            said.push(format!(
                "{}: ignoring `{harness}.env.{name}`: it is yours to set, not a project's",
                file.display()
            ));
        }
    }
    said
}

/// One string key from the project file for `dir`, without reading the
/// person's file.
///
/// For a caller that already holds the person's config and only asks whether
/// the project overrides one key. `None` when the file is not allowed, does
/// not validate as a config (see [`keys_of`]), lacks the key, or the key is
/// one a project may never set. `root` is the state directory that holds
/// consent.
pub fn project_key_in(dir: &Path, key: &str, root: &Path) -> Option<String> {
    if NEVER_FROM_A_PROJECT.contains(&key) {
        return None;
    }
    let path = crate::paths::project_config(dir)?;
    if !crate::consent::allowed_in(root, &path) {
        return None;
    }
    let keys = usable(&path).ok()??;
    Some(keys.get(key)?.as_str()?.to_string())
}

/// Read `files` and lay them over each other in order (see
/// [`layered_tables`]).
#[cfg(test)]
fn layered(files: &[PathBuf]) -> (Config, Vec<String>) {
    layered_tables(files.iter().map(|path| keys_of(path)).collect())
}

/// Lay layers already read over each other in order, key by key.
///
/// A later layer's key replaces the earlier one whole, except `[keys]`, which
/// merges entry by entry. Dials are checked once at the end, because any
/// layer may set `agent`.
fn layered_tables(layers: Vec<(toml::Table, Vec<String>)>) -> (Config, Vec<String>) {
    let mut keys = toml::Table::new();
    let mut warnings = Vec::new();
    for (set, said) in layers {
        for (name, value) in set {
            let laid = match (keys.remove(&name), value) {
                (Some(toml::Value::Table(mut bound)), toml::Value::Table(over))
                    if name == BOUND_KEYS =>
                {
                    bound.extend(over);
                    toml::Value::Table(bound)
                }
                (_, value) => value,
            };
            keys.insert(name, laid);
        }
        warnings.extend(said);
    }

    // Each layer validated as a config on its own and keys replace whole, so
    // the merged table parses too.
    let mut config: Config = keys.clone().try_into().unwrap_or_default();
    config.harnesses = harness_tables(&keys).unwrap_or_default();
    warnings.extend(check_dials(&mut config));
    (config, warnings)
}

/// The keys `path` sets, with warnings that name the file.
///
/// A file that cannot be used contributes no keys and one warning. It is
/// validated as a whole config here, before layering, so a key of the wrong
/// type is blamed on the file that holds it.
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

/// The keys `path` sets, validated as a complete config, or `None` when the
/// file does not exist.
fn usable(path: &Path) -> Result<Option<toml::Table>> {
    let Some(text) = read(path)? else {
        return Ok(None);
    };
    let keys: toml::Table = text.parse()?;
    let _: Config = keys.clone().try_into()?;
    harness_tables(&keys)?;
    Ok(Some(keys))
}

/// A file's text, `None` when it does not exist, or an error when it cannot
/// be read.
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
        assert_eq!(c.max_total, None);
        assert!(c.worktrees);
        assert_eq!(c.notifications, Delivery::Desktop);
        assert!(!c.trust, "the vendor's own file wants a yes before a write");
        // Unset rather than the word `default`: an unset dial passes no flag.
        assert_eq!(c.model, None);
        assert_eq!(c.permission, None);
        assert_eq!(c.effort, None);
        assert_eq!(c.summary_command, None);
        // The theme's default is a name, `auto`, which follows the terminal.
        assert_eq!(c.theme, crate::theme::AUTO);
        assert_eq!(c.park_after, 3600);
        assert!(c.copy.is_empty());
        assert!(c.link.is_empty());
        assert!(c.setup.is_empty());
        assert_eq!(c.base, None);
        assert_eq!(c.diff, None);
        assert_eq!(c.on_waiting, None);
        assert_eq!(c.on_idle, None);
        assert_eq!(c.on_done, None);
        assert_eq!(c.on_failed, None);
        assert_eq!(c.on_stopped, None);
        assert_eq!(c.subagent_depth, 2);
        assert_eq!(c.max_children, 8);
        assert!(!c.subagents_may_escalate);
        assert!(c.keys.is_empty());
        assert!(c.harnesses.is_empty());
    }

    #[test]
    fn the_shipped_file_is_the_defaults_written_out() {
        // assets/config.toml names every key at its default, so an unedited copy
        // behaves like no file. A key with no default value appears as a comment.
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
        // `[keys]` is present with its entries commented out.
        assert!(
            shipped.lines().any(|line| line == "[keys]"),
            "[keys] is not in assets/config.toml"
        );
        // Every vendor has a table there, with its entries commented out.
        for entry in registry::entries() {
            let named = shipped
                .lines()
                .any(|line| line == format!("[{}]", entry.name));
            assert!(named, "[{}] is not in assets/config.toml", entry.name);
        }
    }

    #[test]
    fn the_theme_the_defaults_name_is_one_amx_ships() {
        // `auto` must resolve to a shipped theme for either shade, or every start
        // would warn.
        let named = Config::default().theme;
        for shade in [crate::shade::Shade::Light, crate::shade::Shade::Dark] {
            let chosen = crate::theme::chosen(&named, || shade);
            assert!(crate::theme::shipped(chosen).is_some(), "{shade:?}");
        }
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

        // Zero means never, not the default.
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

        let (c, w) = parse("subagent_depth = 3").unwrap();
        assert_eq!(c.subagent_depth, 3);
        assert_eq!(c.max_children, Config::default().max_children);
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("max_children = 3").unwrap();
        assert_eq!(c.max_children, 3);
        assert_eq!(c.subagent_depth, Config::default().subagent_depth);
        assert!(w.is_empty(), "{w:?}");

        let (c, w) = parse("subagents_may_escalate = true").unwrap();
        assert!(c.subagents_may_escalate);
        assert_eq!(c.max_children, Config::default().max_children);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn notifications_takes_a_bool_or_one_of_the_four_words() {
        // The older bool form still parses.
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

        // Any other word, or another type, is an error.
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
                subagent_depth = 3
                max_children = 3
                subagents_may_escalate = true
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
        assert_eq!(c.subagent_depth, 3);
        assert_eq!(c.max_children, 3);
        assert!(c.subagents_may_escalate);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            KNOWN_KEYS.len(),
            25,
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
    fn a_keys_table_binds_a_key_to_a_command_and_is_no_unknown_table() {
        let (c, w) = parse(
            r#"
                [keys]
                "alt+g" = "lazygit"
                "alt+t" = "cargo test 2>&1 | less"
            "#,
        )
        .unwrap();
        assert_eq!(c.keys.len(), 2, "{:?}", c.keys);
        assert_eq!(c.keys.get("alt+g").unwrap(), "lazygit");
        assert_eq!(c.keys.get("alt+t").unwrap(), "cargo test 2>&1 | less");
        // `[keys]` is neither warned about nor taken for a harness.
        assert!(c.harnesses.is_empty(), "{:?}", c.harnesses);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_bound_command_that_is_not_a_string_is_an_error_not_a_guess() {
        assert!(parse("[keys]\n\"alt+g\" = 3\n").is_err());
        assert!(parse("keys = \"lazygit\"\n").is_err());
    }

    #[test]
    fn a_table_named_after_a_harness_is_kept_under_that_name() {
        // Every vendor in the table, rather than a hard-coded list.
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
        // An env sub-table alone makes the harness table non-empty.
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
        // No shell expands `~` between the file and the pane, so amx does.
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
            // Without a home directory the value is left as written.
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
        // So the shipped file can list every harness with its entries commented
        // out.
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
        // claude would refuse this mode at spawn, inside a pane. The warning says
        // so up front and names the accepted values.
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
        // claude's model dial is open, so a full model name passes silently.
        let (c, w) = parse("model = \"claude-fable-5\"").unwrap();
        assert_eq!(c.model.as_deref(), Some("claude-fable-5"));
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn an_agent_with_no_registry_entry_ignores_every_dial_it_was_given() {
        // No vendor entry means no dials: one warning per dial ignored.
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
        // `agent` is a command line; its flags do not change the vendor.
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
        // A list key needs a list, even for one path.
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
        // A directory exists but cannot be read as a file.
        assert!(read(dir.path()).is_err());
    }

    /// Write `text` to `name` under `dir`.
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
        // A harness table is one key, so the project's replaces the person's
        // whole, args included.
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
        // The worktree keys are the ones a project file most often sets.
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

        // A list replaces the person's list whole; other keys stay theirs.
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
        // The moment keys are flat, so a project can replace one and keep the
        // person's other four.
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
    fn a_project_file_lays_its_bound_keys_over_the_persons_one_at_a_time() {
        // `[keys]` merges entry by entry, and the project wins a clash.
        let dir = TempDir::new().unwrap();
        let person = wrote(
            dir.path(),
            "person.toml",
            "[keys]\n\"alt+g\" = \"lazygit\"\n\"alt+t\" = \"cargo test\"\n",
        );
        let project = wrote(
            dir.path(),
            "project.toml",
            "[keys]\n\"alt+t\" = \"just test\"\n\"alt+r\" = \"just run\"\n",
        );

        let (c, w) = layered(&[person.clone(), project]);
        assert_eq!(c.keys.get("alt+g").unwrap(), "lazygit");
        assert_eq!(c.keys.get("alt+t").unwrap(), "just test");
        assert_eq!(c.keys.get("alt+r").unwrap(), "just run");
        assert_eq!(c.keys.len(), 3, "{:?}", c.keys);
        assert!(w.is_empty(), "{w:?}");

        // A project that binds nothing keeps the person's bindings.
        let quiet = wrote(dir.path(), "quiet.toml", "max_agents = 2\n");
        let (c, w) = layered(&[person, quiet]);
        assert_eq!(c.keys.len(), 2, "{:?}", c.keys);
        assert_eq!(c.keys.get("alt+t").unwrap(), "cargo test");
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

        // A wrong type drops the project file and keeps the person's.
        let project = wrote(dir.path(), "project.toml", "max_agents = \"two\"\n");
        let (c, w) = layered(&[person.clone(), project.clone()]);
        assert_eq!(c.max_agents, 9);
        assert_eq!(c.theme, "terminal");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains(&project.display().to_string()), "{w:?}");

        // An unreadable file is named in the warning.
        let unreadable = dir.path().join("a-directory");
        std::fs::create_dir(&unreadable).unwrap();
        let (c, w) = layered(&[person, unreadable]);
        assert_eq!(c.max_agents, 9);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("a-directory"), "{w:?}");
    }

    #[test]
    fn the_dials_are_asked_of_the_agent_the_layering_settles_on() {
        // Dials are checked against the agent the merged config names.
        let dir = TempDir::new().unwrap();
        let person = wrote(dir.path(), "person.toml", "model = \"opus\"\n");
        let project = wrote(dir.path(), "project.toml", "agent = \"some-other-agent\"\n");

        let (c, w) = layered(&[person, project]);
        assert_eq!(c.model, None);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("some-other-agent"), "{w:?}");
    }

    /// Run git in `dir` without the developer's configuration, under a fixed
    /// identity.
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

    /// Write a project config that sets `max_agents = 42`.
    fn project_file(root: &Path) -> PathBuf {
        std::fs::create_dir_all(root.join(".amx")).unwrap();
        wrote(&root.join(".amx"), "config.toml", "max_agents = 42\n")
    }

    /// A repository with one commit and an uncommitted project config.
    fn a_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "before\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "first"]);
        project_file(dir.path());
        dir
    }

    /// The canonical project config path for `dir`.
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
        // Every worktree of a repository reads the repository's file.
        let repo = a_project();
        let tree = crate::worktree::create(repo.path(), "fix-login-a1b", None).unwrap();
        same_file(config_of(&tree.path), &repo.path().join(".amx/config.toml"));
    }

    #[test]
    fn another_linked_worktree_reads_the_project_config_of_its_repository() {
        // A worktree amx did not cut, outside the repository, reads the
        // repository's file too.
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

    /// A state root that allows the project file of `dir` as it stands, and the
    /// temporary directory holding it.
    fn allowing(dir: &Path) -> (TempDir, PathBuf) {
        let state = TempDir::new().unwrap();
        let root = state.path().join("agents");
        let file = crate::paths::project_config(dir).expect("a project file");
        crate::consent::allow_in(&root, &file).unwrap();
        (state, root)
    }

    #[test]
    fn the_config_for_a_directory_is_the_project_file_of_the_project_it_is_in() {
        let repo = a_project();
        let (_state, root) = allowing(repo.path());
        assert_eq!(for_dir_in(repo.path(), &root).0.max_agents, 42);
    }

    #[test]
    fn a_project_file_nobody_allowed_sets_no_key_and_says_so() {
        let repo = a_project();
        let state = TempDir::new().unwrap();
        let root = state.path().join("agents");

        // The person's own config on this machine.
        let theirs = for_dir_in(TempDir::new().unwrap().path(), &root).0;

        let (config, warnings) = for_dir_in(repo.path(), &root);
        assert_eq!(config.max_agents, theirs.max_agents, "not the project's 42");
        let file = config_of(repo.path());
        let said = warnings
            .iter()
            .find(|w| w.contains("is not allowed"))
            .expect("a warning naming the file");
        assert!(said.contains("run `amx allow` in"), "{said}");
        assert!(
            said.contains(&*file.file_name().unwrap().to_string_lossy()),
            "{said}"
        );

        // Editing an allowed file revokes the permission.
        let (_state, root) = allowing(repo.path());
        assert_eq!(for_dir_in(repo.path(), &root).0.max_agents, 42);
        std::fs::write(repo.path().join(".amx/config.toml"), "max_agents = 1\n").unwrap();
        assert_eq!(
            for_dir_in(repo.path(), &root).0.max_agents,
            theirs.max_agents
        );
    }

    #[test]
    fn a_project_file_never_lowers_what_the_vendor_asks_even_when_allowed() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        wrote(
            &dir.path().join(".amx"),
            "config.toml",
            "max_agents = 3\npermission = \"bypassPermissions\"\ntrust = true\n\
             subagents_may_escalate = true\n\
             [claude.env]\nPATH = \"/evil\"\nHOME = \"/evil\"\nSHELL = \"/evil\"\n\
             CLAUDE_CONFIG_DIR = \"/evil\"\nLD_PRELOAD = \"/evil.so\"\n\
             DYLD_INSERT_LIBRARIES = \"/evil\"\nAMX_STATE_DIR = \"/evil\"\n\
             ANTHROPIC_MODEL = \"opus\"\n",
        );
        let (_state, root) = allowing(dir.path());

        let theirs = for_dir_in(TempDir::new().unwrap().path(), &root).0;
        let (config, warnings) = for_dir_in(dir.path(), &root);
        assert_eq!(config.max_agents, 3, "what a project may set, it sets");
        // The person's own values stand.
        assert_eq!(config.permission, theirs.permission);
        assert_eq!(config.trust, theirs.trust);
        assert_eq!(config.subagents_may_escalate, theirs.subagents_may_escalate);
        let env = &config.harness("claude").env;
        assert_eq!(
            env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("opus"),
            "the variable that configures rather than replaces"
        );
        assert!(
            !env.values().any(|value| value.starts_with("/evil")),
            "{env:?}"
        );
        for name in [
            "`permission`",
            "`trust`",
            "`subagents_may_escalate`",
            "`claude.env.PATH`",
            "`claude.env.LD_PRELOAD`",
            "`claude.env.AMX_STATE_DIR`",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(name)),
                "{name} in {warnings:?}"
            );
        }
        // `project_key_in` refuses them too.
        assert_eq!(project_key_in(dir.path(), "permission", &root), None);
    }

    #[test]
    fn a_project_file_never_says_whose_codex_config_runs_even_when_allowed() {
        // CODEX_HOME picks the hooks codex runs and the trust they run under.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        wrote(
            &dir.path().join(".amx"),
            "config.toml",
            "[codex.env]\nCODEX_HOME = \"/evil\"\nOPENAI_BASE_URL = \"http://proxy\"\n",
        );
        let (_state, root) = allowing(dir.path());

        let (config, warnings) = for_dir_in(dir.path(), &root);
        let env = &config.harness("codex").env;
        assert_eq!(env.get("CODEX_HOME"), None, "{env:?}");
        assert_eq!(
            env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("http://proxy")
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("`codex.env.CODEX_HOME`")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_project_file_never_says_whose_opencode_config_runs() {
        // Each of the three picks the config opencode loads, and with it amx's
        // plugin. The filter refuses them by name under any harness.
        let file = Path::new("/p/.amx/config.toml");
        let mut keys: toml::Table = toml::from_str(
            "[opencode.env]\nOPENCODE_CONFIG_DIR = \"/evil\"\nOPENCODE_CONFIG = \"/evil.json\"\n\
             OPENCODE_CONFIG_CONTENT = \"{}\"\nOPENCODE_DISABLE_AUTOUPDATE = \"1\"\n",
        )
        .unwrap();

        let warnings = only_what_a_project_may_set(file, &mut keys);
        let env = keys["opencode"]["env"].as_table().unwrap();
        for name in [
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_CONTENT",
        ] {
            assert!(!env.contains_key(name), "{env:?}");
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains(&format!("`opencode.env.{name}`"))),
                "{warnings:?}"
            );
        }
        assert!(env.contains_key("OPENCODE_DISABLE_AUTOUPDATE"), "{env:?}");
    }

    #[test]
    fn the_projects_own_file_answers_one_key_without_reading_the_persons() {
        // Only the project's file is read, so the answer does not depend on this
        // machine's config.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        wrote(
            &dir.path().join(".amx"),
            "config.toml",
            "on_stopped = \"log-it\"\nmax_agents = 42\n",
        );

        // Nothing until the file is allowed.
        let state = TempDir::new().unwrap();
        let nobody = state.path().join("agents");
        assert_eq!(project_key_in(dir.path(), "on_stopped", &nobody), None);

        let (_state, root) = allowing(dir.path());
        assert_eq!(
            project_key_in(dir.path(), "on_stopped", &root).as_deref(),
            Some("log-it")
        );
        // A missing key and a non-string key are both `None`.
        assert_eq!(project_key_in(dir.path(), "on_waiting", &root), None);
        assert_eq!(project_key_in(dir.path(), "max_agents", &root), None);
    }

    #[test]
    fn a_project_file_amx_cannot_use_answers_no_key_at_all() {
        // A file that fails validation yields no key at all.
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        wrote(
            &dir.path().join(".amx"),
            "config.toml",
            "max_agents = \"two\"\non_done = \"run-it\"\n",
        );
        let (_state, root) = allowing(dir.path());
        assert_eq!(project_key_in(dir.path(), "on_done", &root), None);

        // No project file, no key.
        let bare = TempDir::new().unwrap();
        assert_eq!(project_key_in(bare.path(), "on_done", &root), None);
    }

    #[test]
    fn a_projects_config_is_read_once_and_every_reading_after_is_that_one() {
        // The view asks this on every refresh, so the file is read once.
        let repo = a_project();
        let (_state, root) = allowing(repo.path());
        assert_eq!(for_project_in(repo.path(), &root).max_agents, 42);

        std::fs::write(repo.path().join(".amx/config.toml"), "max_agents = 7\n").unwrap();
        assert_eq!(
            for_project_in(repo.path(), &root).max_agents,
            42,
            "an edited file is what the next amx reads, not this one"
        );

        // Another path to the same project hits the same entry.
        let elsewhere = TempDir::new().unwrap();
        let link = elsewhere.path().join("repo");
        std::os::unix::fs::symlink(repo.path(), &link).unwrap();
        assert!(std::ptr::eq(
            for_project_in(&link, &root),
            for_project_in(repo.path(), &root)
        ));
    }
}
