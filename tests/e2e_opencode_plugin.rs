//! Runs the JavaScript tests for amx's opencode plugin under `node --test`.
//!
//! `tests/opencode_plugin/plugin.test.mjs` replays the events captured from
//! opencode 2.0.16 in `tests/opencode/events/` into `assets/opencode/tui.js`,
//! with a fake context and a fake amx that records what it is told.

use std::path::Path;
use std::process::Command;

#[test]
fn the_plugin_reports_the_moments_opencode_sent() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/opencode_plugin/plugin.test.mjs");
    let out = Command::new("node")
        .arg("--test")
        .arg(&suite)
        .output()
        .expect("node runs the plugin's suite");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
