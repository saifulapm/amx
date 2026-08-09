//! The module budget this crate's own layout owes (04 §9, R-M1-3).
//!
//! Its own test binary, deliberately: it asserts a property of the source
//! tree, so it must still run — and still fail with a readable message —
//! when the code those files hold does not compile.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::Path;

/// R-M1-3: `app/mod.rs` was over the 500-line soft budget before this task,
/// and the status work is what moves out of it (`docs/07-m1-plan.md` §4).
///
/// The workspace-wide check in `scripts/check-module-size.sh` only *warns* at
/// the soft budget, by design — it fails at the hard one. This asserts the
/// specific outcome the task owes.
#[test]
fn app_status_module_is_extracted_and_mod_rs_returns_under_soft_budget() {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    let lines = |name: &str| {
        std::fs::read_to_string(app.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .lines()
            .count()
    };

    // `status.rs` became `status/` when D15's breakdown, D14's compact form and
    // the clock their ages are rendered against crossed the same budget (X11).
    // What the original assertion is about is unchanged: the status line lives
    // outside `app/mod.rs` and holds real code, wherever inside `status/` it
    // sits.
    let status = ["status/mod.rs", "status/line.rs", "status/clock.rs"];
    for name in status {
        let held = lines(name);
        assert!(held > 40, "app/{name} must hold code, not a re-export");
        assert!(
            held <= 500,
            "app/{name} is {held} lines, over the 500-line soft budget",
        );
    }
    let modrs = lines("mod.rs");
    assert!(
        modrs <= 500,
        "app/mod.rs is {modrs} lines, over the 500-line soft budget",
    );
}
