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
//! stops being permanent is a test that fails once the milestone ships. M2
//! opened twelve and V17 closed the last two, so the guard below is back in its
//! resting state: no call sites, and no helper to make one from.

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

/// The milestone guard, in its resting state: **no dispatch seam exists.**
///
/// A row that lands before its wiring is answered through a `seam` helper
/// rather than `METHOD_NOT_FOUND`, because telling a client a method is unknown
/// tells it to stop offering it. The helper is therefore a milestone's tool,
/// and this test is what keeps it one: while a milestone is being built the
/// list of owning tasks is non-empty and every call site must name one; when
/// the integration task closes the last row, the helper, the list and the
/// exemption go together.
///
/// That has now happened twice. U01 introduced the helper with M1's two rows,
/// U06 and U07 closed them, and both retired. V02 brought both back for M2's
/// twelve; V12 closed four, V09 one, V11 three, V13 two, and **V17 closed
/// `agent.explain` and `agent.next` and deleted the helper** — which is M2's
/// exit check, stated in `dispatch/mod.rs` and enforced here.
///
/// So the assertion is now the empty one: no `seam(` call site, and no helper
/// to make one from. A milestone that wants seams again writes the helper, and
/// rewrites this test with its own owner list — the deliberate friction that
/// stops a seam from quietly outliving the milestone that opened it.
#[test]
fn no_dispatch_seam_outlives_the_milestone_that_opened_it() {
    // `<workspace>/tests/../crates`: the shipped code, not the suites, since a
    // test harness may legitimately name the concept.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates");
    // A call or a definition, not the word: `seam` is the tree's ordinary
    // noun for a trait boundary (`platform.rs`, `persist/io.rs`) and banning
    // the word would ban the vocabulary.
    let call = "seam(";

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
                // Prose says "the seam (`Pty`, `Ipc`)"; code says `seam(…)`.
                if !line.contains(call) || line.trim_start().starts_with("//") {
                    continue;
                }
                found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }

    assert!(
        found.is_empty(),
        "M2's seam ledger is empty and the helper is deleted, so a `seam(…)` \
         call site can only be a row that landed without wiring. Implement it, \
         or reintroduce the helper *with* the owner list this test used to \
         carry:\n{}",
        found.join("\n")
    );
    assert!(
        scanned >= 50,
        "the crates scan read too few source files ({scanned}) to be believed"
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
