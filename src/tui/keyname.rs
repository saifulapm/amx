//! A key as somebody writes it in a file, read into the key a terminal sends.
//!
//! The config file binds commands to keys, and a key in a file is a word:
//! `alt+g`. What arrives at the view is a [`KeyEvent`]. This is the one place
//! the word is turned into the event, so every reader of the table agrees on
//! what `alt+g` means and nobody spells a key twice.
//!
//! The grammar is the smallest one that covers the keys a person would bind:
//! the two chords the view itself reads, in either order, and then one key.
//! Shift is not one of them — a terminal says shift by sending the character
//! it typed, so `A` is the spelling of shift and `shift+a` is not a spelling
//! at all.
//!
//! A spelling this cannot read is refused rather than guessed at. A key nobody
//! can press would sit in the file looking bound, so the view says which one
//! it was and goes on without it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

/// A key somebody bound, and the command they bound to it.
pub(in crate::tui) struct Bound {
    /// The spelling as the file has it, which is what the keys screen shows:
    /// somebody looking for a key of their own is looking for the word they
    /// wrote.
    pub(in crate::tui) spelling: String,
    /// The key that spelling is, which is what a press is matched against.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the press in a later task")
    )]
    pub(in crate::tui) key: KeyEvent,
    pub(in crate::tui) command: String,
}

/// The key this spelling names, where it names one.
///
/// `ctrl+` and `alt+` in either order, each at most once, and then the key
/// they are held with: one printable character, or one of the function keys.
/// Everything else is nothing — whitespace, a chord with no key under it, a
/// word, and `shift+`, which is not how a terminal sends a shifted key.
pub(in crate::tui) fn spelt(spelling: &str) -> Option<KeyEvent> {
    let mut held = KeyModifiers::NONE;
    let mut rest = spelling;
    while let Some((chord, after)) = chord_of(rest) {
        // The same chord twice is somebody writing rather than binding.
        if held.contains(chord) {
            return None;
        }
        held |= chord;
        rest = after;
    }
    let key = key_of(rest)?;
    Some(KeyEvent::new(key.code, key.modifiers | held))
}

/// The chord this spelling opens with, and what is left after it.
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

/// The key under the chords: a function key, or the one character somebody
/// typed to get it.
fn key_of(spelling: &str) -> Option<KeyEvent> {
    if let Some(number) = function_key(spelling) {
        return Some(KeyEvent::new(KeyCode::F(number), KeyModifiers::NONE));
    }
    let one = one_character(spelling)?;
    // An uppercase letter arrives as that letter with shift held, because that
    // is the key somebody pressed to send it. So the spelling of it carries
    // the shift the terminal will.
    let shift = match one.is_uppercase() {
        true => KeyModifiers::SHIFT,
        false => KeyModifiers::NONE,
    };
    Some(KeyEvent::new(KeyCode::Char(one), shift))
}

/// `f1` through `f12`, which are the function keys a keyboard has a row of.
fn function_key(spelling: &str) -> Option<u8> {
    let number: u8 = spelling.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&number).then_some(number)
}

/// The one printable character a spelling is, where it is one.
///
/// One: `gg` is two presses rather than a key. Printable: a spelling made of a
/// space or a tab is a line somebody left something out of.
fn one_character(spelling: &str) -> Option<char> {
    let mut chars = spelling.chars();
    let one = chars.next()?;
    let alone = chars.next().is_none();
    (alone && !one.is_whitespace() && !one.is_control()).then_some(one)
}

/// The table read into the keys it binds, and a sentence for every spelling it
/// could not read.
///
/// In the table's own order, under the spelling as it was written: the keys
/// screen shows both back, and a person looking for what they bound should
/// find the line they wrote.
pub(in crate::tui) fn bound_by(keys: &BTreeMap<String, String>) -> (Vec<Bound>, Vec<String>) {
    let mut bound = Vec::new();
    let mut refused = Vec::new();
    for (spelling, command) in keys {
        match spelt(spelling) {
            Some(key) => bound.push(Bound {
                spelling: spelling.clone(),
                key,
                command: command.clone(),
            }),
            None => refused.push(format!("keys: `{spelling}` is no key the view can read")),
        }
    }
    (bound, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key that spelling is, for a test that names one.
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
        // The modifiers themselves rather than the event: crossterm compares
        // two key events with the case of a letter and the shift on it made
        // to agree, so an event that had lost the shift would still stand
        // equal to one that has it.
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
}
