//! Roles: a named spawn recipe in a file of amx's own.
//!
//! A role is what a spawn would otherwise take on its command line — a brief
//! and the dials — written down once and asked for by name. It lives in amx's
//! own format rather than a vendor's: `<config>/amx/agents/<role>.md`, and a
//! repository's `<project>/.amx/agents/<role>.md` over it, the way a project's
//! config lays over the person's. The frontmatter is flat `key: value` between
//! `---` fences — the shape `catalog` already reads a description out of — so
//! there is no YAML parser here and no dependency to add. The body under the
//! fence is the brief.
//!
//! Nothing in a role is a lock: every value is a default the caller's own flag
//! beats, and `verbs::new` fills them into the slots the caller left empty.

use std::path::{Path, PathBuf};

/// What the directory of roles is called, under a config directory and under a
/// project's `.amx`.
const AGENTS: &str = "agents";

/// The line that opens a frontmatter, and the line that closes it.
const FENCE: &str = "---";

/// One role, as its file speaks it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Role {
    /// The name it is asked for by, which is its file's name.
    pub name: String,
    /// One line about what the role is for, for a listing.
    pub description: String,
    /// The command to run, where the role names one instead of inheriting.
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Whether the spawn cuts a tree of its own, where the role has an
    /// opinion: typed flags beat it, and the verb's own default is the last
    /// word.
    pub worktree: Option<bool>,
    /// The body under the frontmatter: what the role tells the agent.
    pub brief: String,
}

/// The roles directory beside a config directory — the person's, given the
/// directory their `config.toml` is in.
pub fn agents_under(config_dir: &Path) -> PathBuf {
    config_dir.join(AGENTS)
}

/// Where a project's roles stand, under its `.amx`.
pub fn project_dir(project: &Path) -> PathBuf {
    project.join(".amx").join(AGENTS)
}

/// The role `name`, and anything wrong with the file it came from.
///
/// The project's file answers first and whole: a name in both places is one
/// role and it is the repository's, because a role is one voice and half of
/// one voice over another is nobody's. A file that exists and is not a role
/// answers with `None` rather than falling through to the other place — the
/// repository meant to say something and what it said was wrong.
pub fn for_name(personal: &Path, project: &Path, name: &str) -> (Option<Role>, Vec<String>) {
    let mut warnings = Vec::new();
    for dir in [project, personal] {
        let path = dir.join(format!("{name}.md"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        return (read(&text, name, &path, &mut warnings), warnings);
    }
    (None, warnings)
}

/// Every role name either place holds, in one sorted list and no name twice.
pub fn names_under(personal: &Path, project: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for dir in [project, personal] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|kind| kind.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !names.iter().any(|held| held == stem) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    names
}

/// The role a file holds, or `None` from a file that is not one.
fn read(text: &str, name: &str, path: &Path, warnings: &mut Vec<String>) -> Option<Role> {
    let Some((front, body)) = split(text) else {
        warnings.push(match opens(text) {
            true => format!("{}: the frontmatter is never closed", path.display()),
            false => format!(
                "{}: not a role: it opens with no frontmatter",
                path.display()
            ),
        });
        return None;
    };
    let mut role = Role {
        name: name.to_string(),
        brief: body.trim().to_string(),
        ..Role::default()
    };
    for line in front.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            warnings.push(format!("{}: ignoring `{}`", path.display(), line.trim()));
            continue;
        };
        let value = unquoted(value.trim());
        match key.trim() {
            "description" => role.description = value.to_string(),
            "agent" => role.agent = Some(value.to_string()),
            "model" => role.model = Some(value.to_string()),
            "effort" => role.effort = Some(value.to_string()),
            "worktree" => match value {
                "true" => role.worktree = Some(true),
                "false" => role.worktree = Some(false),
                other => warnings.push(format!(
                    "{}: worktree `{other}` is neither true nor false",
                    path.display()
                )),
            },
            other => warnings.push(format!(
                "{}: ignoring unknown key `{other}`",
                path.display()
            )),
        }
    }
    Some(role)
}

/// Whether the text opens with a frontmatter fence.
fn opens(text: &str) -> bool {
    text.lines()
        .next()
        .is_some_and(|line| line.trim_end() == FENCE)
}

/// The frontmatter and the body under it, or `None` from text that is not
/// fenced.
///
/// The body is everything past the line the closing fence stands on, so a
/// `---` a brief writes for a rule of its own is that brief's business.
fn split(text: &str) -> Option<(&str, &str)> {
    if !opens(text) {
        return None;
    }
    let mut front = 0usize;
    let mut at = 0usize;
    let mut open = false;
    for line in text.split_inclusive('\n') {
        at += line.len();
        match open {
            false => {
                open = true;
                front = at;
            }
            true if line.trim_end() == FENCE => {
                return Some((&text[front..at - line.len()], &text[at..]));
            }
            true => {}
        }
    }
    None
}

/// `value` without the quotes it may be written in, which are the file's own
/// punctuation and not part of what it says.
///
/// The same law `catalog` reads a description under: a role file and a skill
/// are written by the same hands.
fn unquoted(value: &str) -> &str {
    for mark in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(mark).and_then(|v| v.strip_suffix(mark)) {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Write a role file into `dir`, making it on the way.
    fn wrote(dir: &Path, name: &str, text: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{name}.md"));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_role_reads_its_flat_keys_and_its_body() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let personal = home.path().join("amx/agents");
        let project = repo.path().join(".amx/agents");
        wrote(
            &project,
            "scout",
            "---\ndescription: fast recon\nagent: pi --approve\nmodel: opencode-go/glm-5.3\neffort: high\nworktree: true\n---\nYou are a scout.\n\nReport findings.\n",
        );

        let (role, warnings) = for_name(&personal, &project, "scout");

        assert!(warnings.is_empty(), "{warnings:?}");
        let role = role.expect("the project's role");
        assert_eq!(role.name, "scout");
        assert_eq!(role.description, "fast recon");
        assert_eq!(role.agent.as_deref(), Some("pi --approve"));
        assert_eq!(role.model.as_deref(), Some("opencode-go/glm-5.3"));
        assert_eq!(role.effort.as_deref(), Some("high"));
        assert_eq!(role.worktree, Some(true));
        assert_eq!(role.brief, "You are a scout.\n\nReport findings.");
    }

    #[test]
    fn the_projects_role_beats_the_persons_of_the_same_name() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let personal = home.path().join("amx/agents");
        let project = repo.path().join(".amx/agents");
        wrote(
            &personal,
            "scout",
            "---\ndescription: the person's\n---\nmine\n",
        );
        wrote(
            &project,
            "scout",
            "---\ndescription: the project's\n---\nours\n",
        );

        let (role, warnings) = for_name(&personal, &project, "scout");

        assert!(warnings.is_empty(), "{warnings:?}");
        let role = role.expect("the project's file answers");
        assert_eq!(role.description, "the project's");
        assert_eq!(role.brief, "ours");

        // The person's answers where the project has nothing to say.
        wrote(
            &personal,
            "helper",
            "---\ndescription: the person's\n---\nmine\n",
        );
        let (role, _) = for_name(&personal, &project, "helper");
        assert_eq!(role.expect("the person's file").brief, "mine");
    }

    #[test]
    fn an_unknown_key_warns_and_the_role_still_reads() {
        let place = TempDir::new().unwrap();
        let personal = place.path().join("amx/agents");
        let project = place.path().join("empty");
        wrote(
            &personal,
            "scout",
            "---\ndescription: fast recon\ntools: read, bash\n---\nbody\n",
        );

        let (role, warnings) = for_name(&personal, &project, "scout");

        let role = role.expect("an unknown key is not a reason to lose the role");
        assert_eq!(role.description, "fast recon");
        assert!(
            warnings.iter().any(|said| said.contains("tools")),
            "the key is named: {warnings:?}"
        );
    }

    #[test]
    fn a_file_with_no_frontmatter_is_not_a_role_and_is_named() {
        let place = TempDir::new().unwrap();
        let personal = place.path().join("amx/agents");
        let project = place.path().join("empty");
        wrote(&personal, "nope", "just words, and no fence above them\n");
        wrote(
            &personal,
            "unclosed",
            "---\ndescription: x\nno fence below\n",
        );

        let (role, warnings) = for_name(&personal, &project, "nope");
        assert!(role.is_none(), "a file with no frontmatter is not a role");
        assert!(
            warnings.iter().any(|said| said.contains("nope.md")),
            "{warnings:?}"
        );

        let (role, warnings) = for_name(&personal, &project, "unclosed");
        assert!(role.is_none(), "nor is one whose frontmatter never closes");
        assert!(
            warnings.iter().any(|said| said.contains("unclosed.md")),
            "{warnings:?}"
        );
    }

    #[test]
    fn worktree_reads_true_and_false_and_names_a_value_that_is_neither() {
        let place = TempDir::new().unwrap();
        let personal = place.path().join("amx/agents");
        let project = place.path().join("empty");
        wrote(&personal, "one", "---\nworktree: false\n---\n");
        wrote(&personal, "two", "---\nworktree: maybe\n---\n");

        let (role, warnings) = for_name(&personal, &project, "one");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(role.expect("a role").worktree, Some(false));

        let (role, warnings) = for_name(&personal, &project, "two");
        assert_eq!(role.expect("a role").worktree, None);
        assert!(
            warnings.iter().any(|said| said.contains("maybe")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_role_quoted_is_read_without_its_quotes() {
        let place = TempDir::new().unwrap();
        let personal = place.path().join("amx/agents");
        let project = place.path().join("empty");
        wrote(
            &personal,
            "scout",
            "---\nagent: \"pi --approve\"\ndescription: 'fast recon'\n---\n",
        );

        let (role, warnings) = for_name(&personal, &project, "scout");

        assert!(warnings.is_empty(), "{warnings:?}");
        let role = role.expect("a role");
        assert_eq!(role.agent.as_deref(), Some("pi --approve"));
        assert_eq!(role.description, "fast recon");
    }

    #[test]
    fn the_two_places_are_beside_the_config_and_under_the_project() {
        assert_eq!(
            agents_under(Path::new("/home/dev/.config/amx")),
            Path::new("/home/dev/.config/amx/agents")
        );
        assert_eq!(
            project_dir(Path::new("/src/app")),
            Path::new("/src/app/.amx/agents")
        );
    }

    #[test]
    fn names_under_lists_both_places_without_repeating_a_name() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let personal = home.path().join("amx/agents");
        let project = repo.path().join(".amx/agents");
        wrote(&personal, "scout", "---\n---\n");
        wrote(&personal, "helper", "---\n---\n");
        wrote(&project, "scout", "---\n---\n");
        wrote(&project, "writer", "---\n---\n");
        std::fs::write(project.join("README.txt"), "no\n").unwrap();
        assert_eq!(
            names_under(&personal, &project),
            ["helper", "scout", "writer"],
            "one name once, and only the markdown"
        );
        assert_eq!(
            names_under(&home.path().join("nowhere"), &home.path().join("neither")),
            Vec::<String>::new(),
            "no directory is no names, not a failure"
        );
    }
}
