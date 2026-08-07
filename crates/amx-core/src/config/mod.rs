//! The user's configuration: types and parsing, no I/O.
//!
//! `config.toml` is per user and shared by every session ([`Ctx::config_path`],
//! D-M1-4). Reading it and watching it for changes are the server's job; this
//! module is pure functions over `&str`, like the rest of amx-core, so the
//! client can read the same types when client-side settings land.
//!
//! # Reload semantics (D-M1-8)
//!
//! Reload is two-tier and lenient, and the leniency is in the signature rather
//! than in a caller convention: [`reload`] takes the *running* config and
//! returns the next one, so "keep what was running" is what a failure at either
//! tier structurally does.
//!
//! - A whole-file failure — unreadable TOML, a top level that is not a table —
//!   keeps the entire running config. A typo while editing must never yank live
//!   settings out from under a session.
//! - A section that fails to deserialize keeps that section's running values;
//!   every valid section still applies.
//! - A section absent from the file resets to its defaults, so the running
//!   config is always reconstructable from the file.
//! - Unknown keys and unknown sections are ignored silently — the same
//!   tolerance rule the wire applies in both directions (04 §4).

use serde::{Deserialize, Serialize};

/// The `[persist]` section's name, as it appears in the file.
pub const PERSIST_SECTION: &str = "persist";

/// The `[terminal]` section's name, as it appears in the file.
pub const TERMINAL_SECTION: &str = "terminal";

/// Every section amx reads, in file order.
///
/// M1 tables exactly the sections that have a consumer; a section nobody reads
/// is a promise nobody keeps. Walked rather than repeated, so adding a section
/// is adding a field plus a row here.
pub const SECTIONS: &[&str] = &[PERSIST_SECTION, TERMINAL_SECTION];

/// The whole configuration, one field per section.
///
/// `Default` is the configuration of a machine with no `config.toml` at all,
/// which is why a missing file is never an error: it is this value.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `[persist]`.
    #[serde(default)]
    pub persist: PersistConfig,
    /// `[terminal]`.
    #[serde(default)]
    pub terminal: TerminalConfig,
}

/// `[persist]`: what durability keeps beyond the snapshot itself.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PersistConfig {
    /// Whether per-pane scrollback sidecars are written (D-M1-6).
    ///
    /// Off by default because scrollback holds secrets (04 §6). The toggle is
    /// hot in both directions: turning it on dumps at the next save, turning it
    /// off wipes `history/` immediately rather than at some later save.
    #[serde(default)]
    pub history: bool,
}

/// `[terminal]`: what a new pane's process is.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// The shell new panes spawn, ahead of `$SHELL` and `/bin/sh`.
    ///
    /// Consulted per spawn, so an edit reaches the next pane without a restart;
    /// panes already running keep the process they started with, because
    /// nothing about a config reload may restart a live process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
}

/// One thing that went wrong while reloading, named by section.
///
/// Typed rather than a formatted string assembled at the call site: the reload
/// event carries a count of these, and `session report`-shaped surfaces may
/// want the parts separately.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConfigDiagnostic {
    /// The section that was rejected, or `None` for a whole-file failure.
    pub section: Option<&'static str>,
    /// What the parser said.
    pub message: String,
}

impl ConfigDiagnostic {
    /// A whole-file failure: nothing in the file applied.
    #[must_use]
    pub fn whole_file(message: impl Into<String>) -> Self {
        Self {
            section: None,
            message: message.into(),
        }
    }

    /// One section failed to deserialize; its running values were kept.
    #[must_use]
    pub fn section(section: &'static str, message: impl Into<String>) -> Self {
        Self {
            section: Some(section),
            message: message.into(),
        }
    }
}

/// Fold `text` into `current`, per the semantics in this module's docs.
///
/// Returns the configuration to run from here on plus one diagnostic per thing
/// that was rejected. An empty diagnostic list means the file applied whole.
///
/// # Task ownership
///
/// U01 (`docs/07-m1-plan.md` §4) froze this signature and the whole-file tier;
/// **U03 implements the per-section tier**. Until it does, a file that parses
/// as TOML leaves the running config untouched — which is not D-M1-8's
/// behaviour, only its first half.
#[must_use]
pub fn reload(current: &Config, text: &str) -> (Config, Vec<ConfigDiagnostic>) {
    let document = match text.parse::<toml::Table>() {
        Ok(document) => document,
        // Tier one: the file is not a TOML table. Keep everything, say why.
        Err(err) => {
            return (
                current.clone(),
                vec![ConfigDiagnostic::whole_file(err.to_string())],
            );
        }
    };
    let _ = document;
    (current.clone(), Vec::new())
}
