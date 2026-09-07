//! What a vendor can be asked for by name, read out of the places it loads
//! from.
//!
//! A word typed on a task line — `/review`, `@code-reviewer` — is the vendor's
//! word and not amx's, and what answers to it is whatever is in the vendor's
//! own directories at the moment somebody types it. [`Catalog`] says where
//! those are and how what is found there is spelled; this reads them, and what
//! comes back is a list of words with a sentence each.
//!
//! Nothing here decides which of them to offer, or what a word is worth on a
//! line: that is the composer's business. A place a vendor loads from is a
//! feature nobody has to use, so a directory that is not there is silence
//! rather than a failure, and so is a file that will not read — a file the
//! vendor could not run either. Somebody typing a task is owed suggestions or
//! none, never a complaint about somebody else's directory.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::vendor::{Catalog, Place};

/// What a word names, which is what decides how it is spelled and where it
/// stands in a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Command,
    Agent,
    Builtin,
}

/// One thing a vendor can be asked for: the word that asks for it, what that
/// word names, and what the thing says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub spelled: String,
    pub kind: Kind,
    pub about: String,
}

/// The file a skill directory says what it is in. Both vendors follow the
/// same standard here: a skill is a directory, and this is the one file in it
/// that has to be there.
const SKILL: &str = "SKILL.md";

/// What a command and an agent are written in, and the only files in those
/// places that are one.
const MARKDOWN: &str = "md";

/// The segment of a place that stands for every directory at that level.
const STAR: &str = "*";

/// The line that opens a frontmatter, and the line that closes it.
const FENCE: &str = "---";

/// Everything `catalog` can be asked for on this machine, by the word that
/// asks for it.
///
/// Sorted, with the commands the vendor answers out of itself after
/// everything on disk: a person is offered their own files first, and the
/// vendor's own list is long enough to bury them. A word two places answer to
/// is offered once, by the first of them to answer — which is what the order
/// the places are declared in is for.
pub fn listing(catalog: &Catalog, home: &Path, project: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    for place in catalog.skills {
        for found in expand(place, home, project) {
            skills(&found, catalog.skill_prefix, &mut entries);
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
                    kind,
                    &mut entries,
                );
            }
        }
    }
    for builtin in catalog.builtins {
        entries.push(Entry {
            spelled: format!("/{builtin}"),
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

/// One directory a place resolves to on this machine, and the plugin it
/// belongs to when the place named one.
struct Found {
    dir: PathBuf,
    plugin: Option<String>,
}

/// Every directory a place names, under the root it hangs off.
///
/// One directory from a place spelled plainly, and one per matching directory
/// from each `*` in it — a pattern that matches nothing resolves to nothing,
/// which is the same silence as a directory that is not there.
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

/// The plugin the entries under a starred place belong to: the last directory
/// but one that a star matched.
///
/// claude keeps a plugin's files under a market, the plugin itself and the
/// version it is installed at, so the star nearest what was asked for is the
/// version and the one before it is the plugin. It is the only starred layout
/// in the table, and a vendor that keeps its plugins some other way is a
/// second measurement to take here.
fn plugin(matched: &[String]) -> Option<String> {
    matched.get(matched.len().checked_sub(2)?).cloned()
}

/// Every skill under a skills place: a directory with a [`SKILL`] in it,
/// under the word the vendor runs one by.
///
/// A directory with no such file is not a skill, and neither is a stray file
/// in among them, which is the same answer as a file that will not read: the
/// place is somebody else's, and what amx cannot read there it does not offer.
fn skills(found: &Found, prefix: &str, into: &mut Vec<Entry>) {
    for dir in contents(&found.dir) {
        let (Some(name), Some(about)) = (named(&dir), about(&dir.join(SKILL))) else {
            continue;
        };
        into.push(Entry {
            spelled: match &found.plugin {
                Some(plugin) => format!("/{plugin}:{name}"),
                None => format!("/{prefix}{name}"),
            },
            kind: Kind::Skill,
            about,
        });
    }
}

/// Every markdown file under a commands or an agents place, walked into the
/// directories it keeps them in.
fn walk(
    dir: &Path,
    plugin: Option<&str>,
    under: &mut Vec<String>,
    kind: Kind,
    into: &mut Vec<Entry>,
) {
    for path in contents(dir) {
        let Some(name) = named(&path) else { continue };
        if path.is_dir() {
            under.push(name.to_string());
            walk(&path, plugin, under, kind, into);
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
            spelled: spell(kind, plugin, under, stem),
            kind,
            about,
        });
    }
}

/// The word a file in one of these places is asked for by.
fn spell(kind: Kind, plugin: Option<&str>, under: &[String], name: &str) -> String {
    match kind {
        // An agent is named by itself: the agents places measured hold no
        // plugins and no directories to qualify one with.
        Kind::Agent => format!("@{name}"),
        // A command carries whatever stands above it, joined the way the
        // vendor namespaces one: the plugin it came with, and the directories
        // it sits under.
        _ => {
            let mut words: Vec<&str> = plugin.into_iter().collect();
            words.extend(under.iter().map(String::as_str));
            words.push(name);
            format!("/{}", words.join(":"))
        }
    }
}

/// What is in `dir`, by name, and nothing at all from a directory that is not
/// there or will not be read.
///
/// By name, so what a machine offers does not depend on the order a
/// filesystem happens to hand its entries back: two places answering to the
/// same word are settled by which place was read first, and two plugins by
/// which is called what.
fn contents(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

/// The last part of a path, when it is a name amx can write down.
fn named(path: &Path) -> Option<&str> {
    path.file_name().and_then(OsStr::to_str)
}

/// What a file says about itself, or nothing from a file that says nothing.
///
/// `None` from a file that cannot be read at all — missing, a directory, or
/// bytes that are not text — which is a file the vendor could not run either.
fn about(path: &Path) -> Option<String> {
    Some(description(&std::fs::read_to_string(path).ok()?))
}

/// The `description` of a frontmatter, if the text opens with one.
///
/// The one key amx reads out of a file it otherwise leaves alone, so this is
/// a look at the first few lines rather than a parse: the value is what
/// follows the colon, with the quotes a file may write it in taken off. A key
/// that stands under another one is that key's, and a `description:` past the
/// closing fence is the file's own words about whatever it likes.
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

/// `value` without the quotes it may be written in, which are the file's own
/// punctuation and not part of what it says.
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
    use crate::vendor::{claude, pi};
    use tempfile::TempDir;

    #[test]
    fn claudes_places_are_read_into_the_words_that_ask_for_them() {
        // claude's layout, measured: a skill is a directory saying what it is
        // in SKILL.md, a command and an agent are files, and a plugin's files
        // sit under a market, the plugin and the version it is installed at.
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
            spellings(&entries),
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
        // The same reading against the other vendor's layout: four skills
        // places under two roots, prompts where claude keeps commands, no
        // agents at all, and a list of commands pi answers out of itself.
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

        // What the vendor answers out of itself comes after everything on
        // disk: a person is offered their own files first.
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
    fn a_place_that_is_not_there_offers_nothing_and_says_nothing() {
        // Nobody has to have any of these directories, and most people have
        // some of them. Two empty roots are the machine that ends up asking
        // for the whole catalog and finding none of it.
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        assert!(
            listing(
                &claude::VENDOR.catalog.unwrap(),
                home.path(),
                project.path()
            )
            .is_empty()
        );
        assert!(
            listing(&pi::VENDOR.catalog.unwrap(), home.path(), project.path())
                .iter()
                .all(|entry| entry.kind == Kind::Builtin),
            "except what the vendor answers out of itself, which is in no \
             directory to be missing from"
        );
    }

    #[test]
    fn a_file_that_will_not_read_is_left_out_and_one_that_says_nothing_is_not() {
        // The two are different: a file amx cannot read is a file it knows
        // nothing about, including whether the vendor would offer it. A file
        // with no description is a file the vendor offers with nothing to say
        // about it.
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
            spellings(&entries),
            ["/plain", "/quiet"],
            "the file that would not read is out, and the one with nothing to \
             say is in; a file that is not markdown was never a command"
        );
        assert_eq!(found(&entries, "/plain").about, "");
        assert_eq!(found(&entries, "/quiet").about, "");
    }

    #[test]
    fn a_word_two_places_answer_to_is_offered_once() {
        // The places are read in the order the vendor reads them, so the
        // first to answer to a word keeps it. Two of the same word in a list
        // of suggestions is a choice between two things that look identical.
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
        // Both spellings the measured files use, quoted and bare, and a
        // description is a description of the file only while the
        // frontmatter it stands in is open.
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

    /// A file saying `about` about itself, with the directories above it.
    fn file(path: &Path, about: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("---\ndescription: {about}\n---\n\nwords\n")).unwrap();
    }

    /// The words a listing offers, in the order it offers them.
    fn spellings(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.spelled.as_str()).collect()
    }

    /// The one entry `spelled` asks for.
    fn found<'a>(entries: &'a [Entry], spelled: &str) -> &'a Entry {
        entries
            .iter()
            .find(|entry| entry.spelled == spelled)
            .unwrap_or_else(|| panic!("nothing is offered as {spelled}"))
    }
}
