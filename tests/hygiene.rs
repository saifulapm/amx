//! W12's guard: no test in this rig waits on the wall clock.
//!
//! The failure mode being kept out: a test that naps a fixed interval and
//! then asserts, passing on a fast machine and flaking on a loaded one. Every
//! wait in this package is a condition plus a deadline — the deadline's expiry
//! is a *failure*, never the green path — and this suite enforces that shape
//! mechanically, because a convention nothing checks is a convention on its
//! way out.

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
    let mut suites = 0;
    for entry in fs::read_dir(root).expect("read the package root") {
        let path = entry.expect("a directory entry").path();
        // This file is exempt for one reason only: the acceptance-test name
        // itself contains the forbidden token.
        if path.file_name().is_some_and(|name| name == "hygiene.rs") {
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            let text = fs::read_to_string(&path).expect("read the suite");
            assert!(
                !text.contains(&needle),
                "{} calls {needle}; wait on a condition instead (rig::wait_until, \
                 Terminal::wait_output, or an interval tick)",
                path.display()
            );
            suites += 1;
        }
    }
    assert!(suites >= 3, "the scan found too few suites to be believed");

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
