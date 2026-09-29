//! Consent for project config files.
//!
//! A repository's `.amx/config.toml` can name the program a pane runs, shell
//! commands run around a turn, and view key bindings, so it is code. A project
//! file counts only after `amx allow`, and only while its bytes match the copy
//! taken then. An agent that edits the file revokes the consent.
//!
//! Copies live in `allowed/` next to the agents directory, one per file, named
//! after the file's absolute path with `%` and `/` percent-escaped. The files
//! are small, so bytes are compared directly and the copy stays readable.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Directory next to the agents that holds the allowed copies.
const ALLOWED: &str = "allowed";

/// Whether `project_file` is allowed as it stands now, per the copies kept
/// next to `root`. A file that cannot be read is not allowed.
pub fn allowed_in(root: &Path, project_file: &Path) -> bool {
    let Some(copy) = copy_of(root, project_file) else {
        return false;
    };
    match (std::fs::read(&copy), std::fs::read(project_file)) {
        (Ok(kept), Ok(now)) => kept == now,
        _ => false,
    }
}

/// Allow the file as it stands now by keeping a copy of its bytes.
pub fn allow_in(root: &Path, project_file: &Path) -> Result<()> {
    let copy = copy_of(root, project_file).context("no place beside the agents to keep it")?;
    let bytes = std::fs::read(project_file)
        .with_context(|| format!("reading {}", project_file.display()))?;
    let dir = copy.parent().expect("the copy sits in a directory");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::store::write_atomic(&copy, &bytes)
}

/// Revoke consent for the file. Returns whether there was a copy to remove.
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

/// Path of the copy kept for `project_file`.
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

        // Any edit revokes consent.
        std::fs::write(&file, "max_agents = 3\n").unwrap();
        assert!(!allowed_in(&root, &file));

        // Allow again, then forget.
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
