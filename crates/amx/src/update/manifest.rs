//! What a channel publishes, and how this build compares to it.
//!
//! The format is one JSON object, small enough to write by hand and to read
//! without a tool:
//!
//! ```json
//! {
//!   "version": "0.2.0",
//!   "notes": "One line about the release.",
//!   "assets": {
//!     "linux-x86_64":  { "url": "https://…/amx-linux-x86_64",  "sha256": "…" },
//!     "macos-aarch64": { "url": "https://…/amx-macos-aarch64", "sha256": "…" }
//!   }
//! }
//! ```
//!
//! `notes` is optional; a version and one usable asset are not. The asset key
//! is [`platform`] — `<os>-<arch>` from the constants the compiler already
//! knows, so a build cannot look for an asset it was not built to run.
//!
//! Parsing is by hand over `serde_json::Value` rather than by derive. The
//! reason is the one the crate already states in its `Cargo.toml`: serde's
//! derive is not a dependency here, and a manifest is small enough that
//! walking it yields the better error anyway — a derive says "missing field
//! `sha256`" and this says which asset the field was missing from.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context as _, bail};
use serde_json::Value;

/// Where `amx update` looks when the config names nothing else.
///
/// The releases of the repository this binary was built from. **No release has
/// published this asset yet** and no pipeline exists to publish one (R-M3-4);
/// the URL is where the manifest goes, not a promise that it is there. Nothing
/// in this crate treats a 404 here as an error, precisely because the expected
/// answer today is a 404.
pub const DEFAULT_CHANNEL: &str =
    "https://github.com/saifulapm/amx/releases/latest/download/latest.json";

/// The version of amx this binary is.
#[must_use]
pub fn current() -> Version {
    // The invariant: `CARGO_PKG_VERSION` is the workspace's own version field,
    // which cargo has already parsed as semver to build this crate at all.
    #[allow(
        clippy::expect_used,
        reason = "cargo rejected the manifest before this could fail"
    )]
    Version::parse(env!("CARGO_PKG_VERSION")).expect("cargo parsed this version to build the crate")
}

/// The asset key this build looks for: `<os>-<arch>`.
///
/// `linux-x86_64`, `macos-aarch64`, and so on, from `std::env::consts` — which
/// are compile-time constants about the target, not process environment, so
/// this is not the `std::env::var` the crate's own rule forbids below `run`.
#[must_use]
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// A `major.minor.patch` version.
///
/// Strictly three decimal numbers, and a leading `v` is accepted because
/// release tags carry one. Anything else — a `-rc1` suffix, a build tag, four
/// components — is refused with the text that was not understood, rather than
/// silently compared as if the suffix were not there. There is no preview
/// channel to publish a prerelease into (R-M3-4), so the strict reading costs
/// nothing today and cannot quietly order `0.2.0-rc1` above `0.2.0` tomorrow.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version {
    /// Breaking.
    pub major: u64,
    /// Feature.
    pub minor: u64,
    /// Fix.
    pub patch: u64,
}

impl Version {
    /// Parse `text` as `major.minor.patch`.
    ///
    /// # Errors
    ///
    /// If it is not exactly three decimal components.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let trimmed = text.trim();
        let body = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let mut parts = body.split('.');
        let mut next = |what: &str| -> anyhow::Result<u64> {
            let part = parts
                .next()
                .with_context(|| format!("{text:?} has no {what} component"))?;
            part.parse::<u64>()
                .with_context(|| format!("{text:?} has a {what} component that is not a number"))
        };
        let major = next("major")?;
        let minor = next("minor")?;
        let patch = next("patch")?;
        if parts.next().is_some() {
            bail!("{text:?} is not a major.minor.patch version");
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// How the channel's version stands against the running one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Standing {
    /// The channel has something to install.
    Newer,
    /// The channel is this build.
    Same,
    /// The channel is behind — a rollback, or a mirror that has not caught up.
    /// Never installed without being asked for; `apply` reports and stops.
    Older,
}

/// One platform's download.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Asset {
    /// Where the binary is.
    pub url: String,
    /// Its SHA-256, lowercase hex, 64 characters.
    pub sha256: String,
}

/// A channel manifest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Manifest {
    /// The version the channel is offering.
    pub version: Version,
    /// What changed, for a human. Optional: a channel that publishes no notes
    /// is thin, not broken.
    pub notes: Option<String>,
    /// One asset per `<os>-<arch>` key.
    pub assets: BTreeMap<String, Asset>,
}

impl Manifest {
    /// Parse `bytes` as a channel manifest.
    ///
    /// # Errors
    ///
    /// If the bytes are not JSON, not an object, carry no usable `version`, or
    /// carry an `assets` entry that is not a `{url, sha256}` pair.
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let value: Value = serde_json::from_slice(bytes).context("the manifest is not JSON")?;
        let object = value
            .as_object()
            .context("the manifest is not a JSON object")?;

        let version = Version::parse(
            object
                .get("version")
                .and_then(Value::as_str)
                .context("the manifest has no `version` string")?,
        )
        .context("the manifest's `version`")?;

        let notes = object
            .get("notes")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|notes| !notes.is_empty())
            .map(str::to_owned);

        let mut assets = BTreeMap::new();
        if let Some(table) = object.get("assets") {
            let table = table
                .as_object()
                .context("the manifest's `assets` is not an object")?;
            for (key, entry) in table {
                assets.insert(key.clone(), asset(key, entry)?);
            }
        }

        Ok(Self {
            version,
            notes,
            assets,
        })
    }

    /// How this manifest stands against `current`.
    #[must_use]
    pub fn standing(&self, current: Version) -> Standing {
        match self.version.cmp(&current) {
            std::cmp::Ordering::Greater => Standing::Newer,
            std::cmp::Ordering::Equal => Standing::Same,
            std::cmp::Ordering::Less => Standing::Older,
        }
    }

    /// The asset for `key`, or the reason there is none.
    ///
    /// The error names the keys that *are* published, because "no asset for
    /// linux-x86_64" and "this channel publishes only macos-aarch64" are the
    /// same fact and only the second one tells the reader what happened.
    ///
    /// # Errors
    ///
    /// If the manifest carries no asset under `key`.
    pub fn asset(&self, key: &str) -> anyhow::Result<&Asset> {
        self.assets.get(key).with_context(|| {
            if self.assets.is_empty() {
                format!("the manifest publishes no assets at all, so none for {key}")
            } else {
                format!(
                    "the manifest publishes no asset for {key}; it has {}",
                    self.assets
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        })
    }
}

/// One `assets` entry: `{url, sha256}`, both non-empty, the digest well formed.
fn asset(key: &str, value: &Value) -> anyhow::Result<Asset> {
    let object = value
        .as_object()
        .with_context(|| format!("asset {key} is not an object"))?;
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .with_context(|| format!("asset {key} has no `url`"))?
        .to_owned();
    let sha256 = object
        .get("sha256")
        .and_then(Value::as_str)
        .map(str::trim)
        .with_context(|| format!("asset {key} has no `sha256`"))?
        .to_ascii_lowercase();
    // Refused here rather than at comparison time: a manifest that cannot state
    // a digest must not get as far as a download, because the only way to
    // "handle" a malformed digest after the bytes are on disk is to install
    // unverified or to throw the download away.
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("asset {key} has a `sha256` that is not 64 hex characters");
    }
    Ok(Asset { url, sha256 })
}
