//! What the client reads out of `config.toml`: `[client]` and `[keys]`.
//!
//! Until M4 `amx-client` read no configuration at all. 04 §7 promised the
//! keybindings would be configurable and the tree shipped three milestones with
//! a `const PREFIX` and a `match` on byte literals instead
//! (`docs/11-m4-plan.md` D-M4-8). This module is the half of that promise the
//! client owes: it takes the two sections `amx-core` defines and turns them
//! into the [`Bindings`] the input machine runs on, plus the narrow threshold
//! the projection reads.
//!
//! # Where the work is
//!
//! `amx-core::config` already folds a file into a [`Config`] leniently — an
//! unreadable file keeps everything, a section that does not parse keeps that
//! section — so nothing here re-implements any of that. What is left is the
//! step after it: a `[keys]` section that parsed as TOML can still name a key
//! this build cannot read or an action it does not have, and *that* failure has
//! the same rule. One bad row loses one row, files a [`ConfigDiagnostic`], and
//! leaves every other binding standing.
//!
//! The `[keys]` half is two modules — [`name`] for the key grammar and [`keys`]
//! for the table it indexes — and this file is the reading, the `[client]`
//! half, and the join. Everything is re-exported here, so a consumer writes
//! `amx_client::config::Bindings` and does not care which file it came from.

mod keys;
mod name;

use std::io;
use std::path::Path;

use amx_core::config;
use amx_core::{Config, ConfigDiagnostic};

pub use keys::{Binding, Bindings, PrefixAction, SHIPPED_PREFIX, Source, bindings_of};
pub use name::{KeyError, key_byte, key_name};

/// Everything the client reads out of the user's configuration.
///
/// One struct rather than two arguments because the two sections are read at
/// the same moment by the same caller, and a client that took its bindings from
/// a file and its threshold from a default would be reading the file twice.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Settings {
    /// `[client] narrow_cols`, or the shipped threshold.
    ///
    /// Read by the narrow-viewport projection (D14): below this width the
    /// client shows one pane full-screen and declares that in its viewport.
    pub narrow_cols: NarrowCols,
    /// `[client] mouse`: whether to ask the host terminal for mouse reports.
    ///
    /// A bare `bool` and not a newtype, because here the `Default` a bare
    /// `bool` gives — `false` — *is* the shipped answer
    /// (`amx_core::config::DEFAULT_MOUSE`, which carries the measurement that
    /// chose it). Read by `amx attach`, which hands it to the terminal guard.
    pub mouse: bool,
    /// `[keys]`, resolved.
    pub bindings: Bindings,
}

/// The narrow threshold, in columns.
///
/// A newtype so the default is the type's own: a bare `u16` behind
/// `#[derive(Default)]` would make "the settings nobody configured" mean a
/// threshold of zero, which is the one value that silently disables the policy.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NarrowCols(pub u16);

impl Default for NarrowCols {
    fn default() -> Self {
        Self(config::DEFAULT_NARROW_COLS)
    }
}

impl NarrowCols {
    /// Whether a client `cols` wide is below the threshold.
    #[must_use]
    pub const fn is_narrow(self, cols: u16) -> bool {
        cols < self.0
    }
}

/// Read `path` and resolve everything the client takes from it.
///
/// A missing file is not an error and never has been: an absent file says what
/// an empty one says — every section is absent, so every section is its default
/// (D-M1-8). Any other read failure files a whole-file diagnostic and leaves
/// the shipped settings in place, for the reason the server gives at the same
/// junction: a client that refused to attach over a stray character in
/// `config.toml` would be a worse answer than one running the documented
/// defaults.
#[must_use]
pub fn load(path: &Path) -> (Settings, Vec<ConfigDiagnostic>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return (
                Settings::default(),
                vec![ConfigDiagnostic::whole_file(format!(
                    "{}: {err}",
                    path.display()
                ))],
            );
        }
    };
    let (config, mut diagnostics) = config::reload(&Config::default(), &text);
    let (settings, resolved) = resolve(&config);
    diagnostics.extend(resolved);
    (settings, diagnostics)
}

/// Resolve a parsed configuration into what the client runs on.
///
/// Split from [`load`] so the resolution is testable without a filesystem, the
/// same division `amx-core::config` draws between its pure fold and the
/// server's `ConfigRuntime`.
#[must_use]
pub fn resolve(config: &Config) -> (Settings, Vec<ConfigDiagnostic>) {
    let (bindings, diagnostics) = bindings_of(&config.keys);
    let settings = Settings {
        narrow_cols: NarrowCols(config.client.narrow_cols()),
        mouse: config.client.mouse(),
        bindings,
    };
    (settings, diagnostics)
}
