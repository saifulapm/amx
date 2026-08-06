//! The golden-file mechanic, shared with T04's suite.
//!
//! Same directory tree (`tests/goldens/`), same pretty-printed one-message-
//! per-file format, and the same regeneration path:
//!
//! ```text
//! AMX_UPDATE_GOLDENS=1 cargo test -p amx-rig --test protocol
//! ```
//!
//! Binary stream messages golden as JSON too: the encoded bytes as hex lines
//! next to a `message` object saying what they encode, so a wire change shows
//! up as a readable diff rather than a blob.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::json;

/// The root of the golden tree, `tests/goldens/`.
#[must_use]
pub fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

/// Check `value` against the golden at `goldens/<name>.json`, or regenerate.
pub fn check_json_golden(name: &str, value: &impl Serialize) {
    let json = format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("serialize the golden value")
    );
    let path = goldens_dir().join(format!("{name}.json"));

    if std::env::var_os("AMX_UPDATE_GOLDENS").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("creating golden dir {parent:?}: {e}"));
        }
        std::fs::write(&path, &json).unwrap_or_else(|e| panic!("writing golden {path:?}: {e}"));
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("reading golden {path:?}: {e} (run with AMX_UPDATE_GOLDENS=1 to create it)")
    });
    assert_eq!(
        json, expected,
        "golden {name} does not match; run with AMX_UPDATE_GOLDENS=1 to update it after an intentional change"
    );
}

/// Check encoded `bytes` against a golden that pairs them with `message`, a
/// JSON description of what was encoded.
pub fn check_bytes_golden(name: &str, message: &serde_json::Value, bytes: &[u8]) {
    check_json_golden(
        name,
        &json!({
            "message": message,
            "bytes": hex_lines(bytes),
        }),
    );
}

/// `bytes` as lines of sixteen space-separated hex pairs.
fn hex_lines(bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(16)
        .map(|line| {
            line.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Decode one of [`hex_lines`]'s lines back into bytes, for round-trip checks
/// against a stored golden.
#[must_use]
pub fn bytes_of(lines: &[String]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|line| line.split_ascii_whitespace())
        .map(|pair| u8::from_str_radix(pair, 16).expect("a hex byte"))
        .collect()
}
