//! The release's tag check, run as the release workflow runs it.

use std::path::Path;
use std::process::{Command, Output};

const SCRIPT: &str = ".github/scripts/tag-version.sh";

fn check(root: &Path, tag: &str) -> Output {
    Command::new("sh")
        .arg(root.join(SCRIPT))
        .arg(tag)
        .output()
        .expect("running the tag check")
}

#[test]
fn a_tag_that_is_not_the_package_version_is_refused_by_name() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = check(root, "v9.9.9");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "tag v9.9.9 is not Cargo.toml's {}\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn the_package_version_as_a_tag_passes_and_says_nothing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = check(root, &format!("v{}", env!("CARGO_PKG_VERSION")));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
}

#[test]
fn a_dependency_s_version_is_never_read_as_the_package_s() {
    let root = tempfile::TempDir::new().unwrap();
    let scripts = root.path().join(".github/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(SCRIPT),
        scripts.join("tag-version.sh"),
    )
    .unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[dependencies.clap]\nversion = \"4.6.6\"\n\n\
         [package]\nname = \"amx\"\nversion = \"0.2.0\"\n\n\
         [dev-dependencies.tempfile]\nversion = \"3.27.0\"\n",
    )
    .unwrap();
    assert_eq!(check(root.path(), "v0.2.0").status.code(), Some(0));
    let out = check(root.path(), "v4.6.6");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tag v4.6.6 is not Cargo.toml's 0.2.0\n"
    );
}
