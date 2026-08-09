//! SGR mouse report recognition, the wheel's button, and the relay's
//! translation (04 §7, D9, D14).
//!
//! amx recognises SGR mouse reports (`CSI < btn ; col ; row (M|m)`) and does
//! one of exactly two things with each. Which one depends on the pane, and the
//! two have different fences, which is the whole shape of this module:
//!
//! - **The pane asked for reports.** The report is *relayed* to it —
//!   [`relocate`] rewrites the coordinates into the pane's own frame and
//!   nothing here looks at what they mean. Positional data is carried, never
//!   interpreted.
//! - **The pane did not.** Then D14's one interpreted event class applies and
//!   [`wheel_of`] reads the **button and nothing else**: the parse stops at the
//!   first `;`, so no column and no row is ever read on this path. That is 03
//!   §1's boundary and D14's ("any *positional* mouse interpretation … remains
//!   out, permanently") in executable form, and the test that feeds four
//!   reports with the same button and wildly different coordinates is what
//!   asserts it.
//!
//! **Why a relay and not a byte-for-byte forward.** A report's coordinates are
//! viewport-absolute and the application in a pane reads them as pane-local,
//! and amx's panes are never at the viewport origin: the content area is the
//! terminal minus a status line and every pane is inset one cell for its border
//! (`amx-client/src/model/mod.rs:364`,
//! `amx-server/src/actor/core/view.rs:225`, `view.rs:37-45`). X01 watched tmux
//! rewrite row 20 to row 7 for a pane at `top=13` for exactly this reason
//! (`docs/notes/m4-mouse-path.md` F-1). Forwarding the bytes unchanged would
//! deliver a coordinate wrong by at least one row and one column, always — so
//! D9's "unchanged" is met in the only way it can be: unchanged in *meaning*,
//! which costs three numbers rewritten.
//!
//! [`relocate`] is not a hit test and cannot become one. Which pane a report
//! is relayed to is decided by *focus*, upstream of this file and before any
//! coordinate is read; all this does is subtract that pane's origin and refuse
//! a report that lands outside it. Nothing here searches a rect list for a
//! point, and the fence D14 draws is against exactly that search.
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

/// A wheel turn: the one mouse event class amx interprets (D14).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wheel {
    /// Wheel up, away from the user. Scrolls back through the cache.
    Up,
    /// Wheel down, toward the user. Scrolls forward, and leaves copy mode at
    /// the live edge.
    Down,
}

/// The wheel bank: bit 6 set, bit 7 clear. Buttons 8 and 9 are 128 and 129
/// (`vendor/libghostty-vt/src/input/mouse_encode.zig:223-224`) and must not be
/// mistaken for a wheel, so both bits are tested.
const WHEEL_BANK: u32 = 0b1100_0000;
/// What a button in the wheel bank looks like: 64/65, and the horizontal pair
/// 66/67 (`mouse_encode.zig:219-222`).
const WHEEL: u32 = 0b0100_0000;
/// Motion adds 32 (`mouse_encode.zig:237`). A drag is not a wheel turn.
const MOTION: u32 = 0b0010_0000;
/// Which of the bank's four buttons this is.
const BUTTON: u32 = 0b0000_0011;

/// The wheel turn `report` describes, if it is one.
///
/// `report` must be a whole report as [`scan`] measured it. The parse reads the
/// button and stops at the first `;`: the column and the row are not read, at
/// all, ever. Modifiers ride the same field — shift adds 4, alt 8, ctrl 16
/// (`vendor/libghostty-vt/src/input/mouse_encode.zig:231-233`) — so this masks
/// rather than compares, and every one of the 28 modifier combinations decodes.
///
/// The horizontal wheel (buttons 66 and 67) is recognised as a wheel bank and
/// deliberately *not* interpreted: a sideways scroll has no meaning in a
/// scrollback.
pub(super) fn wheel_of(report: &[u8]) -> Option<Wheel> {
    // `m` is a release and the wheel never sends one.
    if report.last() != Some(&b'M') {
        return None;
    }
    let mut button: u32 = 0;
    for &b in report.get(INTRODUCER.len()..)? {
        if !b.is_ascii_digit() {
            break;
        }
        // Saturating rather than wrapping: a button field wider than any real
        // encoder emits must not alias onto 64 by overflowing.
        button = button
            .saturating_mul(10)
            .saturating_add(u32::from(b - b'0'));
    }
    if button & WHEEL_BANK != WHEEL || button & MOTION != 0 {
        return None;
    }
    match button & BUTTON {
        0 => Some(Wheel::Up),
        1 => Some(Wheel::Down),
        _ => None,
    }
}

/// Rewrite `report`'s viewport-absolute coordinates into the frame of a pane
/// whose interior is `x`/`y` (zero-based, in the terminal) and `w`×`h` cells,
/// appending the rewritten report to `out`.
///
/// Answers whether it could: a report outside that interior is refused, and the
/// caller drops it. Refusing is not a hit test — the pane was chosen by focus
/// before this was called, and this only checks that the report is inside the
/// pane it was already addressed to.
///
/// `out` is cleared first, so a caller can hand the same buffer in every time
/// and a relayed report allocates nothing steady-state.
pub(super) fn relocate(report: &[u8], x: u16, y: u16, w: u16, h: u16, out: &mut Vec<u8>) -> bool {
    out.clear();
    let Some(body) = report.get(INTRODUCER.len()..) else {
        return false;
    };
    let Some((&final_byte, params)) = body.split_last() else {
        return false;
    };
    let mut fields = params.split(|&b| b == b';');
    let (Some(button), Some(col), Some(row), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    let (Some(col), Some(row)) = (decimal(col), decimal(row)) else {
        return false;
    };
    // Reports are one-based and a rect's origin is zero-based, so the pane's
    // first column is `x + 1` and its last is `x + w`.
    if col <= u32::from(x) || col > u32::from(x) + u32::from(w) {
        return false;
    }
    if row <= u32::from(y) || row > u32::from(y) + u32::from(h) {
        return false;
    }
    out.extend_from_slice(INTRODUCER);
    out.extend_from_slice(button);
    push_decimal(out, col - u32::from(x));
    push_decimal(out, row - u32::from(y));
    out.push(final_byte);
    true
}

/// A report parameter as a number, or `None` when it is empty or overflows.
fn decimal(field: &[u8]) -> Option<u32> {
    if field.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &b in field {
        let digit = b.checked_sub(b'0').filter(|d| *d < 10)?;
        value = value.checked_mul(10)?.checked_add(u32::from(digit))?;
    }
    Some(value)
}

/// Append `value` in decimal, preceded by the `;` that separates it from what
/// came before.
fn push_decimal(out: &mut Vec<u8>, value: u32) {
    out.push(b';');
    let mut digits = [0_u8; 10];
    let mut at = digits.len();
    let mut left = value;
    loop {
        at -= 1;
        // `left % 10` is 0..=9, so the cast is exact.
        digits[at] = b'0' + u8::try_from(left % 10).unwrap_or(0);
        left /= 10;
        if left == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

#[cfg(test)]
mod tests {
    use super::{Scan, Wheel, relocate, scan, wheel_of};

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

    #[test]
    fn the_wheel_decodes_under_every_modifier() {
        // 64/65 bare, then shift (+4), alt (+8), ctrl (+16) and the three
        // together (+28), which is the whole point of masking rather than
        // comparing (`mouse_encode.zig:231-233`).
        for &(button, wheel) in &[
            (64, Wheel::Up),
            (65, Wheel::Down),
            (68, Wheel::Up),
            (69, Wheel::Down),
            (72, Wheel::Up),
            (80, Wheel::Up),
            (92, Wheel::Up),
            (93, Wheel::Down),
        ] {
            let report = format!("\x1b[<{button};10;5M");
            assert_eq!(
                wheel_of(report.as_bytes()),
                Some(wheel),
                "button {button} did not decode",
            );
        }
    }

    #[test]
    fn clicks_drags_and_the_sideways_wheel_are_not_wheel_turns() {
        // A press, a release, a drag (motion adds 32), buttons 8 and 9 in the
        // bank above the wheel's, and the horizontal wheel.
        for report in [
            &b"\x1b[<0;10;5M"[..],
            b"\x1b[<0;10;5m",
            b"\x1b[<32;10;5M",
            b"\x1b[<96;10;5M",
            b"\x1b[<128;10;5M",
            b"\x1b[<129;10;5M",
            b"\x1b[<66;10;5M",
            b"\x1b[<67;10;5M",
            b"\x1b[<64;10;5m",
        ] {
            assert_eq!(wheel_of(report), None, "{report:?} decoded as a wheel");
        }
    }

    #[test]
    fn the_wheel_parse_cannot_see_a_coordinate() {
        // The same button with wildly different coordinates, and one report
        // with no coordinates at all: a parse that stops at the first `;`
        // answers the same for every one of them. This is D14's fence, and it
        // is asserted rather than described.
        for report in [
            &b"\x1b[<64;1;1M"[..],
            b"\x1b[<64;999;999M",
            b"\x1b[<64;0;0M",
            b"\x1b[<64;;M",
            b"\x1b[<64M",
        ] {
            assert_eq!(wheel_of(report), Some(Wheel::Up), "{report:?}");
        }
    }

    #[test]
    fn a_button_field_wider_than_any_encoder_emits_does_not_alias_onto_the_wheel() {
        assert_eq!(wheel_of(b"\x1b[<4294967360;1;1M"), None);
    }

    #[test]
    fn relocating_subtracts_the_panes_origin() {
        let mut out = Vec::new();
        // A pane interior at (1, 1), 10x10: viewport column 2 row 3 is the
        // pane's own column 1 row 2.
        assert!(relocate(b"\x1b[<64;2;3M", 1, 1, 10, 10, &mut out));
        assert_eq!(out, b"\x1b[<64;1;2M");
        // The button and the final byte ride through untouched, release
        // included.
        out.clear();
        assert!(relocate(b"\x1b[<32;11;11m", 1, 1, 10, 10, &mut out));
        assert_eq!(out, b"\x1b[<32;10;10m");
    }

    #[test]
    fn a_report_outside_the_pane_is_refused() {
        let mut out = Vec::new();
        // The status line row, the border row and column, and one cell past
        // each far edge.
        for report in [
            &b"\x1b[<64;1;3M"[..],
            b"\x1b[<64;3;1M",
            b"\x1b[<64;12;3M",
            b"\x1b[<64;3;12M",
        ] {
            assert!(!relocate(report, 1, 1, 10, 10, &mut out), "{report:?}");
            assert!(out.is_empty());
        }
    }

    #[test]
    fn a_report_missing_a_coordinate_is_refused_rather_than_relayed() {
        let mut out = Vec::new();
        for report in [&b"\x1b[<64M"[..], b"\x1b[<64;5M", b"\x1b[<64;5;;5M"] {
            assert!(!relocate(report, 0, 0, 80, 24, &mut out), "{report:?}");
        }
    }
}
