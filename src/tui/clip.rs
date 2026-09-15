//! Putting text on the clipboard of the terminal the view is drawn on.
//!
//! The terminal is the only thing here that can reach a clipboard: amx draws
//! on it over whatever the connection is, and a selection made inside a pane
//! on the far side of an ssh is not a selection the machine amx runs on can
//! paste. So the text goes back the way every other thing the view says goes
//! back — as an escape sequence, OSC 52 — and whoever is looking at the
//! screen has it. A terminal that ignores the sequence copies nothing, which
//! is the cost of asking rather than reaching.
//!
//! Base64 is written out here because it is the one thing OSC 52 needs and
//! twenty lines is less than a dependency: the encoder is RFC 4648's own
//! alphabet with the padding, and its whole job is bytes a terminal will take.

/// `bytes` as base64, RFC 4648 with the padding.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut said = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        // The group as one 24-bit number, a short group zero-filled. What the
        // padding then says is how many of the four characters stood for a
        // byte somebody sent.
        let word = group
            .iter()
            .chain(std::iter::repeat(&0))
            .take(3)
            .fold(0u32, |word, byte| (word << 8) | u32::from(*byte));
        for at in 0..4u32 {
            match at <= group.len() as u32 {
                true => said.push(ALPHABET[(word >> (18 - at * 6)) as usize & 0x3f] as char),
                false => said.push('='),
            }
        }
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_rfc_4648_with_its_padding() {
        // The RFC's own test vectors, which between them cover every length a
        // last group can be: three bytes and no padding, two and one `=`, one
        // and two.
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_carries_every_byte_a_selection_can_hold() {
        // The high bytes of anything but ASCII, which a wall of agent names
        // holds as readily as a card of prose does, and the two characters at
        // the top of the alphabet that only a high byte reaches.
        assert_eq!(base64("│".as_bytes()), "4pSC");
        assert_eq!(base64(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(base64(&[0x00, 0x00, 0x00]), "AAAA");
    }
}
