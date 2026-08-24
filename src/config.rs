//! `~/.config/amx/config.toml` — nine keys and nothing else.
//!
//! Config is a convenience, never a gate: a file that cannot be read or
//! parsed degrades to the defaults with a warning on stderr, because losing
//! an agent to a stray comma is a worse outcome than running with defaults.

use crate::registry;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Every key the file may carry. Anything else is warned about and ignored.
pub const KNOWN_KEYS: [&str; 9] = [
    "agent",
    "max_agents",
    "worktrees",
    "notifications",
    "trust",
    "model",
    "permission",
    "effort",
    "summary_command",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The vendor command a new agent runs.
    pub agent: String,
    /// How many live agents `new` will allow before it refuses.
    pub max_agents: usize,
    /// Give new agents their own git worktree.
    pub worktrees: bool,
    /// Post desktop notifications on the transitions worth interrupting for.
    pub notifications: bool,
    /// Answer claude's folder-trust screen for the worktrees amx cuts, by
    /// writing the vendor's own trust store. Off until the person says so:
    /// the store is their file, and this is the consent the write stands
    /// behind, the way the hooks stand behind doctor --fix's yes.
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: "claude".to_string(),
            max_agents: 5,
            worktrees: true,
            notifications: true,
            trust: false,
            model: None,
            permission: None,
            effort: None,
            summary_command: None,
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
    let mut warnings: Vec<String> = table
        .keys()
        .filter(|key| !KNOWN_KEYS.contains(&key.as_str()))
        .map(|key| format!("ignoring unknown key `{key}`"))
        .collect();
    let mut config: Config = toml::from_str(text)?;
    warnings.extend(check_dials(&mut config));
    Ok((config, warnings))
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

/// The config as amx runs with it, with warnings for the caller to print.
pub fn load() -> (Config, Vec<String>) {
    match crate::paths::config_file() {
        Ok(path) => load_from(&path),
        Err(e) => (Config::default(), vec![format!("using defaults: {e}")]),
    }
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
    }

    #[test]
    fn every_key_there_is_parses_beside_all_the_others() {
        let (c, w) = parse(
            r#"
                agent = "claude --dangerously-skip-permissions"
                max_agents = 3
                worktrees = false
                notifications = false
                trust = true
                model = "opus"
                permission = "plan"
                effort = "xhigh"
                summary_command = "summarise"
            "#,
        )
        .unwrap();
        assert_eq!(c.agent, "claude --dangerously-skip-permissions");
        assert_eq!(c.max_agents, 3);
        assert!(!c.worktrees);
        assert!(!c.notifications);
        assert!(c.trust);
        assert_eq!(c.model.as_deref(), Some("opus"));
        assert_eq!(c.permission.as_deref(), Some("plan"));
        assert_eq!(c.effort.as_deref(), Some("xhigh"));
        assert_eq!(c.summary_command.as_deref(), Some("summarise"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            KNOWN_KEYS.len(),
            9,
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
}
