//! `~/.config/amx/config.toml` — twelve keys and nothing else — with a
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
use std::path::{Path, PathBuf};

/// Every key the file may carry. Anything else is warned about and ignored.
pub const KNOWN_KEYS: [&str; 12] = [
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
];

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
    /// Post desktop notifications on the transitions worth interrupting for.
    pub notifications: bool,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: "claude".to_string(),
            max_agents: 5,
            max_total: None,
            worktrees: true,
            notifications: true,
            trust: false,
            model: None,
            permission: None,
            effort: None,
            summary_command: None,
            theme: "default".to_string(),
            park_after: 3600,
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
    let mut config: Config = table.try_into()?;
    warnings.extend(check_dials(&mut config));
    Ok((config, warnings))
}

/// The keys of a file amx has never heard of, one warning each.
fn unknown_keys(table: &toml::Table) -> Vec<String> {
    table
        .keys()
        .filter(|key| !KNOWN_KEYS.contains(&key.as_str()))
        .map(|key| format!("ignoring unknown key `{key}`"))
        .collect()
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
    // laid over another is that key entire, so what they make parses too.
    let mut config: Config = keys.try_into().unwrap_or_default();
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
        assert!(c.notifications);
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
        assert!(c.notifications);

        let (c, _) = parse("notifications = false").unwrap();
        assert!(!c.notifications);
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
    }

    #[test]
    fn every_key_there_is_parses_beside_all_the_others() {
        let (c, w) = parse(
            r#"
                agent = "claude --dangerously-skip-permissions"
                max_agents = 3
                max_total = 8
                worktrees = false
                notifications = false
                trust = true
                model = "opus"
                permission = "plan"
                effort = "xhigh"
                summary_command = "summarise"
                theme = "terminal"
                park_after = 900
            "#,
        )
        .unwrap();
        assert_eq!(c.agent, "claude --dangerously-skip-permissions");
        assert_eq!(c.max_agents, 3);
        assert_eq!(c.max_total, Some(8));
        assert!(!c.worktrees);
        assert!(!c.notifications);
        assert!(c.trust);
        assert_eq!(c.model.as_deref(), Some("opus"));
        assert_eq!(c.permission.as_deref(), Some("plan"));
        assert_eq!(c.effort.as_deref(), Some("xhigh"));
        assert_eq!(c.summary_command.as_deref(), Some("summarise"));
        assert_eq!(c.theme, "terminal");
        assert_eq!(c.park_after, 900);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            KNOWN_KEYS.len(),
            12,
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
        let tree = crate::worktree::create(repo.path(), "fix-login-a1b").unwrap();
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
