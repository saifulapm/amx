//! The command line an installed hook runs, and how amx recognises its own.
//!
//! An installed entry is one string:
//!
//! ```text
//! /home/you/.local/bin/amx _hook claude --marker 1
//! ```
//!
//! and every question the rest of this directory asks about ownership is a
//! question about that string. It is split out from [`super::hooks`] because it
//! is a *grammar* — write it, read it back, decide whose it is — with no
//! knowledge of the JSON it happens to be stored in.
//!
//! # Ownership is the shape, not the path
//!
//! An entry belongs to amx when its first argument is `_hook` and its second is
//! the agent id: an invocation of the hidden subcommand in
//! `crates/amx/src/cmd/hook.rs` and of nothing else. Nothing but amx spells
//! `_hook`, which is why the leading underscore hides it from the CLI's help.
//!
//! Ownership deliberately does *not* look at the program. An installation made
//! by a copy of amx that has since moved is still amx's entry — precisely the
//! entry `status` has to find and call broken, rather than leave behind as a
//! foreign hook nobody will ever clean up. herdr's `integration status` greps a
//! version comment and reports `current` for an installation that silently does
//! nothing; the marker plus the binary check exist to make that state visible.

use std::path::{Path, PathBuf};

use amx_core::agent::AgentKind;

/// The hidden subcommand every installed hook runs.
const HOOK_VERB: &str = "_hook";

/// The flag carrying the installed asset's version marker.
const MARKER_FLAG: &str = "--marker";

/// The command string an installed entry runs.
///
/// The program is an absolute path rather than the bare word `amx`: a hook
/// process inherits the *agent's* environment (V01 §3 M6), and an agent
/// launched from a desktop session or a login shell need not have amx's
/// directory on `PATH`. A hook that cannot be found is the silent no-op this
/// whole verb exists to delete.
#[must_use]
pub fn build(program: &Path, agent: &AgentKind, marker: u32) -> String {
    format!(
        "{} {HOOK_VERB} {} {MARKER_FLAG} {marker}",
        quote(&program.display().to_string()),
        agent.as_str()
    )
}

/// The program and marker `command` names, if it is an amx entry for `agent`.
#[must_use]
pub fn parse(command: &str, agent: &AgentKind) -> Option<(PathBuf, Option<u32>)> {
    let words = words(command);
    if words.get(1).map(String::as_str) != Some(HOOK_VERB) {
        return None;
    }
    if words.get(2).map(String::as_str) != Some(agent.as_str()) {
        return None;
    }
    let marker = words
        .iter()
        .position(|word| word == MARKER_FLAG)
        .and_then(|at| words.get(at + 1))
        .and_then(|value| value.parse().ok());
    Some((PathBuf::from(words.first()?), marker))
}

/// `text` as one shell word.
///
/// The command is a string the agent hands to a shell, so a program path with a
/// space in it — `/Applications/My Tools/amx` — has to survive as one argument.
/// Single quotes are the only POSIX quoting that needs no escape table: inside
/// them every byte is literal, and the single quote itself is closed, escaped
/// and reopened.
#[must_use]
pub fn quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', r#"'"'"'"#))
}

/// Split a command string into words, undoing [`quote`].
///
/// Both quote characters are honoured, and each is literal inside the other —
/// which is exactly the rule that makes [`quote`]'s `'"'"'` escape work, so a
/// path containing an apostrophe survives the round trip. Nothing else is
/// understood: no backslashes, no expansions, no operators. That is enough to
/// recognise amx's own entries, which is all this is for. A foreign command
/// written with anything richer splits into words that do not look like
/// `_hook`, so it stays foreign and is left alone — the safe direction for the
/// mistake to fall in.
#[must_use]
pub fn words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut single = false;
    let mut double = false;
    let mut started = false;
    for c in command.chars() {
        match c {
            '\'' if !double => {
                single = !single;
                started = true;
            }
            '"' if !single => {
                double = !double;
                started = true;
            }
            c if c.is_whitespace() && !single && !double => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

    use std::path::Path;

    use amx_core::agent::AgentKind;

    use super::{build, parse, quote, words};

    fn claude() -> AgentKind {
        AgentKind::new("claude").expect("a valid id")
    }

    #[test]
    fn a_path_with_a_space_survives_the_round_trip_as_one_word() {
        let line = build(Path::new("/opt/My Tools/amx"), &claude(), 1);
        assert_eq!(line, "'/opt/My Tools/amx' _hook claude --marker 1");
        assert_eq!(words(&line)[0], "/opt/My Tools/amx");
        let (program, marker) = parse(&line, &claude()).expect("amx's own entry");
        assert_eq!(program, Path::new("/opt/My Tools/amx"));
        assert_eq!(marker, Some(1));
    }

    #[test]
    fn a_quote_in_a_path_is_closed_escaped_and_reopened() {
        let quoted = quote("/home/it's/amx");
        assert_eq!(quoted, r#"'/home/it'"'"'s/amx'"#);
        assert_eq!(words(&quoted), ["/home/it's/amx"]);
    }

    #[test]
    fn an_ordinary_path_is_left_unquoted() {
        assert_eq!(quote("/usr/bin/amx"), "/usr/bin/amx");
    }

    #[test]
    fn a_foreign_command_is_never_amxs() {
        let agent = claude();
        assert!(parse("/usr/bin/notify-send hi", &agent).is_none());
        // The right shape for a different agent is still not this agent's.
        assert!(parse("/usr/bin/amx _hook codex --marker 1", &agent).is_none());
        // An entry from an amx that has moved is still this agent's entry —
        // the one `status` must find and call broken.
        assert!(parse("/gone/amx _hook claude --marker 1", &agent).is_some());
    }

    #[test]
    fn an_entry_with_no_marker_parses_and_says_so() {
        let (_, marker) = parse("/usr/bin/amx _hook claude", &claude()).expect("an amx entry");
        assert_eq!(marker, None);
    }
}
