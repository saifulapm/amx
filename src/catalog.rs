//! Lists the skills, commands and agents a vendor can be asked for by name,
//! read from the directories its [`Catalog`] declares, for task-line
//! completion (`/review`, `@code-reviewer`).
//!
//! Missing directories and unreadable files are skipped silently; which
//! entries to offer is the composer's decision.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::vendor::{Catalog, Place};

/// What a catalog word names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Command,
    Agent,
    Builtin,
}

/// One catalog word: its spelling on a task line, its kind, and its
/// description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub spelled: String,
    pub kind: Kind,
    pub about: String,
}

/// The file that makes a directory a skill.
const SKILL: &str = "SKILL.md";

/// The extension of command and agent files.
const MARKDOWN: &str = "md";

/// A place segment that matches every directory at its level.
const STAR: &str = "*";

/// The line that opens a frontmatter, and the line that closes it.
pub const FENCE: &str = "---";

/// Every word `catalog` offers on this machine, sorted, with built-ins after
/// everything found on disk.
///
/// A word found in more than one place is kept once, from the first place in
/// declaration order.
pub fn listing(catalog: &Catalog, home: &Path, project: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    for place in catalog.skills {
        for found in expand(place, home, project) {
            skills(&found, catalog, &mut entries);
        }
    }
    for (places, kind) in [
        (catalog.commands, Kind::Command),
        (catalog.agents, Kind::Agent),
    ] {
        for place in places {
            for found in expand(place, home, project) {
                walk(
                    &found.dir,
                    found.plugin.as_deref(),
                    &mut Vec::new(),
                    &mut Vec::new(),
                    kind,
                    catalog.sigil,
                    &mut entries,
                );
            }
        }
    }
    for builtin in catalog.builtins {
        entries.push(Entry {
            spelled: format!("{}{builtin}", catalog.sigil),
            kind: Kind::Builtin,
            about: String::new(),
        });
    }

    let mut spoken = HashSet::new();
    entries.retain(|entry| spoken.insert(entry.spelled.clone()));
    entries.sort_by(|one, two| {
        (one.kind == Kind::Builtin, &one.spelled).cmp(&(two.kind == Kind::Builtin, &two.spelled))
    });
    entries
}

/// A directory a place resolved to, and the plugin it belongs to, if any.
struct Found {
    dir: PathBuf,
    plugin: Option<String>,
}

/// Every directory `place` resolves to under its root, expanding each `*`
/// segment to the directories that exist there.
fn expand(place: &Place, home: &Path, project: &Path) -> Vec<Found> {
    let root = match place {
        Place::Person(_) => home,
        Place::Project(_) => project,
    };

    let mut found = vec![(root.to_path_buf(), Vec::new())];
    for segment in place.path().split('/') {
        let mut next = Vec::new();
        for (dir, matched) in found {
            if segment != STAR {
                next.push((dir.join(segment), matched));
                continue;
            }
            for child in contents(&dir) {
                let Some(name) = named(&child) else { continue };
                if child.is_dir() {
                    let mut matched = matched.clone();
                    matched.push(name.to_string());
                    next.push((child, matched));
                }
            }
        }
        found = next;
    }

    found
        .into_iter()
        .map(|(dir, matched)| Found {
            dir,
            plugin: plugin(&matched),
        })
        .collect()
}

/// The plugin name from the directories a starred place matched: the
/// second-to-last one.
///
/// Written for claude's plugin cache, `<market>/<plugin>/<version>`, the only
/// starred layout in the table.
fn plugin(matched: &[String]) -> Option<String> {
    matched.get(matched.len().checked_sub(2)?).cloned()
}

/// Every skill under a skills place: each subdirectory holding a readable
/// [`SKILL`].
fn skills(found: &Found, catalog: &Catalog, into: &mut Vec<Entry>) {
    let (sigil, prefix) = (catalog.sigil, catalog.skill_prefix);
    for dir in contents(&found.dir) {
        let (Some(name), Some(about)) = (named(&dir), about(&dir.join(SKILL))) else {
            continue;
        };
        into.push(Entry {
            spelled: match &found.plugin {
                Some(plugin) => format!("{sigil}{plugin}:{name}"),
                None => format!("{sigil}{prefix}{name}"),
            },
            kind: Kind::Skill,
            about,
        });
    }
}

/// Every markdown file under a commands or agents place, recursively.
///
/// `walking` holds the real path of each directory on the way down, so a
/// symlink back to one of them is not followed again.
fn walk(
    dir: &Path,
    plugin: Option<&str>,
    under: &mut Vec<String>,
    walking: &mut Vec<PathBuf>,
    kind: Kind,
    sigil: char,
    into: &mut Vec<Entry>,
) {
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if walking.contains(&real) {
        return;
    }
    walking.push(real);
    for path in contents(dir) {
        let Some(name) = named(&path) else { continue };
        if path.is_dir() {
            under.push(name.to_string());
            walk(&path, plugin, under, walking, kind, sigil, into);
            under.pop();
            continue;
        }
        if path.extension() != Some(OsStr::new(MARKDOWN)) {
            continue;
        }
        let (Some(stem), Some(about)) = (path.file_stem().and_then(OsStr::to_str), about(&path))
        else {
            continue;
        };
        into.push(Entry {
            spelled: spell(kind, sigil, plugin, under, stem),
            kind,
            about,
        });
    }
    walking.pop();
}

/// The word a command or agent file is asked for by.
fn spell(kind: Kind, sigil: char, plugin: Option<&str>, under: &[String], name: &str) -> String {
    match kind {
        // No measured agents place holds plugins or subdirectories.
        Kind::Agent => format!("@{name}"),
        // A command is namespaced by its plugin and subdirectories.
        _ => {
            let mut words: Vec<&str> = plugin.into_iter().collect();
            words.extend(under.iter().map(String::as_str));
            words.push(name);
            format!("{sigil}{}", words.join(":"))
        }
    }
}

/// The entries of `dir`, sorted by path, or none if it cannot be read.
///
/// Sorted so the result does not depend on the filesystem's order.
fn contents(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

/// The file name of `path`, if it is UTF-8.
fn named(path: &Path) -> Option<&str> {
    path.file_name().and_then(OsStr::to_str)
}

/// The frontmatter description of the file at `path`, or `None` if the file
/// cannot be read as text.
fn about(path: &Path) -> Option<String> {
    Some(description(&std::fs::read_to_string(path).ok()?))
}

/// The `description:` value from the text's frontmatter, unquoted, or empty.
///
/// A line scan rather than a YAML parse: only an unindented key inside the
/// opening fence counts.
fn description(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next() != Some(FENCE) {
        return String::new();
    }
    lines
        .take_while(|line| *line != FENCE)
        .find_map(|line| line.strip_prefix("description:"))
        .map(|value| unquoted(value.trim()).to_string())
        .unwrap_or_default()
}

/// `value` without surrounding double or single quotes.
pub fn unquoted(value: &str) -> &str {
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
    use crate::vendor::{claude, codex, pi};
    use tempfile::TempDir;

    #[test]
    fn claudes_places_are_read_into_the_words_that_ask_for_them() {
        // claude's layout: skill directories with SKILL.md, command and agent
        // files, and plugins under <market>/<plugin>/<version>.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let plugin = home.path().join(".claude/plugins/cache/market/focus/2.8.0");

        file(
            &home.path().join(".claude/skills/review/SKILL.md"),
            "Read the diff.",
        );
        std::fs::create_dir_all(home.path().join(".claude/skills/notes")).unwrap();
        file(&home.path().join(".claude/commands/ship.md"), "Ship it.");
        file(
            &home.path().join(".claude/commands/git/sync.md"),
            "Catch the branch up.",
        );
        file(
            &home.path().join(".claude/agents/code-reviewer.md"),
            "Reads for correctness.",
        );
        file(&plugin.join("skills/focus/SKILL.md"), "Hold the plan.");
        file(&plugin.join("commands/status.md"), "Say where the plan is.");
        file(
            &project.path().join(".claude/skills/deploy/SKILL.md"),
            "Push it out.",
        );
        file(
            &project.path().join(".claude/commands/test.md"),
            "Run the suite.",
        );
        file(
            &project.path().join(".claude/agents/scout.md"),
            "Goes and looks.",
        );

        let entries = listing(
            &claude::VENDOR.catalog.expect("claude loads files by name"),
            home.path(),
            project.path(),
        );
        assert_eq!(
            on_disk(&entries),
            [
                "/deploy",
                "/focus:focus",
                "/focus:status",
                "/git:sync",
                "/review",
                "/ship",
                "/test",
                "@code-reviewer",
                "@scout",
            ],
            "the plugin is the middle of the three directories a star matched, \
             never the market it came from or the version it is installed at, \
             and a skills directory with no SKILL.md in it is no skill"
        );

        // `/review` is also a claude built-in; the person's skill wins.
        assert_eq!(found(&entries, "/review").kind, Kind::Skill);
        assert_eq!(found(&entries, "/review").about, "Read the diff.");
        assert_eq!(found(&entries, "/deploy").kind, Kind::Skill);
        assert_eq!(found(&entries, "/focus:focus").kind, Kind::Skill);
        assert_eq!(found(&entries, "/focus:status").kind, Kind::Command);
        assert_eq!(found(&entries, "/git:sync").kind, Kind::Command);
        assert_eq!(found(&entries, "/git:sync").about, "Catch the branch up.");
        assert_eq!(found(&entries, "@scout").kind, Kind::Agent);
        assert_eq!(
            found(&entries, "@code-reviewer").about,
            "Reads for correctness."
        );
    }

    #[test]
    fn pi_spells_a_skill_its_own_way_and_answers_its_own_commands_last() {
        // pi's layout: four skills places, prompts for commands, no agents.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        file(
            &home.path().join(".pi/agent/skills/mem/SKILL.md"),
            "Keep what was decided.",
        );
        file(
            &home.path().join(".agents/skills/desktop/SKILL.md"),
            "Drive the screen.",
        );
        file(
            &home.path().join(".pi/agent/prompts/review.md"),
            "Look for bugs.",
        );
        file(
            &project.path().join(".pi/skills/deploy/SKILL.md"),
            "Push it out.",
        );
        file(
            &project.path().join(".agents/skills/scout/SKILL.md"),
            "Go and look.",
        );
        file(&project.path().join(".pi/prompts/ship.md"), "Ship it.");

        let catalog = pi::VENDOR.catalog.expect("pi loads files by name");
        let entries = listing(&catalog, home.path(), project.path());

        let files = &entries[..6];
        assert_eq!(
            spellings(files),
            [
                "/review",
                "/ship",
                "/skill:deploy",
                "/skill:desktop",
                "/skill:mem",
                "/skill:scout",
            ],
            "pi runs a skill as /skill:name and a prompt as /name"
        );
        assert_eq!(found(&entries, "/skill:mem").kind, Kind::Skill);
        assert_eq!(
            found(&entries, "/skill:mem").about,
            "Keep what was decided."
        );
        assert_eq!(found(&entries, "/ship").kind, Kind::Command);
        assert!(
            !entries.iter().any(|entry| entry.kind == Kind::Agent),
            "pi ships no sub agents, so nothing in its places is one"
        );

        // Built-ins come after everything on disk.
        let mut builtins: Vec<String> = catalog.builtins.iter().map(|b| format!("/{b}")).collect();
        builtins.sort();
        assert_eq!(spellings(&entries[6..]), builtins);
        assert!(
            entries[6..].iter().all(|entry| entry.kind == Kind::Builtin),
            "and nothing on disk is left among them"
        );
        assert_eq!(
            found(&entries, "/compact").about,
            "",
            "a builtin is in no file to read one from"
        );
    }

    #[test]
    fn codex_spells_a_skill_behind_its_own_sigil_out_of_four_places() {
        // codex runs a skill as `$name`. Skills nested deeper than one level
        // are not read, as for claude and pi.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        file(
            &home.path().join(".agents/skills/desktop/SKILL.md"),
            "Drive the screen.",
        );
        file(
            &home.path().join(".codex/skills/mem/SKILL.md"),
            "Keep what was decided.",
        );
        file(
            &project.path().join(".codex/skills/deploy/SKILL.md"),
            "Push it out.",
        );
        file(
            &project.path().join(".agents/skills/scout/SKILL.md"),
            "Go and look.",
        );
        file(
            &project.path().join(".agents/skills/tools/lint/SKILL.md"),
            "Nested a level down.",
        );
        file(
            &home.path().join(".claude/skills/review/SKILL.md"),
            "claude's.",
        );

        let entries = listing(
            &codex::VENDOR.catalog.expect("codex loads skills by name"),
            home.path(),
            project.path(),
        );
        assert_eq!(
            spellings(&entries),
            ["$deploy", "$desktop", "$mem", "$scout"],
            "a bare name behind `$`, and nothing out of another vendor's places"
        );
        assert!(entries.iter().all(|entry| entry.kind == Kind::Skill));
        assert_eq!(found(&entries, "$mem").about, "Keep what was decided.");
    }

    #[test]
    fn a_place_that_is_not_there_offers_nothing_and_says_nothing() {
        // Two empty roots: only built-ins are offered.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        for catalog in [claude::VENDOR.catalog.unwrap(), pi::VENDOR.catalog.unwrap()] {
            assert!(
                listing(&catalog, home.path(), project.path())
                    .iter()
                    .all(|entry| entry.kind == Kind::Builtin),
                "nothing but what the vendor answers out of itself, which is \
                 in no directory to be missing from"
            );
        }
    }

    #[test]
    fn a_file_that_will_not_read_is_left_out_and_one_that_says_nothing_is_not() {
        // An unreadable file is left out; a file without a description is
        // offered with an empty one.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        std::fs::create_dir_all(home.path().join(".claude/skills/broken")).unwrap();
        std::fs::write(
            home.path().join(".claude/skills/broken/SKILL.md"),
            [b'-', b'-', b'-', b'\n', 0xff, 0xfe, b'\n'],
        )
        .unwrap();
        file(&home.path().join(".claude/skills/quiet/SKILL.md"), "");
        std::fs::create_dir_all(home.path().join(".claude/commands")).unwrap();
        std::fs::write(
            home.path().join(".claude/commands/plain.md"),
            "Just the words, with no frontmatter above them.\n",
        )
        .unwrap();
        std::fs::write(
            home.path().join(".claude/commands/notes.txt"),
            "not a command",
        )
        .unwrap();

        let entries = listing(
            &claude::VENDOR.catalog.unwrap(),
            home.path(),
            project.path(),
        );
        assert_eq!(
            on_disk(&entries),
            ["/plain", "/quiet"],
            "the file that would not read is out, and the one with nothing to \
             say is in; a file that is not markdown was never a command"
        );
        assert_eq!(found(&entries, "/plain").about, "");
        assert_eq!(found(&entries, "/quiet").about, "");
    }

    #[test]
    fn a_directory_linked_back_to_one_being_walked_is_walked_once() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let commands = home.path().join(".claude/commands");
        file(&commands.join("ship.md"), "Ship it.");
        std::os::unix::fs::symlink(&commands, commands.join("again")).unwrap();
        std::os::unix::fs::symlink(&commands, commands.join("more")).unwrap();

        let entries = listing(
            &claude::VENDOR.catalog.unwrap(),
            home.path(),
            project.path(),
        );
        assert_eq!(on_disk(&entries), ["/ship"]);
    }

    #[test]
    fn a_word_two_places_answer_to_is_offered_once() {
        // The first place in declaration order keeps a word both places have.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        file(
            &home.path().join(".pi/agent/skills/mem/SKILL.md"),
            "The person's own.",
        );
        file(
            &project.path().join(".pi/skills/mem/SKILL.md"),
            "The project's.",
        );
        file(
            &home.path().join(".pi/agent/prompts/compact.md"),
            "A prompt of my own.",
        );

        let entries = listing(&pi::VENDOR.catalog.unwrap(), home.path(), project.path());
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.spelled == "/skill:mem")
                .count(),
            1
        );
        assert_eq!(found(&entries, "/skill:mem").about, "The person's own.");
        assert_eq!(
            found(&entries, "/compact").kind,
            Kind::Command,
            "and a file answering to a word the vendor also answers to is \
             read first, since it is what the person put there"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.spelled == "/compact")
                .count(),
            1
        );
    }

    #[test]
    fn the_description_in_a_frontmatter_is_what_a_suggestion_says() {
        // Quoted and bare values, and only inside the frontmatter.
        assert_eq!(
            description("---\nname: x\ndescription: Read the diff.\n---\n"),
            "Read the diff."
        );
        assert_eq!(
            description("---\ndescription: \"Run amx: one agent a task.\"\n---\n"),
            "Run amx: one agent a task.",
            "quotes are the file's own punctuation and not part of what it says"
        );
        assert_eq!(
            description("---\ndescription: 'Go and look.'\n---\n"),
            "Go and look."
        );
        assert_eq!(
            description("# A skill with no frontmatter\n\ndescription: no\n"),
            ""
        );
        assert_eq!(
            description("---\nname: x\n---\n\ndescription: below the fence\n"),
            "",
            "past the closing fence is the file's own words about anything"
        );
        assert_eq!(
            description("---\ntools:\n  description: somebody else's key\n---\n"),
            "",
            "and an indented one belongs to whatever it is under"
        );
        assert_eq!(description(""), "");
    }

    /// Write a file whose frontmatter description is `about`, creating its
    /// parent directories.
    fn file(path: &Path, about: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("---\ndescription: {about}\n---\n\nwords\n")).unwrap();
    }

    /// The spellings of `entries`, in order.
    fn spellings(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.spelled.as_str()).collect()
    }

    /// The spellings of the entries found on disk, built-ins excluded.
    fn on_disk(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .filter(|entry| entry.kind != Kind::Builtin)
            .map(|entry| entry.spelled.as_str())
            .collect()
    }

    /// The entry spelled `spelled`.
    fn found<'a>(entries: &'a [Entry], spelled: &str) -> &'a Entry {
        entries
            .iter()
            .find(|entry| entry.spelled == spelled)
            .unwrap_or_else(|| panic!("nothing is offered as {spelled}"))
    }
}
