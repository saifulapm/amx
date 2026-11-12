//! The job summary CI writes from nextest's JUnit report.

use std::path::Path;
use std::process::Command;

fn summary(report: &Path) -> String {
    let out = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/scripts/flaky.py"))
        .arg(report)
        .output()
        .expect("running the summary script");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_test_that_passed_on_its_retry_is_named_under_flaky() {
    let dir = tempfile::TempDir::new().unwrap();
    let report = dir.path().join("junit.xml");
    std::fs::write(
        &report,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" failures="0" errors="0">
  <testsuite name="amx::bin/amx" tests="2" failures="0" errors="0">
    <testcase name="tests::steady" classname="amx::bin/amx" time="0.01"/>
    <testcase name="tests::wobbly" classname="amx::bin/amx" time="0.02">
      <flakyFailure type="test failure" message="code 101"/>
    </testcase>
  </testsuite>
  <testsuite name="amx::e2e_wall" tests="1" failures="0" errors="0">
    <testcase name="a_slow_frame" classname="amx::e2e_wall" time="1.5">
      <flakyError type="test failure" message="timed out"/>
    </testcase>
  </testsuite>
</testsuites>
"#,
    )
    .unwrap();
    assert_eq!(
        summary(&report),
        "## flaky\n\n- `amx::bin/amx tests::wobbly`\n- `amx::e2e_wall a_slow_frame`\n"
    );
}

#[test]
fn the_flaky_heading_is_there_with_none_and_with_no_report() {
    let dir = tempfile::TempDir::new().unwrap();
    let report = dir.path().join("junit.xml");
    std::fs::write(
        &report,
        r#"<testsuites><testsuite name="amx::bin/amx"><testcase name="tests::steady" classname="amx::bin/amx"/></testsuite></testsuites>"#,
    )
    .unwrap();
    assert_eq!(
        summary(&report),
        "## flaky\n\nNone: every test passed on its first try.\n"
    );

    let none = summary(&dir.path().join("missing.xml"));
    assert!(
        none.starts_with("## flaky\n\nNo test report to read:"),
        "{none:?}"
    );
}
