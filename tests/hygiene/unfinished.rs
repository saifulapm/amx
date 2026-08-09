//! `docs/11-m4-plan.md` §7's exit item 8, made mechanical.
//!
//! A child module of [`super`] for the reason [`super::ledgers`] is one — it is
//! a rule about the shipped code rather than about the suites, and the parent
//! was at its budget. Unlike the ledgers this one is meant to be *kept*: it
//! outlives M4.

use std::fs;
use std::path::Path;

use super::rust_files;

/// No shipped function is left as a `todo!()`.
///
/// `docs/11-m4-plan.md` §7's exit item 8 asks for this in words, and words are
/// how it stayed true for three milestones and then stopped: DR-6's two
/// `todo!()`s sat in `amx-core/src/id.rs` from M1 to M4 while `Core` ran a
/// documented stand-in beside them and *persisted its output*, so every day the
/// register went unpaid added snapshots shaped by a mapping nobody had written.
/// X05 paid it; this is what keeps it paid.
///
/// The check is deliberately narrow — `crates/*/src`, a call and not the word,
/// comment lines skipped — because prose about an unfinished body is exactly
/// how the tree discusses this class of debt (`agent/resume.rs` carries such a
/// sentence) and banning the discussion would ban the record.
#[test]
fn no_shipped_function_is_left_unfinished() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    // Assembled rather than written, so this file does not match itself.
    let call = ["to", "do!("].concat();

    let mut found = Vec::new();
    let mut scanned = 0;
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let src = krate.expect("a directory entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        for path in rust_files(&src) {
            scanned += 1;
            let text = fs::read_to_string(&path).expect("read a source file");
            for (n, line) in text.lines().enumerate() {
                if line.contains(&call) && !line.trim_start().starts_with("//") {
                    found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        found.is_empty(),
        "a shipped body is unfinished. A contract with no implementation gets \
         built around — and the thing built around it persists, which is DR-6's \
         whole history. Implement it or delete it:\n{}",
        found.join("\n")
    );
    assert!(
        scanned >= 50,
        "the crates scan read too few source files ({scanned}) to be believed"
    );
}
