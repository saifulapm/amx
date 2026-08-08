//! Reading what came back: the wheel parse, its fence, and the transcript's
//! own rendering of a read.
//!
//! Split from `main.rs` because it is the half of the probe that X13 is
//! actually going to reuse, and because it is the half that carries tests. It
//! decodes exactly two things — a wheel direction and a DECRQM answer — and
//! renders everything else as bytes rather than pretending to understand it.

use std::fmt::Write as _;

/// A wheel direction, the only thing D14's exception ever needs from a report.
///
/// Deliberately not a mouse event: there is no button beyond the wheel, no
/// coordinate, and no modifier. See [`wheel_of`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wheel {
    /// Button 4 — wheel up, scroll back.
    Up,
    /// Button 5 — wheel down, scroll toward the live edge.
    Down,
}

/// The wheel direction of one complete SGR report, or `None`.
///
/// This is the whole of D14's parse and it is deliberately the whole of it.
/// The report's shape is `ESC [ < btn ; col ; row (M|m)`; this reads `btn`,
/// stops at the first `;`, and never looks at `col` or `row` — 03 §1 and D14
/// both draw the line at positional interpretation, so the bytes that carry a
/// position are not read at all rather than read and discarded.
///
/// The button field is not a small enum. libghostty-vt's own encoder is the
/// specification (`vendor/libghostty-vt/src/input/mouse_encode.zig:200-239`):
/// the low two bits pick the button within a bank, bit 6 (64) selects the
/// wheel bank and bit 7 (128) the one above it, and shift/alt/ctrl add 4/8/16
/// while a motion report adds 32. So a wheel-up with shift held is 68 and one
/// with ctrl is 80, and an equality test against 64 would miss both. The test
/// is: wheel bank, not the bank above it, not a motion report, final byte `M`.
pub fn wheel_of(report: &[u8]) -> Option<Wheel> {
    let rest = report.strip_prefix(b"\x1b[<")?;
    // A wheel report is a press. `m` is a release, which the wheel never
    // sends and which nothing here interprets.
    if report.last() != Some(&b'M') {
        return None;
    }
    let mut button: u32 = 0;
    let mut digits = 0usize;
    for &byte in rest {
        if byte == b';' {
            break;
        }
        let digit = (byte as char).to_digit(10)?;
        // Bounded on purpose: past three digits this is not a button field
        // and the probe stops rather than accumulating an unbounded number.
        digits += 1;
        if digits > 3 {
            return None;
        }
        button = button * 10 + digit;
    }
    if digits == 0 {
        return None;
    }
    let button = u8::try_from(button).ok()?;
    // Bit 7 is the 128-bank (buttons 8..11); bit 5 is the motion bit.
    if button & 0b1100_0000 != 0b0100_0000 || button & 0b0010_0000 != 0 {
        return None;
    }
    // Bit 1 set is the horizontal wheel (66/67), which has no meaning in a
    // scrollback and is not interpreted.
    match button & 0b0000_0011 {
        0 => Some(Wheel::Up),
        1 => Some(Wheel::Down),
        _ => None,
    }
}

/// The extent of a complete SGR report at the start of `bytes`.
///
/// A deliberate near-duplicate of `amx_client::input::mouse::scan`, which is
/// crate-private and so unreachable from a binary. The probe needs the extent
/// only to split a read into reports and non-reports for the transcript; X13
/// reuses the real one rather than this.
pub fn report_len(bytes: &[u8]) -> Option<usize> {
    let rest = bytes.strip_prefix(b"\x1b[<")?;
    for (at, &byte) in rest.iter().enumerate() {
        // 24 is `mouse::MAX_REPORT`: past it this stops being a report.
        if at + 3 >= 24 {
            return None;
        }
        if byte.is_ascii_digit() || byte == b';' {
            continue;
        }
        return match byte {
            b'M' | b'm' => Some(at + 4),
            _ => None,
        };
    }
    None
}

/// A DECRQM reply at the start of `bytes`: its length, mode and answer.
///
/// The reply is `CSI ? Ps ; Pm $ y`. `Pm` is the terminal's answer about the
/// mode, and its meanings are the ones DEC gave it: 0 unrecognised, 1 set, 2
/// reset, 3 permanently set, 4 permanently reset.
pub fn decrqm(bytes: &[u8]) -> Option<(usize, u32, &'static str)> {
    let rest = bytes.strip_prefix(b"\x1b[?")?;
    let end = rest.windows(2).position(|pair| pair == b"$y")?;
    let mut parts = rest[..end].split(|&byte| byte == b';');
    let mode = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let state = match std::str::from_utf8(parts.next()?).ok()? {
        "0" => "not recognised",
        "1" => "set",
        "2" => "reset",
        "3" => "permanently set",
        "4" => "permanently reset",
        _ => return None,
    };
    Some((end + 5, mode, state))
}

/// One read, classified: what arrived and what the wheel parse made of it.
///
/// Kept visibly separate from the hex and the escapes beside it in the
/// transcript, so a note can quote the bytes rather than this reading of them.
pub fn classify(chunk: &[u8]) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < chunk.len() {
        if let Some((len, mode, state)) = decrqm(&chunk[at..]) {
            write!(&mut out, "[DECRQM {mode}={state}]").expect("write to a String");
            at += len;
        } else if let Some(len) = report_len(&chunk[at..]) {
            let report = &chunk[at..at + len];
            let wheel = match wheel_of(report) {
                Some(Wheel::Up) => " wheel=up",
                Some(Wheel::Down) => " wheel=down",
                None => "",
            };
            write!(&mut out, "[sgr-report {len}B{wheel}]").expect("write to a String");
            at += len;
        } else {
            let start = at;
            at += 1;
            while at < chunk.len() && report_len(&chunk[at..]).is_none() && chunk[at] != 0x1b {
                at += 1;
            }
            write!(&mut out, "[other \"{}\"]", escape(&chunk[start..at])).expect("write");
        }
    }
    out
}

/// `bytes` as C-style escapes, printable ASCII kept as itself.
pub fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        match byte {
            0x1b => out.push_str("\\e"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(byte as char),
            _ => write!(&mut out, "\\x{byte:02x}").expect("write to a String"),
        }
    }
    out
}

/// `bytes` as space-separated lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (at, &byte) in bytes.iter().enumerate() {
        if at > 0 {
            out.push(' ');
        }
        write!(&mut out, "{byte:02x}").expect("write to a String");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Wheel, decrqm, report_len, wheel_of};

    #[test]
    fn the_wheel_banks_decode_and_nothing_else_does() {
        assert_eq!(wheel_of(b"\x1b[<64;1;1M"), Some(Wheel::Up));
        assert_eq!(wheel_of(b"\x1b[<65;1;1M"), Some(Wheel::Down));
        // Horizontal wheel: recognised as a wheel bank, not interpreted.
        assert_eq!(wheel_of(b"\x1b[<66;1;1M"), None);
        assert_eq!(wheel_of(b"\x1b[<67;1;1M"), None);
        // Ordinary buttons and the 128-bank are not the wheel.
        assert_eq!(wheel_of(b"\x1b[<0;1;1M"), None);
        assert_eq!(wheel_of(b"\x1b[<2;1;1M"), None);
        assert_eq!(wheel_of(b"\x1b[<128;1;1M"), None);
        assert_eq!(wheel_of(b"\x1b[<129;1;1M"), None);
    }

    #[test]
    fn modifiers_ride_the_button_field_and_do_not_defeat_the_parse() {
        // shift +4, alt +8, ctrl +16 — mouse_encode.zig:231-233.
        assert_eq!(wheel_of(b"\x1b[<68;1;1M"), Some(Wheel::Up));
        assert_eq!(wheel_of(b"\x1b[<72;1;1M"), Some(Wheel::Up));
        assert_eq!(wheel_of(b"\x1b[<80;1;1M"), Some(Wheel::Up));
        assert_eq!(wheel_of(b"\x1b[<92;1;1M"), Some(Wheel::Up));
        assert_eq!(wheel_of(b"\x1b[<93;1;1M"), Some(Wheel::Down));
        // Motion adds 32 and is not a wheel click — mouse_encode.zig:237.
        assert_eq!(wheel_of(b"\x1b[<96;1;1M"), None);
        assert_eq!(wheel_of(b"\x1b[<97;1;1M"), None);
    }

    #[test]
    fn a_release_is_never_a_wheel_click() {
        assert_eq!(wheel_of(b"\x1b[<64;1;1m"), None);
        assert_eq!(wheel_of(b"\x1b[<65;1;1m"), None);
    }

    #[test]
    fn the_parse_stops_at_the_first_semicolon() {
        // Same button, wildly different coordinates: the parse cannot see
        // them, so it cannot depend on them. This is the fence, asserted.
        for report in [
            &b"\x1b[<64;1;1M"[..],
            &b"\x1b[<64;999;999M"[..],
            &b"\x1b[<64;0;0M"[..],
            &b"\x1b[<64;;M"[..],
        ] {
            assert_eq!(wheel_of(report), Some(Wheel::Up), "{report:?}");
        }
        // A button field that never terminates is not a button field.
        assert_eq!(wheel_of(b"\x1b[<6400;1;1M"), None);
    }

    #[test]
    fn junk_and_key_sequences_are_not_reports() {
        assert_eq!(wheel_of(b"\x1b[<;1;1M"), None);
        assert_eq!(wheel_of(b"\x1b[97;5u"), None);
        assert_eq!(wheel_of(b"\x1b[<x;1;1M"), None);
        assert_eq!(wheel_of(b""), None);
    }

    #[test]
    fn decrqm_replies_decode_to_a_mode_and_an_answer() {
        assert_eq!(decrqm(b"\x1b[?1006;2$y"), Some((11, 1006, "reset")));
        assert_eq!(decrqm(b"\x1b[?1007;1$y"), Some((11, 1007, "set")));
        assert_eq!(
            decrqm(b"\x1b[?1000;0$y"),
            Some((11, 1000, "not recognised"))
        );
        // A mouse report is not a DECRQM reply, and neither is a key.
        assert_eq!(decrqm(b"\x1b[<64;1;1M"), None);
        assert_eq!(decrqm(b"\x1b[A"), None);
    }

    #[test]
    fn report_extent_covers_the_whole_report_and_no_more() {
        assert_eq!(report_len(b"\x1b[<64;10;5M"), Some(11));
        assert_eq!(report_len(b"\x1b[<64;10;5mrest"), Some(11));
        assert_eq!(report_len(b"\x1b[<64;10"), None);
        assert_eq!(report_len(b"hello"), None);
    }
}
