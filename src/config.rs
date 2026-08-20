//! `~/.config/amx/config.toml` — four keys and nothing else.
//!
//! Config is a convenience, never a gate: a file that cannot be read or
//! parsed degrades to the defaults with a warning on stderr, because losing
//! an agent to a stray comma is a worse outcome than running with defaults.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Every key the file may carry. Anything else is warned about and ignored.
pub const KNOWN_KEYS: [&str; 4] = ["agent", "max_agents", "worktrees", "notifications"];

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: "claude".to_string(),
            max_agents: 5,
            worktrees: true,
            notifications: true,
        }
    }
}

/// Parse config text, returning the config and any warnings about it.
///
/// An unknown key is a warning: config files outlive the versions that wrote
/// them. A key with the wrong *type* is an error, because guessing what
/// `max_agents = "five"` meant is worse than saying so.
pub fn parse(text: &str) -> Result<(Config, Vec<String>)> {
    let table: toml::Table = text.parse().context("not valid TOML")?;
    let warnings = table
        .keys()
        .filter(|key| !KNOWN_KEYS.contains(&key.as_str()))
        .map(|key| format!("ignoring unknown key `{key}`"))
        .collect();
    let config: Config = toml::from_str(text)?;
    Ok((config, warnings))
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
    }

    #[test]
    fn all_four_keys_together_parse() {
        let (c, w) = parse(
            r#"
                agent = "claude --dangerously-skip-permissions"
                max_agents = 3
                worktrees = false
                notifications = false
            "#,
        )
        .unwrap();
        assert_eq!(c.agent, "claude --dangerously-skip-permissions");
        assert_eq!(c.max_agents, 3);
        assert!(!c.worktrees);
        assert!(!c.notifications);
        assert!(w.is_empty());
    }

    #[test]
    fn an_unknown_key_is_named_in_a_warning_and_the_rest_still_applies() {
        let (c, warnings) = parse("max_agents = 2\nwardrobe = true\n").unwrap();
        assert_eq!(c.max_agents, 2);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("wardrobe"), "{warnings:?}");
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
