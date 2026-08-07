//! Bytes over the network, by a curl subprocess.
//!
//! herdr's documented choice, and amx's for the same reason: an HTTP client is
//! a TLS stack, a certificate store and a redirect policy, and pulling all three
//! into the tree so that one twice-a-year command can `GET` a small JSON file
//! is a bad trade. `curl` is present on every platform amx targets, and it is
//! invoked the way every other subprocess in this tree is — **argv as data**,
//! never a shell string, so a URL cannot become a command.
//!
//! # The flags, and why each one
//!
//! Verified against `curl --help all` (8.21.0) rather than remembered:
//!
//! - `-s` silent, `-S` show errors anyway. herdr uses `-s` alone; amx adds `-S`
//!   because the alternative to curl's own diagnosis is amx inventing one, and
//!   an invented mapping from exit code to English is exactly the guess
//!   `HACKING.md` forbids. What the user reads is what curl said.
//! - `-f` fail on an HTTP error rather than saving the error page as if it were
//!   the manifest.
//! - `-L` follow redirects, which the default channel *requires*: a GitHub
//!   release download is a redirect to object storage.
//! - `--retry 3` for transient failures. It does not turn a 404 into three
//!   404s: curl retries transient errors, and a missing file is not one.
//! - `--connect-timeout` and `--max-time`, so a hung network is a bounded wait
//!   and not a hung terminal.
//! - `--max-filesize` on the asset download: a channel amx does not control
//!   must not be able to fill the disk before the checksum gets a say.
//!
//! # Schemes
//!
//! `https` and `file` only, checked here rather than left to curl's `--proto`,
//! so the refusal names amx's rule instead of printing a curl error. Plain
//! `http` is refused deliberately: the manifest is what carries the checksums
//! that make the download trustworthy, so fetching it in cleartext would make
//! the checksums decorative. `file` is what the tests — and an air-gapped
//! mirror — use.

use std::fmt;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, bail};
use tokio::process::Command;

/// How many times curl retries a transient failure.
pub const RETRIES: &str = "3";

/// Seconds curl may spend establishing a connection.
pub const CONNECT_TIMEOUT: &str = "10";

/// Seconds a manifest fetch may take in total.
pub const MANIFEST_MAX_TIME: &str = "20";

/// Seconds an asset download may take in total.
///
/// Wider than the manifest's by a factor of fifteen because it is a binary on
/// someone's hotel wifi, not a kilobyte of JSON.
pub const ASSET_MAX_TIME: &str = "300";

/// The largest asset amx will write to disk, in bytes (256 MiB).
pub const MAX_ASSET_BYTES: &str = "268435456";

/// What a manifest fetch found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fetched {
    /// The channel answered, and these are its bytes.
    Body(Vec<u8>),
    /// Nothing is published there.
    Missing(Missing),
}

/// A channel that did not answer with a manifest.
///
/// Not an error type: for `check` this is the *expected* outcome today
/// (R-M3-4), and the caller decides whether it is worth an exit code. What it
/// carries is enough to tell "nothing published" from "no network" without
/// amx claiming to know which: curl's exit status and curl's own sentence.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Missing {
    /// The URL that was asked.
    pub url: String,
    /// curl's exit status, or `None` if a signal ended it.
    pub status: Option<i32>,
    /// What curl printed on stderr, trimmed. Empty if it said nothing.
    pub said: String,
}

impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no manifest at {}", self.url)?;
        match self.status {
            Some(code) => write!(f, " (curl exited {code}")?,
            None => write!(f, " (curl was signalled")?,
        }
        if self.said.is_empty() {
            f.write_str(")")
        } else {
            write!(f, ": {})", self.said)
        }
    }
}

/// Refuse a URL amx will not fetch.
///
/// # Errors
///
/// If the scheme is anything but `https` or `file`.
pub fn check_scheme(url: &str) -> anyhow::Result<()> {
    if url.starts_with("https://") || url.starts_with("file://") {
        return Ok(());
    }
    if url.starts_with("http://") {
        bail!(
            "{url} is cleartext http; a manifest carries the checksums that make \
             a download trustworthy, so amx reads it over https or file only"
        );
    }
    bail!("{url} is not an https:// or file:// URL");
}

/// Fetch the channel manifest at `url`.
///
/// # Errors
///
/// If the scheme is refused, or curl cannot be run at all. A curl that runs and
/// fails is [`Fetched::Missing`], not an error: see [`Missing`].
pub async fn manifest(url: &str) -> anyhow::Result<Fetched> {
    check_scheme(url)?;
    let output = curl()
        .args(["--max-time", MANIFEST_MAX_TIME])
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("run curl to fetch {url}"))?;

    if output.status.success() {
        return Ok(Fetched::Body(output.stdout));
    }
    Ok(Fetched::Missing(Missing {
        url: url.to_owned(),
        status: output.status.code(),
        said: said(&output.stderr),
    }))
}

/// Download the asset at `url` to `dest`.
///
/// Unlike [`manifest`], a failure here is an error: a manifest named this URL,
/// so its absence is a broken channel rather than an empty one.
///
/// # Errors
///
/// If the scheme is refused, curl cannot be run, or the download fails.
pub async fn asset(url: &str, dest: &Path) -> anyhow::Result<()> {
    check_scheme(url)?;
    let output = curl()
        .args(["--max-time", ASSET_MAX_TIME])
        .args(["--max-filesize", MAX_ASSET_BYTES])
        .arg("-o")
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("run curl to download {url}"))?;

    if output.status.success() {
        return Ok(());
    }
    // The partial file is curl's leftover, and leaving it would let a later
    // run's checksum failure look like a corrupt release rather than a
    // truncated download.
    let _ = std::fs::remove_file(dest);
    let said = said(&output.stderr);
    match output.status.code() {
        Some(code) if said.is_empty() => bail!("downloading {url} failed: curl exited {code}"),
        Some(code) => bail!("downloading {url} failed (curl exited {code}): {said}"),
        None => bail!("downloading {url} failed: curl was signalled"),
    }
}

/// The flags every fetch shares.
fn curl() -> Command {
    let mut command = Command::new("curl");
    command
        .arg("-sSfL")
        .args(["--retry", RETRIES])
        .args(["--connect-timeout", CONNECT_TIMEOUT]);
    command
}

/// curl's stderr as one line, for a message a human reads.
fn said(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}
