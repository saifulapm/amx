//! The key-combo grammar of 04 §8: `ctrl+h`, `f1`, `alt+shift+tab`.
//!
//! `pane.send_keys` carries combos as strings and this is where they stop being
//! strings. Parsing is pure and happens on the connection task, before the pane
//! is touched at all, so a combo this build cannot read is `INVALID_PARAMS`
//! naming the offending element and *nothing* reaches the child — a partially
//! delivered burst of keystrokes is worse than a refused one.
//!
//! What comes out is a [`KeyStroke`]: a physical key, the modifiers held with
//! it, and the text the key produces if it produces any. Encoding is
//! deliberately *not* here — it belongs to the pane's parser thread, which owns
//! the terminal whose DECCKM, keypad and Kitty-keyboard state decide what the
//! bytes are ([`super::drive`]).
//!
//! The vocabulary is small on purpose, and its names are the ones a script
//! writer would try first. A key the table does not name is still reachable as
//! itself: any single character is that character.

use amx_vt::{Key, Mods};
use thiserror::Error;

/// One parsed combo, ready for the encoder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyStroke {
    /// The physical key, layout-independent.
    pub key: Key,
    /// The modifiers held with it.
    pub mods: Mods,
    /// The text the key produces before any Ctrl or Meta transformation, which
    /// is what `ghostty_key_event_set_utf8` documents it wants; `None` for keys
    /// that produce none (`f1`, `enter`, arrows).
    pub text: Option<String>,
}

/// A combo this build cannot read.
///
/// Every variant names the offending element rather than the whole combo: the
/// caller has the combo already, and what it cannot work out on its own is
/// *which* part of it amx did not recognise.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum KeyParseError {
    /// The combo, or a segment of it, was empty.
    #[error("empty key combo")]
    Empty,
    /// A modifier name nothing in the table matches.
    #[error("unknown modifier `{0}`")]
    UnknownModifier(String),
    /// A key name nothing in the table matches, and which is not one character.
    #[error("unknown key `{0}`")]
    UnknownKey(String),
}

impl KeyStroke {
    /// Read one combo: `[modifier+]* key`.
    ///
    /// Modifier and key names are case-insensitive; a single character is
    /// itself, so `a`, `A` (which carries Shift) and `€` all name a key without
    /// the table having to.
    ///
    /// # Errors
    ///
    /// [`KeyParseError`] naming the element that could not be read.
    pub fn parse(combo: &str) -> Result<Self, KeyParseError> {
        let mut parts: Vec<&str> = combo.split('+').collect();
        // Two empty tail segments mean the plus key itself rather than a
        // dangling separator: `ctrl++` is Ctrl with `+`, and `+` alone is `+`.
        // One empty tail segment — `ctrl+` — is the dangling separator, and
        // stays an error below.
        if parts.len() >= 2
            && parts[parts.len() - 1].is_empty()
            && parts[parts.len() - 2].is_empty()
        {
            parts.pop();
            if let Some(last) = parts.last_mut() {
                *last = "+";
            }
        }
        let Some((name, modifiers)) = parts.split_last() else {
            return Err(KeyParseError::Empty);
        };
        if name.is_empty() {
            return Err(KeyParseError::Empty);
        }

        let mut mods = Mods::NONE;
        for held in modifiers {
            mods |= modifier(held)?;
        }
        let (key, key_mods, text) = self::key(name)?;
        Ok(Self {
            key,
            mods: mods | key_mods,
            text,
        })
    }
}

/// One modifier name.
fn modifier(name: &str) -> Result<Mods, KeyParseError> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Ok(Mods::CTRL),
        "alt" | "opt" | "option" | "meta" => Ok(Mods::ALT),
        "shift" => Ok(Mods::SHIFT),
        "super" | "cmd" | "command" | "win" => Ok(Mods::SUPER),
        "" => Err(KeyParseError::Empty),
        other => Err(KeyParseError::UnknownModifier(other.to_owned())),
    }
}

/// One key name: the table, a function key, or a single character.
///
/// The extra [`Mods`] is the shift a capital letter implies: `ctrl+A` is
/// Ctrl+Shift+A, which is not the same keystroke as `ctrl+a`.
fn key(name: &str) -> Result<(Key, Mods, Option<String>), KeyParseError> {
    let mut chars = name.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        return Ok(character(only));
    }
    let lower = name.to_ascii_lowercase();
    if let Some(key) = named(&lower) {
        return Ok((key, Mods::NONE, None));
    }
    if let Some(number) = lower.strip_prefix('f')
        && let Ok(number) = number.parse::<u8>()
        && let Some(key) = function(number)
    {
        return Ok((key, Mods::NONE, None));
    }
    Err(KeyParseError::UnknownKey(name.to_owned()))
}

/// A single character as a keystroke.
///
/// ASCII letters and digits get their physical key, because that is what a
/// Kitty-protocol application is told about. Everything else is
/// [`Key::Unidentified`] carrying its own text, which the encoder renders
/// identically — it works from the text for a key it cannot name, in the legacy
/// encoding and in the Kitty one alike.
fn character(only: char) -> (Key, Mods, Option<String>) {
    let text = Some(only.to_string());
    if only.is_ascii_lowercase() {
        return (LETTERS[only as usize - 'a' as usize], Mods::NONE, text);
    }
    if only.is_ascii_uppercase() {
        return (LETTERS[only as usize - 'A' as usize], Mods::SHIFT, text);
    }
    if only.is_ascii_digit() {
        return (DIGITS[only as usize - '0' as usize], Mods::NONE, text);
    }
    if only == ' ' {
        return (Key::Space, Mods::NONE, text);
    }
    (Key::Unidentified, Mods::NONE, text)
}

/// The named keys, and the aliases a script writer reaches for first.
fn named(lower: &str) -> Option<Key> {
    Some(match lower {
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "esc" | "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        _ => return None,
    })
}

/// `f1` … `f25`, the range the vendored key table names.
fn function(number: u8) -> Option<Key> {
    Some(match number {
        1 => Key::F1,
        2 => Key::F2,
        3 => Key::F3,
        4 => Key::F4,
        5 => Key::F5,
        6 => Key::F6,
        7 => Key::F7,
        8 => Key::F8,
        9 => Key::F9,
        10 => Key::F10,
        11 => Key::F11,
        12 => Key::F12,
        13 => Key::F13,
        14 => Key::F14,
        15 => Key::F15,
        16 => Key::F16,
        17 => Key::F17,
        18 => Key::F18,
        19 => Key::F19,
        20 => Key::F20,
        21 => Key::F21,
        22 => Key::F22,
        23 => Key::F23,
        24 => Key::F24,
        25 => Key::F25,
        _ => return None,
    })
}

/// `a`–`z`, in order, so a letter indexes rather than matches.
const LETTERS: [Key; 26] = [
    Key::A,
    Key::B,
    Key::C,
    Key::D,
    Key::E,
    Key::F,
    Key::G,
    Key::H,
    Key::I,
    Key::J,
    Key::K,
    Key::L,
    Key::M,
    Key::N,
    Key::O,
    Key::P,
    Key::Q,
    Key::R,
    Key::S,
    Key::T,
    Key::U,
    Key::V,
    Key::W,
    Key::X,
    Key::Y,
    Key::Z,
];

/// `0`–`9`, in order.
const DIGITS: [Key; 10] = [
    Key::Digit0,
    Key::Digit1,
    Key::Digit2,
    Key::Digit3,
    Key::Digit4,
    Key::Digit5,
    Key::Digit6,
    Key::Digit7,
    Key::Digit8,
    Key::Digit9,
];

#[cfg(test)]
mod tests {
    use super::{KeyParseError, KeyStroke};
    use amx_vt::{Key, Mods};

    fn parsed(combo: &str) -> KeyStroke {
        // The combos here are literals this test chose; a failure is the test's
        // subject, so unwrapping states it as one.
        KeyStroke::parse(combo).expect("a combo this build reads")
    }

    #[test]
    fn a_bare_character_is_itself() {
        let stroke = parsed("a");
        assert_eq!(stroke.key, Key::A);
        assert_eq!(stroke.mods, Mods::NONE);
        assert_eq!(stroke.text.as_deref(), Some("a"));

        // A key the table cannot name still reaches the child as its text.
        assert_eq!(parsed("€").key, Key::Unidentified);
        assert_eq!(parsed("€").text.as_deref(), Some("€"));
    }

    #[test]
    fn a_capital_carries_the_shift_it_implies() {
        let stroke = parsed("ctrl+A");
        assert_eq!(stroke.key, Key::A);
        assert!(stroke.mods.contains(Mods::CTRL));
        assert!(stroke.mods.contains(Mods::SHIFT));
    }

    #[test]
    fn modifiers_stack_and_are_case_insensitive() {
        let stroke = parsed("Alt+SHIFT+tab");
        assert_eq!(stroke.key, Key::Tab);
        assert!(stroke.mods.contains(Mods::ALT));
        assert!(stroke.mods.contains(Mods::SHIFT));
        assert_eq!(stroke.text, None);
    }

    #[test]
    fn plus_is_a_key_not_only_a_separator() {
        assert_eq!(parsed("+").text.as_deref(), Some("+"));
        let stroke = parsed("ctrl++");
        assert_eq!(stroke.text.as_deref(), Some("+"));
        assert!(stroke.mods.contains(Mods::CTRL));
    }

    #[test]
    fn function_keys_parse_by_number() {
        assert_eq!(parsed("f1").key, Key::F1);
        assert_eq!(parsed("F12").key, Key::F12);
        assert_eq!(
            KeyStroke::parse("f26"),
            Err(KeyParseError::UnknownKey("f26".to_owned()))
        );
    }

    #[test]
    fn an_unreadable_element_is_named_not_swallowed() {
        assert_eq!(
            KeyStroke::parse("hyper+x"),
            Err(KeyParseError::UnknownModifier("hyper".to_owned()))
        );
        assert_eq!(
            KeyStroke::parse("wiggle"),
            Err(KeyParseError::UnknownKey("wiggle".to_owned()))
        );
        assert_eq!(KeyStroke::parse(""), Err(KeyParseError::Empty));
        assert_eq!(KeyStroke::parse("ctrl+"), Err(KeyParseError::Empty));
    }
}
