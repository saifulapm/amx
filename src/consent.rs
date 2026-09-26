//! Which project files somebody has said amx may read.
//!
//! A repository's `.amx/config.toml` can name the program a pane runs, the
//! shell lines run before and after a turn, and the keys the view binds. That
//! is code, and a repository somebody cloned five minutes ago is code nobody
//! here wrote. So a project file counts only once a person has allowed it, and
//! only as long as it still says what it said then: `amx allow` keeps a copy of
//! the bytes it was shown, and a file that no longer matches that copy is a
//! file nobody has allowed. An agent that edits the file un-allows it by doing
//! so, which is the point — the agent is the other party this is about.
//!
//! The copies are kept beside the agents, where the view's own file is: which
//! files a person trusts is theirs and not any agent's. Each is named after
//! the file it stands for, the path spelled out with `%` and `/` escaped, so
//! the name is one directory entry and says which file it is. Bytes are
//! compared rather than hashed: the files are a few hundred bytes, and a copy
//! is what a person inspecting the directory can read.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::paths;

/// The directory beside the agents that holds the allowed copies.
const ALLOWED: &str = "allowed";

/// Whether the project file at `project_file` is one somebody allowed, as it
/// stands now.
#[allow(dead_code)] // config::for_dir asks this once it gates the project file
pub fn allowed(project_file: &Path) -> bool {
    paths::state_root().is_ok_and(|root| allowed_in(&root, project_file))
}

/// The same, with the state directory named.
///
/// A file that cannot be read is not allowed: there is nothing to compare,
/// and nothing a caller could take a key from either.
#[allow(dead_code)] // the same
pub fn allowed_in(root: &Path, project_file: &Path) -> bool {
    let Some(copy) = copy_of(root, project_file) else {
        return false;
    };
    match (std::fs::read(&copy), std::fs::read(project_file)) {
        (Ok(kept), Ok(now)) => kept == now,
        _ => false,
    }
}

/// Allow the file as it stands now: keep a copy of its bytes.
pub fn allow_in(root: &Path, project_file: &Path) -> Result<()> {
    let copy = copy_of(root, project_file).context("no place beside the agents to keep it")?;
    let bytes = std::fs::read(project_file)
        .with_context(|| format!("reading {}", project_file.display()))?;
    let dir = copy.parent().expect("the copy sits in a directory");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::store::write_atomic(&copy, &bytes)
}

/// Stop allowing the file. Whether there was a copy to remove.
pub fn forget_in(root: &Path, project_file: &Path) -> Result<bool> {
    let Some(copy) = copy_of(root, project_file) else {
        return Ok(false);
    };
    match std::fs::remove_file(&copy) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("removing {}", copy.display())),
    }
}

/// Where the copy standing for `project_file` is kept.
fn copy_of(root: &Path, project_file: &Path) -> Option<PathBuf> {
    let file = std::path::absolute(project_file).ok()?;
    let name = file
        .to_string_lossy()
        .replace('%', "%25")
        .replace('/', "%2F");
    let beside = root.parent().filter(|up| !up.as_os_str().is_empty())?;
    Some(beside.join(ALLOWED).join(format!("{name}.toml")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn project(dir: &TempDir, text: &str) -> PathBuf {
        let file = dir.path().join("repo/.amx/config.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        file
    }

    #[test]
    fn consent_holds_for_the_bytes_that_were_allowed_and_no_others() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let file = project(&dir, "max_agents = 2\n");

        assert!(!allowed_in(&root, &file), "nothing is allowed until it is");
        allow_in(&root, &file).unwrap();
        assert!(allowed_in(&root, &file));

        // One byte is a different file: an agent that edits it un-allows it.
        std::fs::write(&file, "max_agents = 3\n").unwrap();
        assert!(!allowed_in(&root, &file));

        // Allowed again as it stands, and then forgotten.
        allow_in(&root, &file).unwrap();
        assert!(allowed_in(&root, &file));
        assert!(forget_in(&root, &file).unwrap());
        assert!(!allowed_in(&root, &file));
        assert!(!forget_in(&root, &file).unwrap(), "nothing left to forget");
    }

    #[test]
    fn consent_keeps_the_copy_beside_the_agents_named_after_the_file() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let file = project(&dir, "base = \"main\"\n");
        allow_in(&root, &file).unwrap();

        let name = format!("{}.toml", file.to_string_lossy().replace('/', "%2F"));
        let copy = dir.path().join("state/allowed").join(name);
        assert_eq!(std::fs::read_to_string(copy).unwrap(), "base = \"main\"\n");
    }

    #[test]
    fn consent_is_never_given_to_a_file_that_is_not_there() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let file = project(&dir, "base = \"main\"\n");
        allow_in(&root, &file).unwrap();
        std::fs::remove_file(&file).unwrap();
        assert!(!allowed_in(&root, &file));
    }
}
