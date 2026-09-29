//! Base64 for OSC 52, the escape sequence that sets the clipboard of the
//! terminal the view is drawn on.
//!
//! OSC 52 reaches the user's own terminal even over ssh, where a clipboard on
//! the machine amx runs on would be no use. A terminal that ignores the
//! sequence copies nothing. The encoder is hand-written to avoid a dependency
//! for twenty lines.

/// Encodes `bytes` as padded base64 (RFC 4648).
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut said = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        // The group as one 24-bit number, a short group zero-filled.
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
        // The RFC 4648 test vectors: a last group of three, two and one bytes.
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
        // Non-ASCII bytes, and the `+` and `/` only high bytes reach.
        assert_eq!(base64("│".as_bytes()), "4pSC");
        assert_eq!(base64(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(base64(&[0x00, 0x00, 0x00]), "AAAA");
    }
}
