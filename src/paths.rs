//! Where amx keeps its files.
//!
//! - State: `~/.local/state/amx/agents/<id>/`. `$AMX_STATE_DIR` replaces the
//!   `~/.local/state/amx` root, for tests.
//! - Config: `$XDG_CONFIG_HOME/amx/config.toml`, else
//!   `~/.config/amx/config.toml`.
//! - Project config: `<project>/.amx/config.toml`, where the project is the
//!   repository, never a worktree of it.
//!
//! Only the public wrappers read the environment; the layout rules are pure
//! functions the tests call directly, since setting environment variables is
//! process-global and `unsafe` in edition 2024.

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Test-only override of the state root.
const STATE_DIR_ENV: &str = "AMX_STATE_DIR";

/// Modes for the directories and files amx writes: owner only, since they hold
/// tasks, answers and transcript paths.
pub const DIR_MODE: u32 = 0o700;
pub const FILE_MODE: u32 = 0o600;

/// Set `path` to `mode`.
///
/// Needed on top of the mode passed at creation: the umask can clear bits from
/// that, and it does not apply at all to a file that already exists.
pub fn keep_to_the_owner(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("keeping {} to its owner", path.display()))
}

/// A file's length and mtime, or `None` if it does not exist.
///
/// Compared against an earlier stamp to tell whether the file changed. The
/// length catches a second write within one mtime tick on filesystems with
/// coarse timestamps.
pub(crate) fn stamped(path: &Path) -> Option<(u64, SystemTime)> {
    let file = std::fs::metadata(path).ok()?;
    Some((file.len(), file.modified().ok()?))
}

/// The directory holding one subdirectory per agent.
pub fn state_root() -> Result<PathBuf> {
    let over = env_path(std::env::var_os(STATE_DIR_ENV));
    match over {
        Some(dir) => Ok(state_root_from(Some(&dir), Path::new("/"))),
        None => Ok(state_root_from(None, &home()?)),
    }
}

/// The state directory of agent `id`.
pub fn agent_dir(id: &str) -> Result<PathBuf> {
    agent_dir_in(&state_root()?, id)
}

/// The view's persisted settings: grouping, held agents, group order, and
/// whether the status line was offered.
///
/// Kept beside the agents directory so walks of it see only agents.
pub fn view_file(state_root: &Path) -> Option<PathBuf> {
    beside_the_agents(state_root, VIEW)
}

const VIEW: &str = "view.json";

/// The agents a terminal was handed to, newest first, for `amx attach --last`.
pub fn visited_file(state_root: &Path) -> Option<PathBuf> {
    beside_the_agents(state_root, VISITED)
}

const VISITED: &str = "visited.json";

/// The terminal colours the view last read, as a tmux style
/// (`fg=#rrggbb,bg=#rrggbb` or `bg=#rrggbb`) for new agent panes.
pub fn background_file(state_root: &Path) -> PathBuf {
    state_root.parent().unwrap_or(state_root).join(BACKGROUND)
}

const BACKGROUND: &str = "background";

/// A file of amx's own in the parent of the agents directory.
fn beside_the_agents(state_root: &Path, name: &str) -> Option<PathBuf> {
    state_root
        .parent()
        // A relative root has an empty parent, meaning the current directory.
        .filter(|root| !root.as_os_str().is_empty())
        .map(|root| root.join(name))
}

/// Where cached vendor model listings are kept, one file per harness.
pub fn models_dir() -> Result<PathBuf> {
    beside_the_agents(&state_root()?, MODELS).context("no state root to keep a model listing under")
}

const MODELS: &str = "models";

/// The config file amx reads, whether or not it exists.
pub fn config_file() -> Result<PathBuf> {
    let xdg = env_path(std::env::var_os("XDG_CONFIG_HOME"));
    Ok(config_file_from(xdg.as_deref(), &home()?))
}

/// A project's config file, relative to the project root.
const PROJECT_CONFIG: &str = ".amx/config.toml";

/// The config file of the project holding `dir`, whether or not it exists.
///
/// The project is the repository, so every tree of it shares one file. A tree
/// amx cut is resolved from its path, which works after the tree is gone; any
/// other directory asks git for its main repository.
pub fn project_config(dir: &Path) -> Option<PathBuf> {
    // Made absolute first: outside a repository the directory is the project,
    // and records hold absolute directories, so a relative one would match no
    // agent when counting caps.
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let project = if crate::worktree::is_amx_tree(&dir) {
        crate::worktree::repo_of(&dir)?
    } else {
        // Outside a repository the directory itself is the project.
        crate::worktree::main_repo(&dir).unwrap_or_else(|_| dir.clone())
    };
    Some(project.join(PROJECT_CONFIG))
}

/// `dir` made absolute and, where it exists, canonicalized, so two spellings
/// of one directory compare equal.
///
/// Records hold canonical absolute directories. A directory that does not
/// exist is only made absolute, and the caller decides whether that is an
/// error.
pub fn anchored(dir: &Path) -> Result<PathBuf> {
    let anchored = std::path::absolute(dir)
        .with_context(|| format!("reading the directory `{}`", dir.display()))?;
    Ok(std::fs::canonicalize(&anchored).unwrap_or(anchored))
}

fn home() -> Result<PathBuf> {
    std::env::home_dir().context("no home directory: set $HOME, or $AMX_STATE_DIR in tests")
}

/// An environment variable as a path, with empty treated as unset (as XDG
/// specifies).
fn env_path(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The state layout, with its inputs as parameters.
fn state_root_from(state_dir_override: Option<&Path>, home: &Path) -> PathBuf {
    let root = match state_dir_override {
        Some(dir) => dir.to_path_buf(),
        None => home.join(".local/state/amx"),
    };
    root.join("agents")
}

/// The config layout, with its inputs as parameters.
fn config_file_from(xdg_config_home: Option<&Path>, home: &Path) -> PathBuf {
    let root = match xdg_config_home {
        Some(dir) => dir.to_path_buf(),
        None => home.join(".config"),
    };
    root.join("amx/config.toml")
}

/// The state directory of `id` under `state_root`.
///
/// The id is validated here, where it becomes a path: `join` with
/// `../../elsewhere` or an absolute id would escape the root.
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
    fn the_background_is_kept_beside_the_agents() {
        assert_eq!(
            background_file(&state_root_from(None, Path::new("/home/dev"))),
            PathBuf::from("/home/dev/.local/state/amx/background")
        );
        assert_eq!(
            background_file(&state_root_from(
                Some(Path::new("/tmp/t1")),
                Path::new("/home/dev")
            )),
            PathBuf::from("/tmp/t1/background")
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
    fn the_trail_of_visited_agents_is_kept_beside_the_agents() {
        assert_eq!(
            visited_file(&state_root_from(None, Path::new("/home/dev"))),
            Some(PathBuf::from("/home/dev/.local/state/amx/visited.json"))
        );
        assert_eq!(
            visited_file(&state_root_from(
                Some(Path::new("/tmp/t1")),
                Path::new("/home/dev")
            )),
            Some(PathBuf::from("/tmp/t1/visited.json")),
            "beside the view's own file, wherever the state root was pointed"
        );
        assert_eq!(
            visited_file(Path::new("agents")),
            None,
            "a root with nowhere above it is not a place to write"
        );
    }

    #[test]
    fn a_vendors_model_listing_is_kept_beside_the_agents() {
        // Reads the real environment without changing it.
        let (Ok(agents), Ok(models)) = (state_root(), models_dir()) else {
            return;
        };
        assert!(models.ends_with(MODELS), "{}", models.display());
        assert_eq!(models.parent(), agents.parent());
    }

    #[test]
    fn the_project_config_of_a_tree_amx_cut_is_the_repositorys_own() {
        // Resolved from the layout alone, with no git and no directory needed.
        assert_eq!(
            project_config(Path::new("/src/app/.amx/worktrees/fix-login-a1b")),
            Some(PathBuf::from("/src/app/.amx/config.toml"))
        );
    }

    #[test]
    fn the_project_config_of_a_relative_directory_is_anchored_on_the_working_directory() {
        // `--dir ../scratch` arrives as typed. Outside a repository the project
        // must be the absolute directory the records hold.
        let found = project_config(Path::new("scratch")).expect("a project");
        assert!(found.is_absolute(), "{}", found.display());
        assert!(
            found.starts_with(std::env::current_dir().unwrap()),
            "under the working directory: {}",
            found.display()
        );
        assert!(found.ends_with(PROJECT_CONFIG), "{}", found.display());
    }

    #[test]
    fn a_directory_is_anchored_on_the_working_directory_and_read_off_the_disk() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            anchored(Path::new("scratch")).unwrap(),
            here.join("scratch"),
            "a directory that is not there is anchored and no more"
        );

        // An existing directory is canonicalized: `..` and symlinks resolve.
        let dir = tempfile::TempDir::new().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(anchored(dir.path()).unwrap(), real);
        assert_eq!(
            anchored(&dir.path().join("..").join(dir.path().file_name().unwrap())).unwrap(),
            real
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
        // Reads the real environment without changing it.
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

        // A missing path fails with the path in the message.
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
