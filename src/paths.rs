//! Where amx keeps things.
//!
//! * **state** — `~/.local/state/amx/agents/<id>/`, durable, survives reboots.
//!   `$AMX_STATE_DIR` (tests) replaces the `amx` root, so agent directories
//!   land at `$AMX_STATE_DIR/agents/<id>/`.
//! * **config** — `$XDG_CONFIG_HOME/amx/config.toml`, else
//!   `~/.config/amx/config.toml`.
//!
//! The environment is read only by the wrappers; the layout rules themselves
//! are pure functions over their inputs, and that is what the tests drive.
//! Mutating environment variables is process-global and `unsafe` in edition
//! 2024 — not something a parallel test suite may do.

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Test-only override of the state root.
const STATE_DIR_ENV: &str = "AMX_STATE_DIR";

/// What a directory amx makes is kept at, and what a file it writes is kept
/// at: an agent's task, the answers a person gave it and the path to the
/// transcript of the whole conversation are the owner's business alone.
pub const DIR_MODE: u32 = 0o700;
pub const FILE_MODE: u32 = 0o600;

/// Set `path` to `mode`.
///
/// The mode is asked for at creation *and* set here, because neither creation
/// call keeps the promise on its own: a mode handed to `open` is a request the
/// umask may take bits out of, and it is ignored outright for a file that is
/// already there — a state file amx wrote before this law existed, or a log an
/// agent's own shell left lying about.
pub fn keep_to_the_owner(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("keeping {} to its owner", path.display()))
}

/// The directory holding one subdirectory per agent.
pub fn state_root() -> Result<PathBuf> {
    let over = env_path(std::env::var_os(STATE_DIR_ENV));
    match over {
        Some(dir) => Ok(state_root_from(Some(&dir), Path::new("/"))),
        None => Ok(state_root_from(None, &home()?)),
    }
}

/// `<id>`'s state directory.
pub fn agent_dir(id: &str) -> Result<PathBuf> {
    agent_dir_in(&state_root()?, id)
}

/// What the view keeps between runs: the way it was gathered, the agents held
/// at the top of their groups, the order somebody put a group in, and whether
/// the status line has been offered.
///
/// Beside the agents rather than among them. Nothing in it belongs to an agent
/// — it is the reader's own — and a file inside the agents directory would be
/// an entry every walk of that directory has to know is not an agent.
pub fn view_file(state_root: &Path) -> Option<PathBuf> {
    state_root
        .parent()
        // A relative root has an empty parent, which names wherever the
        // process happens to be running rather than anywhere amx keeps things.
        .filter(|root| !root.as_os_str().is_empty())
        .map(|root| root.join(VIEW))
}

/// What that file is called.
const VIEW: &str = "view.json";

/// The config file amx reads, whether or not it exists.
pub fn config_file() -> Result<PathBuf> {
    let xdg = env_path(std::env::var_os("XDG_CONFIG_HOME"));
    Ok(config_file_from(xdg.as_deref(), &home()?))
}

fn home() -> Result<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().context("no home directory: set $HOME, or $AMX_STATE_DIR in tests")
}

/// An environment variable's value as a path — an unset variable and an empty
/// one mean the same thing, which is what XDG says and what a shell that
/// exports `FOO=` produces.
fn env_path(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The state layout, with its two inputs as parameters.
fn state_root_from(state_dir_override: Option<&Path>, home: &Path) -> PathBuf {
    let root = match state_dir_override {
        Some(dir) => dir.to_path_buf(),
        None => home.join(".local/state/amx"),
    };
    root.join("agents")
}

/// The config layout, with its two inputs as parameters.
fn config_file_from(xdg_config_home: Option<&Path>, home: &Path) -> PathBuf {
    let root = match xdg_config_home {
        Some(dir) => dir.to_path_buf(),
        None => home.join(".config"),
    };
    root.join("amx/config.toml")
}

/// The join itself — and the id law checked *here*, where an id becomes a
/// path, rather than only where one is minted. `state_root.join(id)` is not a
/// lookup: an id of `../../elsewhere` addresses any directory on the machine,
/// and an absolute one replaces the root outright.
pub(crate) fn agent_dir_in(state_root: &Path, id: &str) -> Result<PathBuf> {
    if !crate::ids::is_valid(id) {
        bail!("no agent `{id}`");
    }
    Ok(state_root.join(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_lives_under_the_home_directory_by_default() {
        assert_eq!(
            state_root_from(None, Path::new("/home/dev")),
            Path::new("/home/dev/.local/state/amx/agents")
        );
    }

    #[test]
    fn the_state_override_replaces_the_amx_root_not_the_agents_child() {
        assert_eq!(
            state_root_from(Some(Path::new("/tmp/t1")), Path::new("/home/dev")),
            Path::new("/tmp/t1/agents")
        );
    }

    #[test]
    fn the_view_keeps_its_own_file_beside_the_agents() {
        assert_eq!(
            view_file(&state_root_from(None, Path::new("/home/dev"))),
            Some(PathBuf::from("/home/dev/.local/state/amx/view.json"))
        );
        assert_eq!(
            view_file(&state_root_from(
                Some(Path::new("/tmp/t1")),
                Path::new("/home/dev")
            )),
            Some(PathBuf::from("/tmp/t1/view.json")),
            "and it follows the state root wherever that was pointed"
        );
        assert_eq!(
            view_file(Path::new("agents")),
            None,
            "a root with nowhere above it is not a place to write"
        );
    }

    #[test]
    fn config_follows_xdg_and_falls_back_to_dot_config() {
        assert_eq!(
            config_file_from(Some(Path::new("/cfg")), Path::new("/home/dev")),
            Path::new("/cfg/amx/config.toml")
        );
        assert_eq!(
            config_file_from(None, Path::new("/home/dev")),
            Path::new("/home/dev/.config/amx/config.toml")
        );
    }

    #[test]
    fn an_empty_environment_variable_reads_as_unset() {
        assert_eq!(env_path(None), None);
        assert_eq!(env_path(Some(OsString::from(""))), None);
        assert_eq!(
            env_path(Some(OsString::from("/tmp/t2"))),
            Some(PathBuf::from("/tmp/t2"))
        );
    }

    #[test]
    fn the_wrappers_root_the_layout_at_the_agents_directory() {
        // Reads the ambient environment and never touches it: whichever branch
        // it takes, the answer is the agents directory with the id under it.
        let Ok(root) = state_root() else { return };
        assert!(root.ends_with("agents"), "{}", root.display());
        assert_eq!(
            agent_dir("fix-login-a1b").unwrap(),
            root.join("fix-login-a1b")
        );
        assert!(agent_dir("../escape").is_err());
    }

    #[test]
    fn hardening_a_mode_is_set_rather_than_asked_for() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        keep_to_the_owner(&path, FILE_MODE).unwrap();
        keep_to_the_owner(dir.path(), DIR_MODE).unwrap();
        assert_eq!(mode_of(&path), FILE_MODE, "a file that was already there");
        assert_eq!(mode_of(dir.path()), DIR_MODE);

        // And a path that is not there is a failure with the path in it.
        let missing = dir.path().join("never-written");
        let said = format!("{:#}", keep_to_the_owner(&missing, FILE_MODE).unwrap_err());
        assert!(said.contains("never-written"), "{said}");
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn an_agent_directory_is_a_valid_id_under_the_root() {
        let root = Path::new("/state/agents");
        assert_eq!(
            agent_dir_in(root, "fix-login-a1b").unwrap(),
            Path::new("/state/agents/fix-login-a1b")
        );
    }

    #[test]
    fn an_id_that_would_leave_the_root_is_refused_at_the_join() {
        let root = Path::new("/state/agents");
        for bad in ["", "..", "../../etc", "/etc/passwd", "a/b"] {
            assert!(agent_dir_in(root, bad).is_err(), "{bad:?} must be refused");
        }
    }
}
