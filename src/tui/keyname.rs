//! Parses the key spellings in the config's `keys` table (`alt+g`) into the
//! [`KeyEvent`]s a terminal sends.
//!
//! The grammar: `ctrl+` and `alt+`, each at most once and in either order,
//! then one printable character or `f1` to `f12`. Shift is written as the
//! uppercase character, because that is what a terminal sends; `shift+a` is
//! refused. A spelling outside the grammar, or a key amx already binds, is
//! refused with a message rather than guessed at.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

use super::chord;
use super::paint::HELP;

/// A key from the config and the command bound to it.
pub(in crate::tui) struct Bound {
    /// The spelling as written in the config, which the keys screen shows.
    pub(in crate::tui) spelling: String,
    /// The parsed key a press is matched against.
    pub(in crate::tui) key: KeyEvent,
    pub(in crate::tui) command: String,
}

/// Parses a key spelling, or `None` for anything outside the grammar.
///
/// Refused: whitespace, a chord with no key, a word, a repeated chord, and
/// `shift+`, since a terminal sends a shifted key as its uppercase character.
pub(in crate::tui) fn spelt(spelling: &str) -> Option<KeyEvent> {
    let mut held = KeyModifiers::NONE;
    let mut rest = spelling;
    while let Some((chord, after)) = chord_of(rest) {
        if held.contains(chord) {
            return None;
        }
        held |= chord;
        rest = after;
    }
    let key = key_of(rest)?;
    Some(KeyEvent::new(key.code, key.modifiers | held))
}

/// Splits a leading `ctrl+` or `alt+` off the spelling.
fn chord_of(spelling: &str) -> Option<(KeyModifiers, &str)> {
    spelling
        .strip_prefix("ctrl+")
        .map(|after| (KeyModifiers::CONTROL, after))
        .or_else(|| {
            spelling
                .strip_prefix("alt+")
                .map(|after| (KeyModifiers::ALT, after))
        })
}

/// Parses the key after the chords: a function key or one character.
fn key_of(spelling: &str) -> Option<KeyEvent> {
    if let Some(number) = function_key(spelling) {
        return Some(KeyEvent::new(KeyCode::F(number), KeyModifiers::NONE));
    }
    let one = one_character(spelling)?;
    // A terminal sends an uppercase letter with SHIFT set, so the parsed key
    // carries it too.
    let shift = match one.is_uppercase() {
        true => KeyModifiers::SHIFT,
        false => KeyModifiers::NONE,
    };
    Some(KeyEvent::new(KeyCode::Char(one), shift))
}

/// `f1` through `f12`.
fn function_key(spelling: &str) -> Option<u8> {
    let number: u8 = spelling.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&number).then_some(number)
}

/// The spelling as a single printable, non-whitespace character.
fn one_character(spelling: &str) -> Option<char> {
    let mut chars = spelling.chars();
    let one = chars.next()?;
    let alone = chars.next().is_none();
    (alone && !one.is_whitespace() && !one.is_control()).then_some(one)
}

/// What amx itself does on `key`, if it binds it.
///
/// Reads the key column of [`HELP`], so the keys screen and this check cannot
/// disagree. Only the code and the ctrl/alt chord are compared: shift is the
/// case of the character, so `G` and `g` are different keys.
pub(in crate::tui) fn amx_binds(key: KeyEvent) -> Option<&'static str> {
    let pressed = |bound: KeyEvent| bound.code == key.code && chord(bound) == chord(key);
    for (keys, does) in HELP {
        if keys.split_whitespace().filter_map(spelt).any(pressed) {
            return Some(does);
        }
    }
    besides(key)
}

/// Keys amx binds that the [`HELP`] key column does not spell as one token.
///
/// The column says `gg` for two presses of `g` and `alt+1..9` for a range, and
/// leaves the keys that answer a question card to the card.
fn besides(key: KeyEvent) -> Option<&'static str> {
    let held = chord(key);
    match key.code {
        KeyCode::Char('g') if held.is_empty() => does("gg G"),
        KeyCode::Char('1'..='9') if held == KeyModifiers::ALT => does("alt+1..9"),
        KeyCode::Char('1'..='9' | 'y') if held.is_empty() => Some("an answer on a question card"),
        _ => None,
    }
}

/// The [`HELP`] description for the row whose key column is exactly `keys`.
fn does(keys: &str) -> Option<&'static str> {
    HELP.iter()
        .find(|(column, _)| *column == keys)
        .map(|(_, does)| *does)
}

/// Parses the config's key table into bindings, plus a message for each
/// spelling it refused.
///
/// Keeps the table's order and each spelling as written, so the keys screen
/// shows the user's own lines back.
pub(in crate::tui) fn bound_by(keys: &BTreeMap<String, String>) -> (Vec<Bound>, Vec<String>) {
    let mut bound = Vec::new();
    let mut refused = Vec::new();
    for (spelling, command) in keys {
        let Some(key) = spelt(spelling) else {
            refused.push(format!("keys: `{spelling}` is no key the view can read"));
            continue;
        };
        // amx's own keys are matched first, so a binding on one would never
        // run.
        if let Some(does) = amx_binds(key) {
            refused.push(format!("keys: `{spelling}` is amx's own: {does}"));
            continue;
        }
        bound.push(Bound {
            spelling: spelling.clone(),
            key,
            command: command.clone(),
        });
    }
    (bound, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An expected parse result.
    fn key(code: KeyCode, modifiers: KeyModifiers) -> Option<KeyEvent> {
        Some(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn a_spelling_is_the_chords_it_is_held_with_and_the_one_key_under_them() {
        assert_eq!(
            spelt("alt+g"),
            key(KeyCode::Char('g'), KeyModifiers::ALT),
            "the shape the config file's own examples are written in"
        );
        assert_eq!(spelt("g"), key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(spelt("/"), key(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(spelt("f12"), key(KeyCode::F(12), KeyModifiers::NONE));
        assert_eq!(spelt("ctrl+f1"), key(KeyCode::F(1), KeyModifiers::CONTROL));
        assert_eq!(
            spelt("f"),
            key(KeyCode::Char('f'), KeyModifiers::NONE),
            "and an f with no number after it is the letter"
        );

        let both = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(spelt("ctrl+alt+t"), key(KeyCode::Char('t'), both));
        assert_eq!(
            spelt("alt+ctrl+t"),
            key(KeyCode::Char('t'), both),
            "either order: nobody agrees which one comes first"
        );
    }

    #[test]
    fn an_uppercase_letter_is_that_letter_with_the_shift_a_terminal_sends() {
        // Compare the modifiers directly: crossterm's `KeyEvent` equality
        // normalises a letter's case against SHIFT, so an event missing SHIFT
        // would still compare equal.
        let shifted = spelt("alt+G").expect("a letter with alt held");
        assert_eq!(shifted.code, KeyCode::Char('G'));
        assert_eq!(
            shifted.modifiers,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
            "which is what arrives when somebody presses it"
        );
        assert_eq!(
            spelt("g").expect("the same letter, unshifted").modifiers,
            KeyModifiers::NONE
        );
    }

    #[test]
    fn a_spelling_the_view_cannot_read_is_no_key_rather_than_a_guess() {
        for spelling in [
            "",
            "ctrl+",
            "alt+",
            "ctrl",
            "shift+a",
            "alt+shift+a",
            "ctrl+ctrl+a",
            "alt+ ",
            "alt+gg",
            "lazygit",
            "f0",
            "f13",
            "Alt+g",
        ] {
            assert_eq!(spelt(spelling), None, "{spelling:?} is no key");
        }
    }

    #[test]
    fn the_keys_amx_binds_are_the_ones_the_keys_screen_names() {
        // Every key the table spells answers with its row. A key named on two
        // rows (`→`) answers with the first.
        let mut seen: Vec<KeyEvent> = Vec::new();
        for (keys, does) in HELP {
            for token in keys.split_whitespace() {
                let Some(key) = spelt(token) else { continue };
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                assert_eq!(amx_binds(key), Some(does), "`{token}` is amx's own");
            }
        }

        // Keys the column does not spell as a token: `gg`, a range, and the
        // question card's answers.
        let named = |spelling: &str| amx_binds(spelt(spelling).expect("a spelling"));
        assert_eq!(
            named("g"),
            Some("the top of the list, and the foot"),
            "`gg` is two presses of a key the column cannot spell once"
        );
        assert_eq!(
            named("alt+3"),
            Some("reach one by where it is on the wall"),
            "the row names the range rather than the nine keys in it"
        );
        assert_eq!(named("7"), Some("an answer on a question card"));
        assert_eq!(
            named("y"),
            Some("an answer on a question card"),
            "which the table leaves to the card itself"
        );

        for spelling in ["alt+g", "x", "f5"] {
            assert_eq!(named(spelling), None, "`{spelling}` is nobody's yet");
        }
    }

    #[test]
    fn the_table_is_read_in_its_own_order_and_says_what_it_could_not_read() {
        let keys = BTreeMap::from([
            ("alt+g".to_string(), "lazygit".to_string()),
            ("alt+t".to_string(), "cargo test 2>&1 | less".to_string()),
            ("shift+z".to_string(), "never runs".to_string()),
        ]);
        let (bound, refused) = bound_by(&keys);

        assert_eq!(
            bound
                .iter()
                .map(|one| (one.spelling.as_str(), one.command.as_str()))
                .collect::<Vec<_>>(),
            [("alt+g", "lazygit"), ("alt+t", "cargo test 2>&1 | less")],
            "the spelling as it was written, against what it runs"
        );
        assert_eq!(
            bound[0].key,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT)
        );
        assert_eq!(
            refused,
            ["keys: `shift+z` is no key the view can read"],
            "and the one it could not read is named rather than dropped"
        );
    }

    #[test]
    fn a_spelling_amx_already_binds_is_refused_by_what_that_key_does() {
        let keys = BTreeMap::from([
            ("alt+g".to_string(), "lazygit".to_string()),
            ("ctrl+x".to_string(), "never runs".to_string()),
        ]);
        let (bound, refused) = bound_by(&keys);

        assert_eq!(
            bound
                .iter()
                .map(|one| one.spelling.as_str())
                .collect::<Vec<_>>(),
            ["alt+g"],
            "a key amx binds is bound to nothing, so nothing lists it or \
             matches it"
        );
        assert_eq!(
            refused,
            ["keys: `ctrl+x` is amx's own: stop it · again forgets · a heading, the group"],
            "and the sentence says which key of amx's somebody wrote"
        );
    }
}
