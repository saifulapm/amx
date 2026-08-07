//! Who installed this amx, read off the path it is running from.
//!
//! The rule is **redirect, never write**. A binary under a package manager's
//! tree is that manager's file: it is recorded in a receipt, it is compared
//! against a checksum on the manager's next run, and replacing it behind the
//! manager's back produces an installation that lies about what it contains.
//! So when amx recognises one of those trees it prints the manager's own
//! upgrade command and touches nothing — herdr's behavior, kept whole, because
//! the reasoning is the same for any tool that can be installed two ways.
//!
//! Detection is by **path shape**, and by the canonicalised path as well, since
//! the amx on a user's `PATH` is routinely a symlink (`~/.local/bin/amx`,
//! `/opt/homebrew/bin/amx`, `~/.nix-profile/bin/amx`) into the tree that owns
//! the real file. The shapes, from each manager's own layout:
//!
//! | Manager | Shape |
//! |---|---|
//! | Homebrew | `…/Cellar/amx/<version>/bin/amx` |
//! | mise | `…/installs/amx/<version>/bin/amx` |
//! | Nix | anything under `/nix/store` |
//!
//! # Two bounds, stated rather than papered over
//!
//! - **mise's `$MISE_INSTALLS_DIR`.** mise lets the installs root be moved, and
//!   herdr reads that variable to catch the moved case. amx does not read it:
//!   this crate's rule is that process environment is read once, in
//!   [`crate::run`], and threaded as a value ([`amx_core::Env`]), and no `Env`
//!   field carries it. A user who has moved mise's installs root out of a
//!   directory named `installs` is therefore seen as a standalone install. The
//!   fix is an `Env` field, which is amx-core's file and not this task's.
//! - **A binary copied out of a manager's tree** is a standalone install by
//!   this test, and correctly so: nothing owns it any more.

use std::path::{Path, PathBuf};

/// The file name amx is installed under.
const EXE: &str = "amx";

/// How this amx got onto the machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Install {
    /// Nobody's but the user's: amx may replace it.
    Standalone,
    /// Homebrew's.
    Brew,
    /// mise's.
    Mise,
    /// Nix's.
    Nix,
}

impl Install {
    /// The manager's own name, for a sentence a human reads.
    #[must_use]
    pub fn manager(self) -> Option<&'static str> {
        match self {
            Self::Standalone => None,
            Self::Brew => Some("Homebrew"),
            Self::Mise => Some("mise"),
            Self::Nix => Some("Nix"),
        }
    }

    /// What to run instead of `amx update apply`.
    ///
    /// Nix has no single command — a Nix install is upgraded by whatever
    /// declares it, a flake input or a channel — so it gets a sentence rather
    /// than a fabricated one-liner.
    #[must_use]
    pub fn upgrade_hint(self) -> Option<&'static str> {
        match self {
            Self::Standalone => None,
            Self::Brew => Some("run `brew upgrade amx`"),
            Self::Mise => Some("run `mise upgrade amx`"),
            Self::Nix => Some("upgrade it the way it was declared — a flake input, or a channel"),
        }
    }

    /// Whether amx may write over this binary.
    #[must_use]
    pub fn is_managed(self) -> bool {
        self != Self::Standalone
    }
}

/// Classify the binary at `exe`.
///
/// Checks the path as given and, if that says nothing, the path with every
/// symlink resolved.
#[must_use]
pub fn classify(exe: &Path) -> Install {
    let direct = shape(exe);
    if direct != Install::Standalone {
        return direct;
    }
    match exe.canonicalize() {
        Ok(real) => shape(&real),
        Err(_) => Install::Standalone,
    }
}

/// Classify one literal path, following nothing.
fn shape(exe: &Path) -> Install {
    if exe.starts_with("/nix/store") {
        return Install::Nix;
    }
    if keg(exe, "Cellar").is_some() {
        return Install::Brew;
    }
    if keg(exe, "installs").is_some() {
        return Install::Mise;
    }
    Install::Standalone
}

/// The version directory of a `<root>/amx/<version>/bin/amx` path.
///
/// Homebrew and mise lay their trees out identically apart from the name of the
/// root — `Cellar` and `installs` — so one walk answers for both, and a shape
/// that matches everything but the root name matches neither.
fn keg(exe: &Path, root: &str) -> Option<PathBuf> {
    if exe.file_name()? != EXE {
        return None;
    }
    let bin = exe.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let version = bin.parent()?;
    let tool = version.parent()?;
    if tool.file_name()? != EXE {
        return None;
    }
    if tool.parent()?.file_name()? != root {
        return None;
    }
    Some(version.to_path_buf())
}
