//! The one-pane viewport's detach chord: prefix, then `q`.
//!
//! The full client's detach is the input machine's own prefix `d` verb
//! (04 §7) and lives in `amx-client`; nothing here applies to it. The
//! chrome-free `amx attach --pane` viewport deliberately runs no mode
//! machine — every byte belongs to the pane — so the one sequence it must
//! recognise itself, prefix+`q` (04 §1), gets the smallest possible
//! recogniser: [`Chord`] decodes no keys, tracks no modes, and forwards
//! nothing.

/// The prefix key: `ctrl+a` (04 §7's default).
pub const PREFIX: u8 = 0x01;

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
