//! The detach chord: prefix, then one key.
//!
//! 04 §7's input model is modal — "Prefix mode (`ctrl+a`, configurable):
//! one-shot commands — split, zoom, kill, **detach**, next-attention, picker,
//! rename" — and the full mode machine over it is T14's
//! (`amx-client/src/input/**`, plus `App::handle_input`, which is still
//! `todo!()`).
//!
//! Detach is the one sequence this binary cannot wait for, because a client
//! that cannot be left is worse than a client that cannot be typed in: the
//! terminal it took would only come back by killing it. So [`Chord`] is
//! deliberately the smallest thing that recognises `prefix`+key in a byte
//! stream, and nothing else — it decodes no keys, tracks no modes, and forwards
//! nothing. When T14's mode machine lands, its detach arm replaces this and the
//! attach loops call `App::handle_input` instead.

/// The prefix key: `ctrl+a` (04 §7's default).
pub const PREFIX: u8 = 0x01;

/// The key that detaches a full client: `prefix` then `d`.
pub const DETACH: u8 = b'd';

/// The key that detaches a single-pane viewport: `prefix` then `q` (04 §1).
pub const DETACH_PANE: u8 = b'q';

/// Recogniser for `prefix` followed by one key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    key: u8,
    armed: bool,
}

impl Chord {
    /// A recogniser for `prefix` then `key`.
    #[must_use]
    pub const fn new(key: u8) -> Self {
        Self { key, armed: false }
    }

    /// Whether the last byte fed was the prefix.
    #[must_use]
    pub const fn is_armed(&self) -> bool {
        self.armed
    }

    /// Feed a chunk of input; `true` once the chord completes.
    ///
    /// A second prefix re-arms rather than cancelling (`ctrl+a ctrl+a` is how
    /// every multiplexer sends a literal prefix onward, so the state after it
    /// is "prefix pending", not "back to normal"), and any other key disarms.
    /// Bytes after the completing key are not consumed here: the caller is
    /// leaving, and what it does not read stays in the terminal's buffer.
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            if !self.armed {
                self.armed = byte == PREFIX;
                continue;
            }
            if byte == self.key {
                self.armed = false;
                return true;
            }
            self.armed = byte == PREFIX;
        }
        false
    }
}
