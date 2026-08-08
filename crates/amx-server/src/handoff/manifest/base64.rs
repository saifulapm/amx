//! Base64, for the one binary field in a manifest of text.
//!
//! Hand-rolled and forty lines, because the alternative shapes are both worse:
//! a serde byte array costs about four times the wire, and a dependency costs a
//! dependency. Everything else in a pane entry is already text — the
//! synthesized grid is ASCII control sequences and clusters the library handed
//! over as UTF-8 — so this is the only place an encoding is needed at all.

use super::ManifestError;

/// The standard base64 alphabet.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes for a JSON string field.
///
/// Hand-rolled because the packed rows are the one binary field in an otherwise
/// textual manifest, and a base64 crate is a dependency the tree does not need
/// for forty lines (HACKING.md's "prefer std, keep the tree lean").
pub(super) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for slot in 0..4 {
            if slot <= chunk.len() {
                let index = (packed >> (18 - slot * 6)) & 0x3F;
                out.push(char::from(ALPHABET[index as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode what [`encode`] produced.
pub(super) fn decode(text: &str) -> Result<Vec<u8>, ManifestError> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut block = 0u32;
    let mut filled = 0u32;
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let Some(index) = ALPHABET.iter().position(|slot| *slot == byte) else {
            return Err(ManifestError::Malformed(format!(
                "base64 character {byte:#04x}"
            )));
        };
        // The index came from a 64-entry table, so it is six bits.
        block = (block << 6) | u32::try_from(index).unwrap_or(0);
        filled += 6;
        if filled >= 8 {
            filled -= 8;
            // Shifted down to a single byte by the line above.
            out.push(u8::try_from((block >> filled) & 0xFF).unwrap_or(0));
        }
    }
    Ok(out)
}
