//! The checksum, between the download and the install.
//!
//! One rule, and it is the reason this module exists as a step rather than a
//! line: **nothing is installed that was not verified first.** The bytes arrive
//! over a channel amx does not control, from a redirect chain amx does not
//! control, and the only thing that makes them trustworthy is that the manifest
//! said what their digest would be. So the digest is computed on the staged
//! file, compared, and a mismatch destroys the staged file — a verified-later
//! design would have a window in which an unverified binary is sitting where
//! the installer looks.
//!
//! `sha2` is the milestone's one new dependency (R-M3-8). The alternatives were
//! weighed and are worse: hand-rolling SHA-256 puts a cryptographic primitive
//! in this tree for one caller, and shelling out means parsing `sha256sum` on
//! Linux and `shasum -a 256` on darwin and deciding which of them the machine
//! has — two output formats and a platform fork, to avoid one small, audited,
//! widely-deployed crate.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use anyhow::Context as _;
use sha2::{Digest as _, Sha256};

/// How much of the file is hashed at a time.
///
/// The file is a binary of tens of megabytes and the digest does not need it in
/// memory, so it never is.
const CHUNK: usize = 64 * 1024;

/// The SHA-256 of the file at `path`, as lowercase hex.
///
/// # Errors
///
/// If the file cannot be opened or read.
pub fn sha256_of(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Check the file at `path` against the digest the manifest published.
///
/// # Errors
///
/// If the file cannot be read, or its digest is not `expected`. The comparison
/// is case-insensitive, because a hex digest is the same number either way and
/// refusing an upper-case manifest would be pedantry with a download attached.
pub fn verify(path: &Path, expected: &str) -> anyhow::Result<()> {
    let actual = sha256_of(path)?;
    let expected = expected.trim().to_ascii_lowercase();
    anyhow::ensure!(
        actual == expected,
        "checksum mismatch: the manifest published sha256 {expected}, \
         the download is {actual}",
    );
    Ok(())
}

/// Bytes as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        // The invariant: writing to a `String` is infallible — `fmt::Write` for
        // `String` never returns `Err` — so there is no error path to handle.
        let _ = write!(out, "{byte:02x}");
        out
    })
}
