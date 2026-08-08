//! The byte-level emitters both halves of the replay share.
//!
//! Nothing here knows what a pane is: they take numbers and append the escape
//! sequence that carries them. They are a file of their own because the grid
//! synthesis and the mode replay both reach for the same five, and a shared
//! helper living in whichever caller happened to need it first is how one
//! spelling of `CSI` becomes two.

/// Append `CSI ? number h|l`.
pub(super) fn put_dec_mode(out: &mut Vec<u8>, number: u16, on: bool) {
    out.extend_from_slice(b"\x1b[?");
    put_number(out, u32::from(number));
    out.push(if on { b'h' } else { b'l' });
}

/// Append `CSI row ; column H`, both one-based.
pub(super) fn put_cup(out: &mut Vec<u8>, row: usize, column: usize) {
    out.extend_from_slice(b"\x1b[");
    put_number(
        out,
        u32::try_from(row.saturating_add(1)).unwrap_or(u32::MAX),
    );
    out.push(b';');
    put_number(
        out,
        u32::try_from(column.saturating_add(1)).unwrap_or(u32::MAX),
    );
    out.push(b'H');
}

/// Append `CSI value final`.
pub(super) fn put_csi(out: &mut Vec<u8>, value: u32, final_byte: u8) {
    out.extend_from_slice(b"\x1b[");
    put_number(out, value);
    out.push(final_byte);
}

/// Append `CSI value SP q` — DECSCUSR, whose final byte takes an intermediate.
pub(super) fn put_csi_space_q(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(b"\x1b[");
    put_number(out, value);
    out.extend_from_slice(b" q");
}

/// Append a decimal number.
pub(super) fn put_number(out: &mut Vec<u8>, value: u32) {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    let mut left = value;
    loop {
        at -= 1;
        // `left % 10` is a single digit, so the sum is inside a byte.
        digits[at] = b'0' + u8::try_from(left % 10).unwrap_or(0);
        left /= 10;
        if left == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}
