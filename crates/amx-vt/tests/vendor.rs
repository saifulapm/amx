#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The vendored tree has to describe itself honestly: the recorded commit must
//! match what was extracted, the patch manifest must list every file that
//! diverges from the dist, and a wrong Zig must fail with something a
//! contributor can act on rather than a compile error from inside zig's
//! standard library.

use std::fs;
use std::path::{Path, PathBuf};

// The same code `build.rs` runs, compiled into the test.
#[path = "../build/toolchain.rs"]
mod toolchain;

fn vendor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .canonicalize()
        .expect("the vendor directory exists")
}

fn manifest() -> serde_json::Value {
    let path = vendor_dir().join("libghostty-vt.vendor.json");
    serde_json::from_str(&fs::read_to_string(&path).expect("vendor manifest is readable"))
        .expect("vendor manifest is json")
}

fn text(value: &serde_json::Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("manifest key {key} is a string"))
        .to_string()
}

#[test]
fn vendored_source_matches_recorded_commit() {
    let manifest = manifest();
    let commit = text(&manifest, "source_commit");
    assert_eq!(commit.len(), 40, "source_commit is a full sha: {commit}");

    // The dist stamps the abbreviated commit into VERSION, which is the only
    // evidence inside the tree of where it came from. Everything else in the
    // manifest is derived from that same string, so all three must agree.
    let version = fs::read_to_string(vendor_dir().join("libghostty-vt/VERSION"))
        .expect("the vendored tree has a VERSION");
    let version = version.trim();
    let short = version
        .rsplit_once('+')
        .map(|(_, short)| short)
        .unwrap_or_else(|| panic!("VERSION carries a +<short commit> suffix: {version}"));

    assert!(
        commit.starts_with(short),
        "VERSION is {version} but the manifest records {commit}"
    );
    assert_eq!(
        text(&manifest, "extracted_dir"),
        format!("libghostty-vt-{version}"),
    );
    assert_eq!(
        text(&manifest, "dist_archive"),
        format!("libghostty-vt-{version}.tar.gz"),
    );
    assert_eq!(
        text(&manifest, "zig_version"),
        toolchain::REQUIRED_ZIG_VERSION
    );
}

#[test]
fn patch_manifest_lists_every_file_diverging_from_the_dist() {
    let vendor = vendor_dir();
    let patches_md = fs::read_to_string(vendor.join("libghostty-vt.patches.md"))
        .expect("the patch manifest is readable");
    let manifest = manifest();
    let recorded: Vec<String> = manifest["patches"]
        .as_array()
        .expect("manifest lists patches")
        .iter()
        .map(|entry| entry.as_str().expect("patch paths are strings").to_string())
        .collect();

    let mut on_disk: Vec<String> = Vec::new();
    let patch_dir = vendor.join("patches/libghostty-vt");
    if patch_dir.is_dir() {
        for entry in fs::read_dir(&patch_dir).expect("patch directory is readable") {
            let path = entry.expect("patch directory entry").path();
            if path.extension().is_some_and(|ext| ext == "patch") {
                on_disk.push(format!(
                    "vendor/patches/libghostty-vt/{}",
                    path.file_name()
                        .expect("patch has a name")
                        .to_string_lossy()
                ));
            }
        }
    }
    on_disk.sort();
    let mut recorded_sorted = recorded.clone();
    recorded_sorted.sort();
    assert_eq!(
        on_disk, recorded_sorted,
        "vendor.json's patch list and vendor/patches/libghostty-vt/ disagree"
    );

    for patch in &on_disk {
        assert!(
            patches_md.contains(patch),
            "{patch} is applied but has no section in libghostty-vt.patches.md"
        );
        // Every file the patch rewrites has to be named under `local files:`,
        // which is what makes the manifest a complete list of divergence.
        let body = fs::read_to_string(vendor.join("..").join(patch)).expect("patch is readable");
        for line in body.lines().filter(|line| line.starts_with("+++ b/")) {
            let file = line.trim_start_matches("+++ b/").trim();
            assert!(
                patches_md.contains(file),
                "{patch} rewrites {file}, which libghostty-vt.patches.md does not list"
            );
            assert!(
                vendor.join("..").join(file).is_file(),
                "{patch} names {file}, which is not in the vendored tree"
            );
        }
    }
}

/// The patch manifest says how the tree differs from the dist; the checksum
/// file says nothing else has been touched since. Together they are the claim
/// that `vendor/libghostty-vt/` is the pinned dist plus the listed patches and
/// nothing more.
#[test]
fn vendored_tree_matches_recorded_checksums() {
    let vendor = vendor_dir();
    let checksums = vendor.join("libghostty-vt.sha256");
    let recorded = fs::read_to_string(&checksums).expect("the checksum file is readable");
    let tree = vendor.join("libghostty-vt");

    // Files first: a deletion or an addition never reaches the hash check.
    let mut listed: Vec<String> = recorded
        .lines()
        .filter_map(|line| line.split_once("  ").map(|(_, path)| path.to_string()))
        .collect();
    listed.sort();
    let mut present = Vec::new();
    walk(&tree, &tree, &mut present);
    present.sort();
    assert_eq!(
        present, listed,
        "the vendored tree and libghostty-vt.sha256 list different files; \
         re-run scripts/vendor-libghostty-vt.sh sync"
    );

    let (program, args) = checksum_command();
    let output = std::process::Command::new(program)
        .args(args)
        .arg(&checksums)
        .current_dir(&tree)
        .output()
        .unwrap_or_else(|err| panic!("cannot run {program}: {err}"));
    assert!(
        output.status.success(),
        "the vendored tree does not match libghostty-vt.sha256:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// coreutils on Linux, the BSD/perl one on macOS — the script writes whichever
/// it found and both read the same format back.
fn checksum_command() -> (&'static str, &'static [&'static str]) {
    if std::process::Command::new("sha256sum")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        ("sha256sum", &["--check", "--quiet", "--strict"])
    } else {
        ("shasum", &["-a", "256", "--check", "--status"])
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("the vendored tree is readable") {
        let path = entry.expect("directory entry").path();
        let relative = path.strip_prefix(root).expect("paths are under the root");
        // The dist's own build output is not part of what was vendored.
        if matches!(
            relative.components().next().map(|c| c.as_os_str()),
            Some(name) if name == ".zig-cache" || name == "zig-out"
        ) {
            continue;
        }
        // Symlinks count as entries, not as directories to descend into:
        // the dist ships CLAUDE.md as a link to AGENTS.md.
        let metadata = path.symlink_metadata().expect("entry metadata");
        if metadata.is_dir() {
            walk(root, &path, out);
        } else {
            out.push(format!("./{}", relative.display()));
        }
    }
}

/// A zig of the wrong version must be reported as such, naming the fix.
#[test]
fn build_fails_with_clear_message_on_wrong_zig_version() {
    let dir = std::env::temp_dir().join(format!("amx-vt-zig-probe-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch directory");
    let shim = dir.join("zig");
    fs::write(&shim, "#!/bin/sh\necho 0.16.0\n").expect("write the shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod the shim");
    }

    let error = toolchain::find_zig(&vendor_dir(), Some(shim.clone()))
        .expect_err("0.16.0 is not the pinned version");

    assert!(
        error.contains("0.16.0"),
        "message names what was found: {error}"
    );
    assert!(
        error.contains(toolchain::REQUIRED_ZIG_VERSION),
        "message names what is required: {error}"
    );
    assert!(
        error.contains("scripts/vendor-libghostty-vt.sh toolchain"),
        "message names the command that fixes it: {error}"
    );
    assert!(
        error.contains("AMX_LIBGHOSTTY_VT_LIB_DIR"),
        "message names the escape hatch: {error}"
    );

    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn missing_zig_is_reported_with_the_command_that_installs_it() {
    let error = toolchain::find_zig(
        Path::new("/nonexistent-vendor-dir"),
        Some(PathBuf::from("/nonexistent/zig")),
    )
    .expect_err("there is no compiler there");
    assert!(
        error.contains("scripts/vendor-libghostty-vt.sh toolchain"),
        "message names the command that fixes it: {error}"
    );
}

#[test]
fn unsupported_targets_are_named_rather_than_guessed() {
    assert_eq!(
        toolchain::zig_target("x86_64-unknown-linux-gnu").expect("a mapped target"),
        "x86_64-linux-gnu"
    );
    let error = toolchain::zig_target("mips64-unknown-linux-gnuabi64")
        .expect_err("unmapped targets are an error, not a guess");
    assert!(error.contains("mips64-unknown-linux-gnuabi64"), "{error}");
}
