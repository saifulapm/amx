//! W10 acceptance: `amx update check|apply` (D-M3-8).
//!
//! Every test here drives the real binary, and the two that install drive a
//! *copy* of it — because the thing under test is a process replacing the file
//! it is running from, and a test that stubbed the path would be testing its
//! own stub. The copies also make the package-manager fixtures real: `amx
//! update apply` reads `std::env::current_exe()`, so a binary sitting at
//! `…/Cellar/amx/<version>/bin/amx` is genuinely a Homebrew install as far as
//! the code can tell.
//!
//! Nothing here touches the network. The channel is a `file://` URL every time,
//! which is what D-M3-8 says CI tests against and is the only honest option
//! while no manifest is published anywhere (R-M3-4).

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use amx::update::pm::{self, Install};
use amx::update::verify::sha256_of;
use amx_server::session::probe::{Probe, probe, server_pid};

mod support;

use support::{Rig, TempDir};

/// The version this build is, which every manifest here is written against.
const CURRENT: &str = env!("CARGO_PKG_VERSION");

// ------------------------------------------------------------------ the rig

/// The manifest half of the fixture, which is this suite's alone.
///
/// The rest — roots, planting a copy of the binary at an arbitrary path,
/// running it, serving from it — lives in [`support::rig`], where W11 lifted it
/// so the bridge and worktree suites could use it too.
trait Manifests {
    /// Write a manifest offering `version` of `asset`, and return its URL.
    fn manifest(&self, name: &str, version: &str, asset: Option<(&Path, &str)>) -> String;
}

impl Manifests for Rig {
    fn manifest(&self, name: &str, version: &str, asset: Option<(&Path, &str)>) -> String {
        let assets = match asset {
            Some((path, sha256)) => format!(
                "{{\"{}\": {{\"url\": \"file://{}\", \"sha256\": \"{sha256}\"}}}}",
                platform_key(),
                path.display()
            ),
            None => "{}".to_owned(),
        };
        let path = self.root().join(name);
        fs::write(
            &path,
            format!(
                "{{\"version\": \"{version}\", \"notes\": \"Release notes for {version}.\", \
                 \"assets\": {assets}}}"
            ),
        )
        .expect("write the manifest");
        format!("file://{}", path.display())
    }
}

/// The asset key this build looks for.
fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// A version one patch above this build's.
fn newer() -> String {
    let (major, minor, patch) = triple();
    format!("{major}.{minor}.{}", patch + 1)
}

/// A version below this build's, whatever this build's is.
fn older() -> String {
    let (major, minor, patch) = triple();
    if patch > 0 {
        format!("{major}.{minor}.{}", patch - 1)
    } else if minor > 0 {
        format!("{major}.{}.999", minor - 1)
    } else {
        assert!(major > 0, "0.0.0 has nothing below it");
        format!("{}.999.999", major - 1)
    }
}

fn triple() -> (u64, u64, u64) {
    let mut parts = CURRENT
        .split('.')
        .map(|part| part.parse().expect("a number"));
    (
        parts.next().expect("major"),
        parts.next().expect("minor"),
        parts.next().expect("patch"),
    )
}

/// A stand-in for "the newer amx": a script that says which one it is.
///
/// Executable and self-identifying, so the install test can prove the file at
/// the path really was replaced by *this* content rather than merely changed.
fn payload(rig: &Rig, marker: &str) -> (PathBuf, String) {
    let path = rig.root().join("amx-next");
    fs::write(&path, format!("#!/bin/sh\necho '{marker}'\n")).expect("write the payload");
    let sha256 = sha256_of(&path).expect("digest the payload");
    (path, sha256)
}

// -------------------------------------------------------------------- check

#[test]
fn check_against_a_file_url_manifest_reports_newer_older_equal() {
    let rig = Rig::new("chk");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    let asset = rig.root().join("amx-next");
    fs::write(&asset, b"not downloaded by check").expect("write an asset");
    let sha256 = sha256_of(&asset).expect("digest");

    let newer = newer();
    rig.channel(&rig.manifest("newer.json", &newer, Some((&asset, &sha256))));
    let out = rig.run(&exe, &["update", "check"]).ok().to_owned();
    assert!(out.contains(&format!("{newer} is available")), "{out}");
    assert!(
        out.contains(&format!("Release notes for {newer}.")),
        "the notes the manifest published are what the user reads: {out}"
    );
    assert!(out.contains("run `amx update apply`"), "{out}");

    rig.channel(&rig.manifest("same.json", CURRENT, Some((&asset, &sha256))));
    let out = rig.run(&exe, &["update", "check"]).ok().to_owned();
    assert!(
        out.contains(&format!("{CURRENT} is the newest published version")),
        "{out}"
    );
    assert!(
        !out.contains("is available"),
        "equal is not an upgrade: {out}"
    );

    let older = older();
    rig.channel(&rig.manifest("older.json", &older, Some((&asset, &sha256))));
    let out = rig.run(&exe, &["update", "check"]).ok().to_owned();
    assert!(
        out.contains(&format!("the channel publishes {older}")) && out.contains("older than"),
        "{out}"
    );
    assert!(
        out.contains("installs nothing"),
        "a rollback is reported, never taken: {out}"
    );

    // check reads; it never writes. Nothing was staged by any of the three.
    assert!(!rig.staging().exists(), "check staged something");
}

#[test]
fn check_takes_a_channel_of_its_own_and_it_beats_the_configured_one() {
    let rig = Rig::new("chch");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    let asset = rig.root().join("amx-next");
    fs::write(&asset, b"not downloaded by check").expect("write an asset");
    let sha256 = sha256_of(&asset).expect("digest");

    // The configured channel says there is nothing newer; the one named on the
    // command line says there is. `--channel` was on `apply` only until W14,
    // which left the *read-only* verb unable to look anywhere but the config
    // file — backwards, since looking elsewhere is the half that risks nothing.
    rig.channel(&rig.manifest("configured.json", CURRENT, Some((&asset, &sha256))));
    let newer = newer();
    let elsewhere = rig.manifest("elsewhere.json", &newer, Some((&asset, &sha256)));

    let out = rig
        .run(&exe, &["update", "check", "--channel", &elsewhere])
        .ok()
        .to_owned();
    assert!(
        out.contains(&elsewhere),
        "it says which channel it read: {out}"
    );
    assert!(out.contains(&format!("{newer} is available")), "{out}");

    // Without the flag the configured channel still wins, so the argument is
    // an override and not a new default.
    let out = rig.run(&exe, &["update", "check"]).ok().to_owned();
    assert!(
        out.contains(&format!("{CURRENT} is the newest published version")),
        "{out}"
    );
    assert!(!rig.staging().exists(), "check staged something");
}

#[test]
fn no_published_manifest_is_reported_plainly_not_as_an_error_crash() {
    let rig = Rig::new("nopub");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
    let absent = rig.root().join("absent.json");
    rig.channel(&format!("file://{}", absent.display()));

    let done = rig.run(&exe, &["update", "check"]);
    let out = done.ok();
    assert!(
        out.contains("no manifest at") && out.contains("absent.json"),
        "the sentence names the channel that has nothing: {out}"
    );
    assert!(
        out.contains("curl exited"),
        "and carries what curl actually said, rather than amx's guess at it: {out}"
    );
    assert!(
        !out.contains("no release pipeline"),
        "that sentence is about the default channel, not about any empty one: {out}"
    );
    assert!(
        done.stderr.is_empty(),
        "an answer is not a diagnostic: {:?}",
        done.stderr
    );

    // The same for `apply`: nothing published is nothing to do.
    let done = rig.run(&exe, &["update", "apply"]);
    let out = done.ok();
    assert!(out.contains("no manifest at"), "{out}");
    assert!(out.contains("nothing was installed"), "{out}");
    assert!(!rig.staging().exists(), "apply staged something");
}

// -------------------------------------------------------------------- apply

#[test]
fn apply_verifies_sha256_and_refuses_a_mismatch_without_installing() {
    let rig = Rig::new("mism");
    let exe = rig.plant("bin/amx");
    let before = fs::metadata(&exe).expect("stat").len();
    let (asset, real) = payload(&rig, "amx: the impostor");

    // A digest that is well formed and wrong: the manifest is not malformed,
    // the bytes simply are not the ones it promised.
    let lie = "0".repeat(64);
    assert_ne!(lie, real, "the fixture must actually disagree");
    rig.channel(&rig.manifest("latest.json", &newer(), Some((&asset, &lie))));

    let done = rig.run(&exe, &["update", "apply"]);
    let err = done.failed();
    assert!(err.contains("checksum mismatch"), "{done:#?}");
    assert!(
        err.contains(&lie) && err.contains(&real),
        "both digests are named, so the reader can tell which end is wrong: {done:#?}"
    );

    assert_eq!(
        fs::metadata(&exe).expect("stat").len(),
        before,
        "the running binary was replaced by an unverified download"
    );
    assert!(
        staged_files(&rig).is_empty(),
        "an unverified download outlived the failure that rejected it"
    );
    assert!(
        siblings(&exe).is_empty(),
        "a half-installed file was left beside the binary"
    );
}

#[tokio::test]
async fn apply_stages_then_renames_atomically_and_the_old_exe_keeps_running() {
    let mut rig = Rig::new("swap");
    let exe = rig.plant("bin/amx");

    // A server started from this very file, so "the old exe keeps running" is a
    // claim about a live process and not a figure of speech.
    let pid = rig.serve(&exe);

    let marker = "amx: the successor";
    let (asset, sha256) = payload(&rig, marker);
    let version = newer();
    rig.channel(&rig.manifest("latest.json", &version, Some((&asset, &sha256))));

    // The payload is a shell script, not an amx, so the handoff that now
    // follows a successful install is *refused* — by the pre-flight, before a
    // single pane is touched (D-M3-6 point 2). That is what makes this test
    // still about the rename: the install lands, nothing swaps, and the
    // process executing the replaced file is exactly where it was.
    let done = rig.run(&exe, &["update", "apply"]);
    assert_eq!(
        done.code,
        Some(1),
        "an install whose successor cannot be handed the session is not a \
         silent success: {done:?}"
    );
    let out = done.stdout.as_str();
    assert!(out.contains(&format!("sha256 {sha256} verified")), "{out}");
    assert!(
        out.contains(&format!("installed {version} at {}", exe.display())),
        "the install itself succeeded and says so on stdout: {out}"
    );
    assert!(
        done.stderr.contains("refused the handoff")
            && done.stderr.contains("did not print handoff capabilities"),
        "the refusal names what was wrong with the successor: {done:?}"
    );

    // The path now holds the new file, and it runs.
    let planted = Command::new(&exe).output().expect("run the new binary");
    assert!(
        String::from_utf8_lossy(&planted.stdout).contains(marker),
        "the path was not replaced by the staged file: {planted:?}"
    );
    assert!(
        siblings(&exe).is_empty(),
        "the staging sibling outlived the rename that consumed it"
    );
    assert!(
        staged_files(&rig).is_empty(),
        "the download outlived the install"
    );

    // And the process that was executing the old file is untouched: same pid,
    // still serving the socket. `rename(2)` moved a directory entry, not an
    // image.
    assert_eq!(
        probe(&rig.socket()).expect("probe"),
        Probe::Running,
        "the running server died when its file was replaced"
    );
    assert_eq!(
        server_pid(&rig.socket()).await.expect("peer"),
        Some(pid),
        "a different process is answering; the old exe did not keep running"
    );
}

#[test]
#[allow(
    clippy::assertions_on_constants,
    reason = "the constant is the subject: this pins the dormant path dormant"
)]
fn apply_hands_the_running_session_to_the_binary_it_installed() {
    // W10's tripwire, discharged. It asserted the constant was `false` and that
    // `apply` printed a sentence saying panes were still on the old binary;
    // both halves went red the day W14 flipped it, which is what the tripwire
    // was for — the sentence had to be *replaced* by an assertion that the
    // successor answered, not merely deleted.
    assert!(
        amx::cmd::update::HANDOFF_AFTER_INSTALL,
        "the handoff glue is dormant again; this test is asserting about a \
         swap that cannot happen"
    );

    let mut rig = Rig::new("hand");
    let exe = rig.plant("bin/amx");
    // The asset is a real amx, because the successor has to be one: it is
    // exec'd as `<binary> _handoff-caps` and then as `<binary> --session <name>
    // server --handoff-import <socket>`, and a shell script answers neither.
    let asset = rig.plant("dist/amx");
    let sha256 = sha256_of(&asset).expect("digest the asset");
    rig.channel(&rig.manifest("latest.json", &newer(), Some((&asset, &sha256))));

    let before = rig.serve(&exe);
    let out = rig.run(&exe, &["update", "apply"]);
    let text = out.ok().to_owned();
    assert!(
        text.contains("handoff accepted"),
        "the install did not ask for the swap: {out:?}"
    );
    assert!(
        text.contains(&format!("session {} is now served by amx", rig.session)),
        "the successor never answered: {out:?}"
    );

    // A different process, serving the same session, on the socket the caller
    // was already talking to. That is the whole claim.
    let after = answering_pid(&rig);
    assert!(
        after.is_some() && after != Some(before),
        "the session is still served by the process that installed the binary \
         (before {before}, after {after:?})"
    );
    let state = rig.run(&exe, &["session", "state"]);
    assert_eq!(
        state.code,
        Some(0),
        "the successor does not answer: {state:?}"
    );

    let stopped = rig.run(&exe, &["session", "stop"]);
    assert_eq!(stopped.code, Some(0), "{stopped:?}");
}

/// The pid of whatever is answering the rig's session socket.
///
/// Peer credentials, which is the only thing that distinguishes a successor
/// from its predecessor when both are built from the same tree.
fn answering_pid(rig: &Rig) -> Option<u32> {
    let socket = rig.socket();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(async move { server_pid(&socket).await.ok().flatten() })
}

// --------------------------------------------------------- package managers

#[test]
fn a_brew_mise_nix_exe_path_redirects_and_writes_nothing() {
    // Nix first, and by path alone: `/nix/store` is the shape, and a test
    // cannot create one outside a machine that has Nix. The classifier is the
    // whole of the detection, so a path is the whole of the fixture.
    assert_eq!(
        pm::classify(Path::new("/nix/store/9k3v0q1x2m-amx-0.1.0/bin/amx")),
        Install::Nix
    );
    assert_eq!(
        pm::classify(Path::new("/usr/local/bin/amx")),
        Install::Standalone,
        "an ordinary install is amx's to replace"
    );

    let rig = Rig::new("pm");
    // A channel with something to install, so the only reason nothing is
    // fetched is the redirect and not an empty manifest.
    let (asset, sha256) = payload(&rig, "amx: never installed");
    rig.channel(&rig.manifest("latest.json", &newer(), Some((&asset, &sha256))));

    for (shape, manager, hint) in [
        (
            format!("Cellar/amx/{CURRENT}/bin/amx"),
            "Homebrew",
            "brew upgrade amx",
        ),
        (
            format!("installs/amx/{CURRENT}/bin/amx"),
            "mise",
            "mise upgrade amx",
        ),
    ] {
        let exe = rig.plant(&shape);
        let before = fs::metadata(&exe).expect("stat").len();

        let done = rig.run(&exe, &["update", "apply"]);
        let err = done.failed();
        assert!(err.contains(manager), "{done:#?}");
        assert!(
            err.contains(hint),
            "a redirect that does not name the command is not a redirect: {done:#?}"
        );
        assert!(err.contains("Nothing was written"), "{done:#?}");

        assert_eq!(
            fs::metadata(&exe).expect("stat").len(),
            before,
            "amx wrote into {manager}'s tree"
        );
        assert!(
            siblings(&exe).is_empty(),
            "amx left a file in {manager}'s tree"
        );
        assert!(
            !rig.staging().exists(),
            "amx downloaded before it checked whose binary this is"
        );
    }

    // And through a symlink, which is how these installs reach `PATH`.
    let link = rig.root().join("linked-amx");
    std::os::unix::fs::symlink(
        rig.root().join(format!("Cellar/amx/{CURRENT}/bin/amx")),
        &link,
    )
    .expect("symlink");
    assert_eq!(pm::classify(&link), Install::Brew);
}

#[test]
fn the_checksum_is_sha_256_and_not_something_shaped_like_it() {
    // The one known-answer test in the suite: FIPS 180-4's digest of "abc".
    // Everything else here compares amx's digest against amx's digest, which
    // would agree just as happily on the wrong algorithm.
    let dir = TempDir::new("sha");
    let path = dir.path().join("abc");
    fs::write(&path, b"abc").expect("write");
    assert_eq!(
        sha256_of(&path).expect("digest"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    let empty = dir.path().join("empty");
    fs::write(&empty, b"").expect("write");
    assert_eq!(
        sha256_of(&empty).expect("digest"),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

// ------------------------------------------------------------------ helpers

/// Whatever is sitting in the staging directory.
fn staged_files(rig: &Rig) -> Vec<PathBuf> {
    entries(&rig.staging())
}

/// The files beside `exe` that are not `exe`.
fn siblings(exe: &Path) -> Vec<PathBuf> {
    entries(exe.parent().expect("a directory"))
        .into_iter()
        .filter(|path| path != exe)
        .collect()
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    match fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(Result::ok).map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    }
}
