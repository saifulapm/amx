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
    // One reading is printed, kept and reported, so what the person saw is
    // what is allowed.
    let bytes = std::fs::read(&file)?;
    let text = shown(&String::from_utf8_lossy(&bytes));
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    consent::allow_bytes_in(root, &file, &bytes)?;
    writeln!(out, "allowed {}", file.display())?;
    Ok(exit::OK)
}

/// `text` with each control character but newline and tab, and each
/// character that reorders text, written as `\u{..}`, so the terminal shows
/// the file as it is rather than as it would have the reader see it.
fn shown(text: &str) -> String {
    text.chars()
        .map(|c| {
            let bidi = matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            match (c.is_control() && c != '\n' && c != '\t') || bidi {
                true => format!("\\u{{{:x}}}", c as u32),
                false => c.to_string(),
            }
        })
        .collect()
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

    /// A writer that swaps `file` for other bytes the first time it is written
    /// to, as an agent could between the print and the copy.
    struct Swapping {
        file: std::path::PathBuf,
        said: Vec<u8>,
    }

    impl Write for Swapping {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.said.is_empty() {
                std::fs::write(&self.file, "on_stopped = \"swapped\"\n")?;
            }
            self.said.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn a_project_file(dir: &TempDir, text: &[u8]) -> (std::path::PathBuf, std::path::PathBuf) {
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join(".amx")).unwrap();
        let file = project.join(".amx/config.toml");
        std::fs::write(&file, text).unwrap();
        (project, file)
    }

    #[test]
    fn allow_keeps_the_bytes_it_printed_even_if_the_file_changes_meanwhile() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let (project, file) = a_project_file(&dir, b"max_agents = 2\n");
        let mut out = Swapping {
            file: file.clone(),
            said: Vec::new(),
        };

        assert_eq!(run(&root, &project, false, &mut out).unwrap(), exit::OK);
        assert!(out.said.starts_with(b"max_agents = 2\n"));
        // What was printed is what was allowed, so the swap is not.
        assert!(!consent::allowed_in(&root, &file));
        std::fs::write(&file, "max_agents = 2\n").unwrap();
        assert!(consent::allowed_in(&root, &file));
    }

    #[test]
    fn allow_shows_control_and_bidi_characters_escaped_and_keeps_them_raw() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("state/agents");
        let raw = "# safe \u{202e}evil\u{1b}[2J\tok\nmax_agents = 2\n";
        let (project, file) = a_project_file(&dir, raw.as_bytes());

        let mut out = Vec::new();
        assert_eq!(run(&root, &project, false, &mut out).unwrap(), exit::OK);
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.starts_with("# safe \\u{202e}evil\\u{1b}[2J\tok\nmax_agents = 2\n"),
            "{said}"
        );
        assert!(
            consent::allowed_in(&root, &file),
            "the raw bytes stay allowed"
        );
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
