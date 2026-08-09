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
//! | mise | `…/installs/amx/<version>/bin/amx`, or the same under
//!   `$MISE_INSTALLS_DIR` |
//! | Nix | anything under `/nix/store` |
//!
//! # mise's relocated root
//!
//! mise lets the installs root be moved, and a moved root is the one case a
//! path shape cannot see: `/opt/tools/amx/0.1.0/bin/amx` is mise's file and
//! looks like nobody's. So [`classify`] takes the root as a parameter — read
//! from `$MISE_INSTALLS_DIR` into [`amx_core::Env`] at process start, like every
//! other piece of environment this crate uses, and threaded here as a value.
//!
//! Only a root moved *out of a directory named `installs`* needs it. mise's
//! other spelling of the same move, `$MISE_DATA_DIR`, puts its installs at
//! `<data>/installs`, and the shape rule already matches a directory named
//! `installs` wherever it sits.
//!
//! # One bound, stated rather than papered over
//!
//! **A binary copied out of a manager's tree** is a standalone install by this
//! test, and correctly so: nothing owns it any more.

use std::path::Path;

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

/// Classify the binary at `exe`, with mise's installs root as `mise_installs`
/// says it is ([`amx_core::Env::mise_installs_dir`], `None` when the variable is
/// unset).
///
/// Checks the path as given and, if that says nothing, the path with every
/// symlink resolved — and the configured root the same way, since a resolved
/// `exe` cannot sit under an unresolved root.
#[must_use]
pub fn classify(exe: &Path, mise_installs: Option<&Path>) -> Install {
    let mise = Mise::rooted_at(mise_installs);
    let direct = shape(exe, &mise);
    if direct != Install::Standalone {
        return direct;
    }
    match exe.canonicalize() {
        Ok(real) => shape(&real, &mise),
        Err(_) => Install::Standalone,
    }
}

/// Where mise installs, as configured: the root as given and as resolved.
///
/// Both, because [`classify`] tries `exe` twice. A root reached through a
/// symlink — `~/tools` pointing into another filesystem is the ordinary case —
/// matches the literal `exe` under its own spelling and the canonicalised `exe`
/// under the resolved one, and neither spelling answers for the other.
struct Mise {
    roots: Vec<std::path::PathBuf>,
}

impl Mise {
    /// The configured root, if there is one.
    fn rooted_at(configured: Option<&Path>) -> Self {
        let mut roots = Vec::new();
        if let Some(root) = configured {
            roots.push(root.to_path_buf());
            if let Ok(real) = root.canonicalize()
                && real != root
            {
                roots.push(real);
            }
        }
        Self { roots }
    }

    /// Whether `dir` is that root.
    fn holds(&self, dir: &Path) -> bool {
        self.roots.iter().any(|root| root == dir)
    }
}

/// Classify one literal path, following nothing.
fn shape(exe: &Path, mise: &Mise) -> Install {
    if exe.starts_with("/nix/store") {
        return Install::Nix;
    }
    let Some(root) = keg_root(exe) else {
        return Install::Standalone;
    };
    if named(root, "Cellar") {
        return Install::Brew;
    }
    if named(root, "installs") || mise.holds(root) {
        return Install::Mise;
    }
    Install::Standalone
}

/// The directory above a `<root>/amx/<version>/bin/amx` path.
///
/// Homebrew and mise lay their trees out identically apart from the root — one
/// walk answers for both, and the root is compared afterwards, by name for the
/// two default layouts and by path for a relocated one.
fn keg_root(exe: &Path) -> Option<&Path> {
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
    tool.parent()
}

/// Whether `dir`'s own name is `name`, wherever `dir` sits.
fn named(dir: &Path, name: &str) -> bool {
    dir.file_name().is_some_and(|actual| actual == name)
}
