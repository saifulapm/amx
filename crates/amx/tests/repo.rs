//! Repository hygiene: licensing, the CI entry point, and the module budget.
//!
//! These drive the scripts the way CI does — as processes, with their real exit
//! codes — because a check that is only ever asserted about in Rust is a check
//! that can pass here and fail in the workflow.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{fs, io};

/// Every crate in the workspace, as listed in the root manifest.
const CRATES: [&str; 6] = [
    "amx",
    "amx-client",
    "amx-core",
    "amx-proto",
    "amx-server",
    "amx-vt",
];

const LICENSE: &str = "MIT OR Apache-2.0";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<name>/ sits two levels below the repository root")
        .to_path_buf()
}

#[test]
fn all_crates_are_dual_licensed() {
    let root = repo_root();

    for name in CRATES {
        let manifest_path = root.join("crates").join(name).join("Cargo.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("{}: {e}", manifest_path.display()));

        let declared = manifest
            .lines()
            .find_map(|line| line.strip_prefix("license = "))
            .unwrap_or_else(|| panic!("{name} declares no license field"));

        assert_eq!(
            declared.trim().trim_matches('"'),
            LICENSE,
            "{name} must be dual licensed"
        );
    }

    // The texts the field points at have to be in the tree, or the field is a
    // claim about files nobody shipped.
    for file in ["LICENSE-MIT", "LICENSE-APACHE"] {
        let text = fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(text.len() > 500, "{file} looks like a stub");
    }
    assert!(
        fs::read_to_string(root.join("LICENSE-MIT"))
            .unwrap()
            .contains("MIT License")
    );
    assert!(
        fs::read_to_string(root.join("LICENSE-APACHE"))
            .unwrap()
            .contains("Apache License")
    );
}

#[test]
fn workspace_members_are_exactly_the_six_crates() {
    let manifest = fs::read_to_string(repo_root().join("Cargo.toml")).unwrap();
    for name in CRATES {
        assert!(
            manifest.contains(&format!("\"crates/{name}\"")),
            "{name} is not a workspace member"
        );
    }
    assert!(manifest.contains("resolver = \"3\""));
    assert!(manifest.contains("edition = \"2024\""));
}

#[test]
fn ci_script_passes_on_empty_workspace() {
    let root = repo_root();
    let script = root.join("scripts/ci.sh");
    assert!(script.is_file(), "scripts/ci.sh is the CI entry point");

    // Run the real script in dry-run mode: it must succeed and it must be the
    // pipeline the workflow relies on. Executing the cargo steps here would
    // re-enter the `cargo test` that is running this test.
    let output = Command::new(&script)
        .env("AMX_CI_DRY_RUN", "1")
        .output()
        .unwrap_or_else(|e| panic!("running ci.sh: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "ci.sh failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for step in [
        "cargo fmt --all --check",
        "cargo clippy",
        "cargo test",
        "scripts/check-module-size.sh",
    ] {
        assert!(
            stdout.contains(step),
            "ci.sh does not run `{step}`:\n{stdout}"
        );
    }
    assert!(stdout.contains("ci: ok"));

    // And the one step that is cheap to run for real, run for real: the
    // workspace is under budget right now.
    let sizes = Command::new(root.join("scripts/check-module-size.sh"))
        .output()
        .unwrap_or_else(|e| panic!("running check-module-size.sh: {e}"));
    assert!(
        sizes.status.success(),
        "the workspace is over the module budget:\n{}",
        String::from_utf8_lossy(&sizes.stderr)
    );
}

#[test]
fn module_size_check_fails_at_1001_lines() {
    let dir = TempDir::new("module-size-hard");
    write_lines(&dir.path().join("crates/amx-core/src/huge.rs"), 1001);

    let output = run_size_check(dir.path());
    assert!(
        !output.status.success(),
        "1001 lines must fail the hard budget"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("crates/amx-core/src/huge.rs"),
        "the failure must name the file"
    );

    // 1000 lines is the limit, not one line past it.
    let dir = TempDir::new("module-size-edge");
    write_lines(&dir.path().join("crates/amx-core/src/exactly.rs"), 1000);
    let output = run_size_check(dir.path());
    assert!(output.status.success(), "1000 lines is inside the budget");

    // And the soft budget warns without failing.
    let dir = TempDir::new("module-size-soft");
    write_lines(&dir.path().join("crates/amx-core/src/large.rs"), 501);
    let output = run_size_check(dir.path());
    assert!(output.status.success(), "501 lines warns, it does not fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("warning:"));
}

#[test]
fn module_size_check_exempts_generated_sys_module() {
    let dir = TempDir::new("module-size-generated");
    // Generated code is exempt (04 §9): bindgen output, wherever it lands.
    write_lines(&dir.path().join("crates/amx-vt/src/sys.rs"), 4001);
    write_lines(
        &dir.path().join("target/debug/build/x/out/bindings.rs"),
        4001,
    );
    write_lines(&dir.path().join("vendor/libghostty-vt/huge.rs"), 4001);

    let output = run_size_check(dir.path());
    assert!(
        output.status.success(),
        "generated and vendored code is exempt:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The exemption is by path, not by size: a hand-written neighbour of the
    // generated module is still held to the budget.
    write_lines(&dir.path().join("crates/amx-vt/src/terminal.rs"), 1001);
    assert!(!run_size_check(dir.path()).status.success());
}

fn run_size_check(root: &Path) -> std::process::Output {
    Command::new(repo_root().join("scripts/check-module-size.sh"))
        .arg(root)
        .output()
        .unwrap_or_else(|e| panic!("running check-module-size.sh: {e}"))
}

fn write_lines(path: &Path, lines: usize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("{}: {e}", parent.display()));
    }
    let mut body = String::with_capacity(lines * 16);
    for n in 0..lines {
        body.push_str(&format!("// generated line {n}\n"));
    }
    fs::write(path, body).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// A directory under `$TMPDIR` that removes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("amx-{label}-{}-{unique}", std::process::id()));
        if let Err(e) = fs::remove_dir_all(&path)
            && e.kind() != io::ErrorKind::NotFound
        {
            panic!("{}: {e}", path.display());
        }
        fs::create_dir_all(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
