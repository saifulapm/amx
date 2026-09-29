//! opencode's plugin, run by node against what opencode 2.0.16 sent.
//!
//! The plugin is JavaScript loaded into opencode's own TUI, so the suite that
//! holds it to Rulings 2 to 6 is JavaScript too:
//! `tests/opencode_plugin/plugin.test.mjs` replays the events captured in
//! `tests/opencode/events/` into `assets/opencode/tui.js` with a fake ctx and
//! an amx that writes down what it is told. This runs it under `node --test`.

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
