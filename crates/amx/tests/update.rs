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
use std::process::{Child, Command, Stdio};

use amx::update::pm::{self, Install};
use amx::update::verify::sha256_of;
use amx_server::session::probe::{Probe, probe, server_pid};

mod support;

use support::{TempDir, assert_sun_path_fits, wait_until};

/// The version this build is, which every manifest here is written against.
const CURRENT: &str = env!("CARGO_PKG_VERSION");

// ------------------------------------------------------------------ the rig

/// A machine with its own roots, and somewhere to put binaries.
struct Rig {
    dir: TempDir,
    session: String,
    server: Option<Child>,
}

impl Rig {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        assert_sun_path_fits(dir.path(), tag);
        for sub in ["run", "state", "config/amx"] {
            fs::create_dir_all(dir.path().join(sub)).expect("create a root");
        }
        Self {
            dir,
            session: tag.to_owned(),
            server: None,
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// This rig's session socket, once something binds it.
    fn socket(&self) -> PathBuf {
        self.root()
            .join("run")
            .join("amx")
            .join(&self.session)
            .join("sock")
    }

    /// Where `amx update apply` stages a download.
    fn staging(&self) -> PathBuf {
        self.root()
            .join("state")
            .join("amx")
            .join(&self.session)
            .join("update")
    }

    /// Put a working copy of the binary under test at `rel`.
    ///
    /// A copy and never a symlink: `current_exe()` resolves symlinks, so a
    /// symlinked fixture would report the path in `target/debug` and every
    /// package-manager shape would evaporate.
    fn plant(&self, rel: &str) -> PathBuf {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().expect("a directory")).expect("create the bin directory");
        let source = PathBuf::from(env!("CARGO_BIN_EXE_amx"));
        // Same-filesystem is the cheap case and worth taking when it is there.
        if fs::hard_link(&source, &path).is_err() {
            fs::copy(&source, &path).expect("copy the binary under test");
        }
        path
    }

    /// Write the config file, pointing the channel at `url`.
    fn channel(&self, url: &str) {
        fs::write(
            self.root().join("config/amx/config.toml"),
            format!("[update]\nchannel = \"{url}\"\n"),
        )
        .expect("write config.toml");
    }

    /// Write a manifest offering `version` of `asset`, and return its URL.
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

    /// Run `exe` with this rig's roots.
    fn run(&self, exe: &Path, args: &[&str]) -> Done {
        let out = self
            .command(exe)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run amx");
        Done {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn command(&self, exe: &Path) -> Command {
        let mut command = Command::new(exe);
        command
            .env("XDG_RUNTIME_DIR", self.root().join("run"))
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("HOME", self.root())
            .env("AMX_SESSION", &self.session);
        command
    }

    /// Start a server from `exe` and wait until it answers.
    fn serve(&mut self, exe: &Path) -> u32 {
        let child = self
            .command(exe)
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the server");
        let pid = child.id();
        self.server = Some(child);
        wait_until("the server answers its socket", || {
            probe(&self.socket()).is_ok_and(|state| state == Probe::Running)
        });
        pid
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

/// What a finished invocation did.
#[derive(Debug)]
struct Done {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Done {
    fn ok(&self) -> &str {
        assert_eq!(self.code, Some(0), "amx failed: {self:#?}");
        &self.stdout
    }

    fn failed(&self) -> &str {
        assert_ne!(self.code, Some(0), "amx succeeded: {self:#?}");
        &self.stderr
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

    let done = rig.run(&exe, &["update", "apply"]);
    let out = done.ok();
    assert!(out.contains(&format!("sha256 {sha256} verified")), "{out}");
    assert!(
        out.contains(&format!("installed {version} at {}", exe.display())),
        "{out}"
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
fn apply_says_plainly_that_the_running_session_is_not_handed_over_yet() {
    // The tripwire for W14: when `HANDOFF_AFTER_INSTALL` flips, this sentence
    // stops being true and this test says so rather than letting a stale
    // promise ship. Replace it then with the assertion that the successor
    // answered.
    assert!(
        !amx::cmd::update::HANDOFF_AFTER_INSTALL,
        "the handoff glue is live: W06 has landed, so this test owes W14 its \
         other half"
    );

    let rig = Rig::new("dorm");
    let exe = rig.plant("bin/amx");
    let (asset, sha256) = payload(&rig, "amx: the successor");
    rig.channel(&rig.manifest("latest.json", &newer(), Some((&asset, &sha256))));

    let out = rig.run(&exe, &["update", "apply"]).ok().to_owned();
    assert!(
        out.contains("the running session was not handed over"),
        "a user with a new binary and old panes must be told: {out}"
    );
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
