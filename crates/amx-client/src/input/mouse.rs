//! SGR mouse report recognition (04 §7, D9).
//!
//! amx forwards SGR mouse reports (`CSI < btn ; col ; row (M|m)`) to panes
//! that enabled mouse reporting and otherwise swallows them; chrome never
//! interprets one. That takes recognising the report — and nothing else: this
//! is not a mouse *decoder*, it never extracts a button or a coordinate, it
//! only finds the report's extent so the bytes can be handed on whole or
//! dropped whole.
//!
//! **Not byte-for-byte, whatever the older wording here said.** A report's
//! coordinates are viewport-absolute and the application in a pane reads them
//! as pane-local, and amx's panes are never at the viewport origin: the content
//! area is the terminal minus a status line and every pane is inset one cell
//! for its border (`amx-client/src/model/mod.rs:364`,
//! `amx-server/src/actor/core/view.rs:225`, `view.rs:37-45`). X01 watched tmux
//! rewrite row 20 to row 7 for a pane at `top=13` for exactly this reason
//! (`docs/notes/m4-mouse-path.md`). Translating the offset — or declining to,
//! and saying so — is X13's decision and X13's code. What is settled here is
//! only that this module does not read the numbers.
//!
//! A report can be split across two reads. [`scan`] reports a split as
//! [`Scan::Partial`] only once the unambiguous `ESC [ <` introducer is fully
//! present — no key encoding starts with `<` after CSI. A read ending in a
//! bare `ESC` or `ESC [` is *not* held back: buffering it would delay a
//! plain `Esc` keypress (the classic vi timeout problem), so those bytes pass
//! through and the rare mouse report split inside its first two bytes leaks
//! to the pane instead of being gated. Decoding is bounded either way:
//! anything unterminated past [`MAX_REPORT`] stops being treated as mouse.

/// The longest byte run [`scan`] will treat as one SGR report:
/// `ESC [ <` + `btn;col;row` at full width + the final byte, with slack.
pub(super) const MAX_REPORT: usize = 24;

/// The three-byte introducer every SGR mouse report starts with.
const INTRODUCER: &[u8] = b"\x1b[<";

/// What [`scan`] found at the start of its input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Scan {
    /// A complete SGR report of this many bytes.
    Report(usize),
    /// The input is a proper prefix of an SGR report (introducer seen, no
    /// final byte yet): carry it and continue on the next read.
    Partial,
    /// Not an SGR mouse report.
    Not,
}

/// Classify the start of `bytes` as an SGR mouse report, a split one, or
/// neither.
pub(super) fn scan(bytes: &[u8]) -> Scan {
    if !bytes.starts_with(INTRODUCER) {
        return Scan::Not;
    }
    for (at, &b) in bytes.iter().enumerate().skip(INTRODUCER.len()) {
        if at >= MAX_REPORT {
            return Scan::Not;
        }
        if !param_byte(b) {
            return match b {
                b'M' | b'm' => Scan::Report(at + 1),
                _ => Scan::Not,
            };
        }
    }
    Scan::Partial
}

/// Whether `b` may appear in a report's `btn;col;row` parameters.
pub(super) fn param_byte(b: u8) -> bool {
    b.is_ascii_digit() || b == b';'
}

#[cfg(test)]
mod tests {
    use super::{Scan, scan};

    #[test]
    fn recognises_press_and_release_reports() {
        assert_eq!(scan(b"\x1b[<0;10;5M"), Scan::Report(10));
        assert_eq!(scan(b"\x1b[<0;10;5mrest"), Scan::Report(10));
    }

    #[test]
    fn a_split_report_is_partial_only_past_the_introducer() {
        assert_eq!(scan(b"\x1b[<0;10"), Scan::Partial);
        assert_eq!(scan(b"\x1b[<"), Scan::Partial);
        assert_eq!(scan(b"\x1b["), Scan::Not);
        assert_eq!(scan(b"\x1b"), Scan::Not);
    }

    #[test]
    fn key_sequences_and_junk_are_not_reports() {
        assert_eq!(scan(b"\x1b[97;5u"), Scan::Not);
        assert_eq!(scan(b"\x1b[<0;10;5x"), Scan::Not);
        assert_eq!(scan(b"\x1b[<111111111111111111111111;1;1M"), Scan::Not);
    }
}
