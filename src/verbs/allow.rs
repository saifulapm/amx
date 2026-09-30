//! `amx allow`: consent to reading this project's `.amx/config.toml`.
//!
//! A project file names programs amx will run, so it is read only after a
//! person has allowed it (see [`crate::consent`]). The verb prints the file and
//! records a copy of exactly those bytes; running it is the consent. `--forget`
//! withdraws it. A directory with no project file is a failure, since the
//! caller expected one.

use anyhow::Result;
use std::io::Write;
use std::path::Path;

use crate::{complain, consent, exit, paths};

/// Run the verb for the project `dir` (or the working directory) is in.
pub fn from_env(dir: Option<&Path>, forget: bool) -> Result<i32> {
    let root = paths::state_root()?;
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let mut out = std::io::stdout().lock();
    run(&root, &dir, forget, &mut out)
}

/// The verb, with the state directory named.
pub fn run(root: &Path, dir: &Path, forget: bool, out: &mut impl Write) -> Result<i32> {
    let Some(file) = paths::project_config(dir).filter(|file| file.is_file()) else {
        complain!(
            "amx: {} has no project file: nothing to allow",
            dir.display()
        );
        return Ok(exit::FAILURE);
    };
    if forget {
        consent::forget_in(root, &file)?;
        writeln!(out, "forgot {}", file.display())?;
        return Ok(exit::OK);
    }
    let text = std::fs::read_to_string(&file)?;
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    consent::allow_in(root, &file)?;
    writeln!(out, "allowed {}", file.display())?;
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn allow_prints_the_file_and_keeps_it_until_it_is_forgotten() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join(".amx")).unwrap();
        let file = project.join(".amx/config.toml");
        std::fs::write(&file, "max_agents = 2").unwrap();

        let mut out = Vec::new();
        assert_eq!(run(&root, &project, false, &mut out).unwrap(), exit::OK);
        let said = String::from_utf8(out).unwrap();
        assert!(said.starts_with("max_agents = 2\n"), "{said}");
        assert!(
            said.contains(&format!("allowed {}", file.display())),
            "{said}"
        );
        assert!(consent::allowed_in(&root, &file));

        let mut out = Vec::new();
        assert_eq!(run(&root, &project, true, &mut out).unwrap(), exit::OK);
        assert!(String::from_utf8(out).unwrap().starts_with("forgot "));
        assert!(!consent::allowed_in(&root, &file));
    }

    #[test]
    fn allow_fails_where_there_is_no_project_file() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let mut out = Vec::new();
        assert_eq!(
            run(&root, dir.path(), false, &mut out).unwrap(),
            exit::FAILURE
        );
        assert!(out.is_empty());
    }
}
