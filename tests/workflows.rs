//! What the release, CI and Pages workflows promise, read off the files
//! themselves, so a change that would publish untested, rerun the whole
//! suite or put main's docs live goes red here rather than on GitHub.

use std::path::Path;

fn read(file: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap()
}

/// The lines of one job, from its `  name:` line to the next job's.
fn job<'a>(workflow: &'a str, name: &str) -> Vec<&'a str> {
    let head = format!("  {name}:");
    let mut lines = workflow.lines().skip_while(|line| *line != head);
    let first = lines.next().unwrap_or_else(|| panic!("no job {name}"));
    std::iter::once(first)
        .chain(lines.take_while(|line| {
            line.is_empty() || line.starts_with("   ") || line.starts_with("  #")
        }))
        .collect()
}

/// Whether every checkout in these lines keeps no credentials.
fn checkouts_keep_no_credentials(lines: &[&str]) -> bool {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("uses: actions/checkout@"))
        .all(|(at, _)| {
            lines[at + 1..]
                .iter()
                .take(3)
                .any(|line| line.trim() == "persist-credentials: false")
        })
}

#[test]
fn a_release_builds_only_after_its_tag_and_its_suite_passed() {
    let release = read(".github/workflows/release.yml");
    let test = job(&release, "test");
    assert!(test.contains(&"    needs: version"), "{test:?}");
    assert!(
        test.contains(&"    uses: ./.github/workflows/ci.yml"),
        "{test:?}"
    );
    let version = job(&release, "version");
    assert!(
        version
            .iter()
            .any(|line| line.contains("sh .github/scripts/tag-version.sh \"$TAG\"")),
        "{version:?}"
    );
    let build = job(&release, "build");
    assert!(build.contains(&"    needs: [version, test]"), "{build:?}");
    assert!(job(&release, "release").contains(&"    needs: build"));
}

#[test]
fn only_the_job_that_publishes_can_write() {
    let release = read(".github/workflows/release.yml");
    assert!(release.contains("\npermissions:\n  contents: read\n"));
    assert_eq!(release.matches("contents: write").count(), 1);
    let publish = job(&release, "release");
    assert!(publish.contains(&"      contents: write"), "{publish:?}");
    for name in ["version", "build"] {
        assert!(
            checkouts_keep_no_credentials(&job(&release, name)),
            "{name} checks out with credentials"
        );
    }

    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("\npermissions:\n  contents: read\n"));
    assert!(ci.contains("\n  workflow_call:\n"), "the release calls it");
    assert!(checkouts_keep_no_credentials(&job(&ci, "check")));
}

#[test]
fn ci_runs_the_suite_once_under_nextest_with_one_retry_and_names_the_flaky() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        !ci.contains("cargo test"),
        "the suite runs under nextest alone"
    );
    assert!(ci.contains("        run: cargo nextest run --profile ci\n"));
    assert!(
        ci.contains("          tool: nextest@0.9.151\n"),
        "the version proven"
    );
    assert!(ci.contains(
        "        if: ${{ !cancelled() }}\n        run: python3 .github/scripts/flaky.py \
         target/nextest/ci/junit.xml >> \"$GITHUB_STEP_SUMMARY\"\n"
    ));

    let profile = read(".config/nextest.toml");
    let ci_profile: Vec<&str> = profile
        .lines()
        .skip_while(|line| *line != "[profile.ci]")
        .take_while(|line| !line.starts_with("[profile.ci.junit]"))
        .collect();
    assert!(ci_profile.contains(&"retries = 1"), "{ci_profile:?}");
    assert!(profile.contains("[profile.ci.junit]\npath = \"junit.xml\"\n"));
}

#[test]
fn the_site_is_built_from_the_newest_tag_and_never_from_main() {
    let pages = read(".github/workflows/pages.yml");
    let on: Vec<&str> = pages
        .lines()
        .skip_while(|line| *line != "on:")
        .take_while(|line| line.is_empty() || line.starts_with(' ') || *line == "on:")
        .collect();
    assert_eq!(
        on,
        [
            "on:",
            "  workflow_run:",
            "    workflows: [Release]",
            "    types: [completed]",
            "  workflow_dispatch:",
            "",
        ]
    );
    let build = job(&pages, "build");
    let picks = build
        .iter()
        .position(|line| line.contains("git checkout --detach \"$tag\""))
        .expect("a checkout of the newest tag");
    let builds = build
        .iter()
        .position(|line| line.contains("run: node build.mjs"))
        .expect("the build");
    assert!(picks < builds, "the tag is checked out before the build");
    assert!(build.iter().any(|line| line.contains("--sort=-v:refname")));
}

#[test]
fn rust_version_is_declared_and_every_source_install_is_locked() {
    let cargo = read("Cargo.toml");
    let package: Vec<&str> = cargo
        .lines()
        .skip_while(|line| *line != "[package]")
        .take_while(|line| !line.starts_with("[dependencies"))
        .collect();
    assert!(
        package
            .iter()
            .any(|line| line.starts_with("rust-version = \"")),
        "{package:?}"
    );

    let mut installs = 0;
    for file in ["README.md", "site/docs/quick-start.md", "install.sh"] {
        for (at, _) in read(file).match_indices("cargo install --") {
            let text = read(file);
            let rest = &text[at..];
            assert!(
                rest.starts_with("cargo install --locked "),
                "{file}: {}",
                rest.lines().next().unwrap()
            );
            installs += 1;
        }
    }
    assert_eq!(installs, 5, "README 1, quick start 2, install.sh 2");
}
