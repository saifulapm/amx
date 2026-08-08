//! The key-name grammar: a name in, a byte out, and back again.
//!
//! # Why a key is one byte
//!
//! The input machine is a byte-stream state machine and deliberately not a key
//! decoder ([`crate::input`]): it recognises the prefix byte, the mode keys of
//! the layer it is in and the extent of an SGR mouse report, and passes
//! everything else through as opaque runs so encodings it has never heard of
//! reach the pane intact. A prefix-layer key is therefore *one byte*, and this
//! module refuses a name that is not — `f1` and `up` arrive as escape
//! sequences, `alt+x` arrives as two bytes, `€` as three. Refusing them by name
//! is what keeps the refusal legible; guessing at a multi-byte binding would
//! put a second, lossy key decoder in the one place the design forbids it.
//!
//! Combos are spelled the way `pane.send_keys` spells them — `ctrl+a`, `esc`,
//! or a bare character — so a user who has read one part of the docs can write
//! the other. The vocabulary here is the subset of that grammar which survives
//! the paragraph above, and [`key_name`] prints the canonical spelling of a
//! byte so `amx keys` teaches it rather than assuming it.

use thiserror::Error;

/// A key name this build cannot use as a prefix-layer key.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum KeyError {
    /// The name, or a segment of it, was empty.
    #[error("empty key name")]
    Empty,
    /// A modifier nothing in the table matches.
    #[error("unknown modifier `{0}`")]
    UnknownModifier(String),
    /// A key name nothing in the table matches, and which is not one character.
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    /// A key amx can name but which does not reach a client as one byte.
    #[error(
        "`{name}` does not reach a client as a single byte ({why}), and a prefix-layer key is one byte"
    )]
    NotOneByte {
        /// The name as written.
        name: String,
        /// Why it is more than a byte, and what to write instead.
        why: &'static str,
    },
}

/// The byte a key name arrives as, or why it does not arrive as one.
///
/// # Errors
///
/// [`KeyError`] naming the element that could not be read, or the reason the
/// key is not a single byte.
pub fn key_byte(name: &str) -> Result<u8, KeyError> {
    let mut parts: Vec<&str> = name.split('+').collect();
    // Two empty tail segments mean the plus key itself rather than a dangling
    // separator — `ctrl++` is Ctrl with `+`, and `+` alone is `+` — which is
    // the rule `pane.send_keys` reads combos by, kept identical on purpose.
    if parts.len() >= 2 && parts[parts.len() - 1].is_empty() && parts[parts.len() - 2].is_empty() {
        parts.pop();
        if let Some(last) = parts.last_mut() {
            *last = "+";
        }
    }
    let Some((key, modifiers)) = parts.split_last() else {
        return Err(KeyError::Empty);
    };
    if key.is_empty() {
        return Err(KeyError::Empty);
    }

    let mut ctrl = false;
    for held in modifiers {
        match held.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "" => return Err(KeyError::Empty),
            "shift" => {
                return Err(KeyError::NotOneByte {
                    name: name.to_owned(),
                    why: "spell the character it produces, like `A`",
                });
            }
            "alt" | "opt" | "option" | "meta" => {
                return Err(KeyError::NotOneByte {
                    name: name.to_owned(),
                    why: "alt sends an Esc ahead of the key, which is two bytes",
                });
            }
            "super" | "cmd" | "command" | "win" => {
                return Err(KeyError::NotOneByte {
                    name: name.to_owned(),
                    why: "no terminal encoding carries it",
                });
            }
            other => return Err(KeyError::UnknownModifier(other.to_owned())),
        }
    }

    let base = base_byte(name, key)?;
    if !ctrl {
        return Ok(base);
    }
    control(base).ok_or_else(|| KeyError::NotOneByte {
        name: name.to_owned(),
        why: "ctrl makes a control byte out of `@`–`_`, `a`–`z`, `?` and space, \
              and nothing else",
    })
}

/// The unmodified key: one printable character, or a name from the table.
fn base_byte(whole: &str, key: &str) -> Result<u8, KeyError> {
    let mut chars = key.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        return match u8::try_from(only) {
            Ok(byte) if (0x20..=0x7e).contains(&byte) => Ok(byte),
            _ => Err(KeyError::NotOneByte {
                name: whole.to_owned(),
                why: "it is more than one byte of UTF-8",
            }),
        };
    }
    let lower = key.to_ascii_lowercase();
    if let Some(byte) = named(&lower) {
        return Ok(byte);
    }
    // Named, and named honestly: these are keys `pane.send_keys` reads, so the
    // answer a user needs is "amx knows this key and it is not one byte", not
    // "amx has never heard of `up`".
    if SEQUENCE_KEYS.contains(&lower.as_str())
        || (lower.starts_with('f') && lower[1..].parse::<u8>().is_ok())
    {
        return Err(KeyError::NotOneByte {
            name: whole.to_owned(),
            why: "it arrives as an escape sequence",
        });
    }
    Err(KeyError::UnknownKey(key.to_owned()))
}

/// The named keys that *are* one byte.
fn named(lower: &str) -> Option<u8> {
    Some(match lower {
        "enter" | "return" => 0x0d,
        "tab" => 0x09,
        "esc" | "escape" => 0x1b,
        "space" => 0x20,
        // What every terminal amx has met sends for the Backspace key; the
        // `^H` spelling is reachable as `ctrl+h` for a terminal that sends it.
        "backspace" => 0x7f,
        _ => return None,
    })
}

/// Keys amx can name that arrive as escape sequences rather than as a byte.
const SEQUENCE_KEYS: &[&str] = &[
    "up", "down", "left", "right", "home", "end", "pageup", "pgup", "pagedown", "pgdn", "insert",
    "ins", "delete", "del",
];

/// `ctrl` applied to a printable byte, where it makes one.
const fn control(base: u8) -> Option<u8> {
    match base {
        b' ' => Some(0x00),
        b'?' => Some(0x7f),
        // `@`–`_` is the ASCII definition; lowercase letters fold onto it.
        0x40..=0x5f => Some(base & 0x1f),
        0x61..=0x7a => Some(base & 0x1f),
        _ => None,
    }
}

/// The canonical name of a byte, as `amx keys` prints it.
///
/// Total, and the inverse of [`key_byte`] on everything [`key_byte`] produces:
/// a user who reads a row of `amx keys` can paste the key back into
/// `config.toml` and get the same binding.
#[must_use]
pub fn key_name(byte: u8) -> String {
    match byte {
        0x00 => "ctrl+space".to_owned(),
        0x09 => "tab".to_owned(),
        0x0d => "enter".to_owned(),
        0x1b => "esc".to_owned(),
        0x20 => "space".to_owned(),
        0x7f => "backspace".to_owned(),
        0x01..=0x1a => format!("ctrl+{}", (byte + 0x60) as char),
        0x1c..=0x1f => format!("ctrl+{}", (byte + 0x40) as char),
        0x21..=0x7e => (byte as char).to_string(),
        _ => format!("0x{byte:02x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyError, key_byte, key_name};

    #[test]
    fn every_byte_key_byte_produces_names_back_to_itself() {
        // The round trip is the promise `amx keys` makes: a row it prints is a
        // row a user can paste into `config.toml`. Checked over the whole byte
        // range rather than over a sample, because the interesting cases are
        // exactly the boundaries between the spellings.
        for byte in 0..=u8::MAX {
            let name = key_name(byte);
            match key_byte(&name) {
                Ok(back) => assert_eq!(back, byte, "{name:?} named byte {byte:#04x}"),
                Err(err) => assert!(
                    byte >= 0x80,
                    "{byte:#04x} names {name:?}, which does not read back: {err}",
                ),
            }
        }
    }

    #[test]
    fn a_key_that_is_not_one_byte_says_so_rather_than_reading_as_unknown() {
        for (name, fragment) in [
            ("f1", "escape sequence"),
            ("up", "escape sequence"),
            ("alt+x", "Esc ahead"),
            ("shift+a", "character it produces"),
            ("€", "one byte of UTF-8"),
            ("ctrl+1", "control byte"),
        ] {
            let err = key_byte(name).expect_err("not a single byte");
            assert!(
                matches!(err, KeyError::NotOneByte { .. }),
                "{name:?} read as {err:?}",
            );
            assert!(err.to_string().contains(fragment), "{name:?}: {err}");
        }
    }
}
