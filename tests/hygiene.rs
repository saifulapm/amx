//! W12's guard: no test in this rig waits on the wall clock.
//!
//! The failure mode being kept out: a test that naps a fixed interval and
//! then asserts, passing on a fast machine and flaking on a loaded one. Every
//! wait in this package is a condition plus a deadline — the deadline's expiry
//! is a *failure*, never the green path — and this suite enforces that shape
//! mechanically, because a convention nothing checks is a convention on its
//! way out.
//!
//! The second guard here is the same idea one milestone up: a *seam* — a
//! method that landed in the shared table before its implementation — is
//! allowed to exist only while its milestone is being built, and the way that
//! stops being permanent is a test that fails once the milestone ships. M2's
//! twelve are declared below with the task that closes each; V17 empties the
//! list and deletes the helper together.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::path::Path;

/// The forbidden token, assembled so this file does not match itself.
fn needle() -> String {
    ["sl", "eep"].concat()
}

#[test]
fn no_test_depends_on_wall_clock_sleep() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let needle = needle();

    // The suites: no sleeping at all, under any name. Waiting is done through
    // the support crate's condition-driven helpers, and observation windows
    // (the flood test) tick an interval instead of napping blind.
    //
    // Recursive, because suites grew module directories — `persistence/`,
    // `seams/` — and a scan of the package root alone had quietly stopped
    // covering most of the rig's test code. `support/` is walked separately
    // below under its own, looser rule.
    let mut suites = 0;
    for path in rust_files(root) {
        if path.starts_with(root.join("support")) {
            continue;
        }
        // This file is exempt for one reason only: the acceptance-test name
        // itself contains the forbidden token.
        if path.file_name().is_some_and(|name| name == "hygiene.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read the suite");
        assert!(
            !text.contains(&needle),
            "{} calls {needle}; wait on a condition instead (rig::wait_until, \
             Terminal::wait_output, or an interval tick)",
            path.display()
        );
        suites += 1;
    }
    assert!(suites >= 10, "the scan found too few suites to be believed");

    // The support crate: the poll tick inside its deadline loops is the one
    // place a nap is allowed, and every such line names TICK so a bare
    // interval cannot hide there.
    let mut waits = 0;
    for entry in fs::read_dir(root.join("support")).expect("read support/") {
        let path = entry.expect("a directory entry").path();
        if !path.extension().is_some_and(|ext| ext == "rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read the support module");
        for (n, line) in text.lines().enumerate() {
            if line.contains(&needle) {
                assert!(
                    line.contains("(TICK)"),
                    "{}:{} naps outside a TICK-paced deadline loop: {line}",
                    path.display(),
                    n + 1
                );
                waits += 1;
            }
        }
    }
    assert!(
        waits > 0,
        "the support crate's wait helpers have gone missing; this guard checks them"
    );
}

/// The same convention where the naps used to live: `crates/*/tests`.
///
/// Those suites legitimately *say* the forbidden word — spawned shells run
/// `sleep 60` to stay alive, and that is program text under test, not a test
/// waiting — so the scan matches the Rust call (`thread::sleep`,
/// `tokio::time::sleep`) rather than the word. Every call must either pace a
/// deadline loop at a named `TICK`-family constant (the line names `(TICK)`)
/// or be a scheduling window the test opens on purpose — a controlled ingest
/// rate, an adversarial hold — marked `// deliberate` on the same line, with
/// the justification above it. A deliberate window must never be load-bearing
/// for the green path: expiring long or short may weaken the test, never
/// redden it.
#[test]
fn crate_tests_wait_on_conditions_not_wall_clock() {
    // `<workspace>/tests/../crates`: this package sits beside `crates/`.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    let call = ["::sl", "eep"].concat();

    let mut suites = 0;
    let mut flagged = Vec::new();
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let tests = krate.expect("a directory entry").path().join("tests");
        if !tests.is_dir() {
            continue;
        }
        for path in rust_files(&tests) {
            suites += 1;
            let text = fs::read_to_string(&path).expect("read the suite");
            for (n, line) in text.lines().enumerate() {
                if line.contains(&call)
                    && !line.contains("(TICK)")
                    && !line.trim_end().ends_with("// deliberate")
                {
                    flagged.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        flagged.is_empty(),
        "wall-clock naps outside a TICK-paced loop; wait on a condition, or \
         mark a justified scheduling window `// deliberate`:\n{}",
        flagged.join("\n")
    );
    assert!(
        suites >= 10,
        "the crates scan found too few suites ({suites}) to be believed"
    );
}

/// The tasks allowed to own a dispatch seam while M2 is being built.
///
/// The exemption `dispatch/mod.rs` describes: U01 introduced the `seam` helper
/// with M1's two rows, U06 and U07 closed them, and helper and exemption
/// retired together. V02 reintroduces both, for M2's twelve rows, and V17
/// deletes both again — which is what stops a seam from quietly outliving the
/// milestone that opened it. An empty list here means the helper must be gone
/// from the tree entirely.
///
/// The names are the wave tasks of `docs/08-m2-plan.md` §5. Every `seam(` call
/// site must name one, so a seam nobody owns cannot be written — that is T19's
/// and U01's lesson (exclusive file ownership leaves the *seams* unowned by
/// construction) applied to the dispatch table itself.
///
/// A task drops off this list when it lands: **V12** closed the four
/// pane-driving rows of §4 and removed itself here in the same commit, which is
/// the bookkeeping that makes the count below mean something.
const SEAM_OWNERS: &[&str] = &["V06", "V08", "V09", "V11", "V13"];

/// How many dispatch seams are still open.
///
/// V02 opened twelve, one per row of §4's table; V12 closed four
/// (`pane.send_text`, `pane.send_keys`, `pane.run`, `pane.read`). The count is
/// here rather than only in the plan so that closing a seam without deleting
/// its call site, or opening a thirteenth, fails a test instead of passing a
/// review — and so that a wave task landing has to say so here.
const SEAM_COUNT: usize = 8;

/// The milestone guard: every dispatch seam names the task that closes it.
///
/// A row that lands before its wiring is answered through a `seam` helper
/// rather than `METHOD_NOT_FOUND`, because telling a client a method is unknown
/// tells it to stop offering it. This test is what keeps that temporary: while
/// [`SEAM_OWNERS`] is non-empty each call site must name a task from it, and
/// when M2's integration task empties the list the helper has to go with it.
#[test]
fn every_dispatch_seam_names_the_task_that_closes_it() {
    // `<workspace>/tests/../crates`: the shipped code, not the suites, since a
    // test harness may legitimately name the concept.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    // A call or a definition, not the word: `seam` is the tree's ordinary
    // noun for a trait boundary (`platform.rs`, `persist/io.rs`) and banning
    // the word would ban the vocabulary.
    let call = "seam(";

    let mut unowned = Vec::new();
    let mut owned = 0;
    for krate in fs::read_dir(&crates).expect("read crates/") {
        let src = krate.expect("a directory entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        for path in rust_files(&src) {
            let text = fs::read_to_string(&path).expect("read a source file");
            for (n, line) in text.lines().enumerate() {
                // Prose says "the seam (`Pty`, `Ipc`)"; code says `seam(…)`.
                if !line.contains(call) || line.trim_start().starts_with("//") {
                    continue;
                }
                // The helper's own definition is not a seam.
                if line.contains("fn seam") {
                    continue;
                }
                if SEAM_OWNERS.iter().any(|owner| line.contains(owner)) {
                    owned += 1;
                } else {
                    unowned.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        unowned.is_empty(),
        "a dispatch seam names no task that closes it; every `seam(…)` call \
         passes the owning task from {SEAM_OWNERS:?} as its second argument:\n{}",
        unowned.join("\n")
    );
    assert_eq!(
        owned, SEAM_COUNT,
        "M2 opened {SEAM_COUNT} seams (docs/08-m2-plan.md §4's twelve rows); \
         found {owned}. Closing one means deleting its call site, and closing \
         the last means deleting the helper and emptying SEAM_OWNERS.",
    );
    assert!(
        !SEAM_OWNERS.is_empty() || owned == 0,
        "with no owners declared, no seam may exist at all",
    );
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).expect("read a test directory");
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}
