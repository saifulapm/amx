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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The `[persist]` section's name, as it appears in the file.
pub const PERSIST_SECTION: &str = "persist";

/// The `[terminal]` section's name, as it appears in the file.
pub const TERMINAL_SECTION: &str = "terminal";

/// The `[update]` section's name, as it appears in the file.
pub const UPDATE_SECTION: &str = "update";

/// The `[work]` section's name, as it appears in the file.
pub const WORK_SECTION: &str = "work";

/// The `[client]` section's name, as it appears in the file.
pub const CLIENT_SECTION: &str = "client";

/// The `[keys]` section's name, as it appears in the file.
pub const KEYS_SECTION: &str = "keys";

/// Every section amx reads, in file order.
///
/// M1 tables exactly the sections that have a consumer; a section nobody reads
/// is a promise nobody keeps. Walked rather than repeated, so adding a section
/// is adding a field plus a row here.
///
/// M4 adds the first two sections the *client* reads rather than the server
/// (`docs/11-m4-plan.md` D-M4-8). They keep the rule above: `[client]` is read
/// by the narrow-viewport projection and `[keys]` by the input machine and
/// `amx keys`, all of them inside the same milestone that freezes the section
/// (D-M4-10).
pub const SECTIONS: &[&str] = &[
    PERSIST_SECTION,
    TERMINAL_SECTION,
    UPDATE_SECTION,
    WORK_SECTION,
    CLIENT_SECTION,
    KEYS_SECTION,
];

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
    /// `[update]`.
    #[serde(default)]
    pub update: UpdateConfig,
    /// `[work]`.
    #[serde(default)]
    pub work: WorkConfig,
    /// `[client]`.
    #[serde(default)]
    pub client: ClientConfig,
    /// `[keys]`.
    #[serde(default)]
    pub keys: KeysConfig,
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

/// `[update]`: where `amx update` looks for a newer amx (D-M3-8).
///
/// One field, and it is an override rather than a value: the default channel is
/// a fact about the binary — the releases of the repository it was built from —
/// so it lives beside the code that fetches it and not in a file every user
/// would have to write out to get the ordinary behavior. What belongs here is
/// the *other* answer: a private mirror, an air-gapped path, a `file://` URL a
/// test points at.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// The channel manifest `amx update` reads, ahead of the built-in default.
    ///
    /// Consulted per invocation, like every other setting here: a session that
    /// has been running for a week reads whatever the file says today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

/// `[work]`: where `amx work <branch>` puts the worktree it adds (D-M3-10).
///
/// One field, and an override rather than a value, on the same reasoning
/// [`UpdateConfig`] gives: the default template is a fact about the design — a
/// *sibling* of the repository, so repo-internal tooling never walks into a
/// nested checkout — and a user who is happy with it should not have to write
/// it out to get it.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WorkConfig {
    /// The path template a new worktree is placed at, ahead of the built-in
    /// default.
    ///
    /// Read per invocation, like every other setting here, and never consulted
    /// again afterwards: the path a workspace's worktree actually sits at is
    /// remembered on the workspace itself, so editing this template does not
    /// move a tree that already exists. What it does change is where the *next*
    /// `amx work` puts one — and `amx work done` recomputes the template to
    /// check the tree it is about to remove is the one this template would have
    /// derived, which is the pin that keeps a destructive verb off a
    /// user-supplied path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// `[client]`: how a client draws what it was given (D14).
///
/// The first section the *client* reads rather than the server, and the reason
/// it exists is `docs/11-m4-plan.md` D-M4-8: 10 §D14 says the phone profile is
/// "documentation work, not code", and every clause of that is false in the
/// tree — there was no `[client]` section, no `[keys]` section, and `amx-client`
/// read no configuration at all. This is the minimum that sentence needs to
/// become true.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ClientConfig {
    /// The width below which a client shows one pane full-screen.
    ///
    /// An override rather than a value, on the same reasoning [`UpdateConfig`]
    /// gives: [`DEFAULT_NARROW_COLS`] is a fact about the design, not something
    /// a user should have to write out to get the ordinary behavior. Read it
    /// through [`narrow_cols`](Self::narrow_cols) so the fallback lives in one
    /// place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrow_cols: Option<u16>,
    /// Whether the client asks its host terminal for mouse reporting (D14).
    ///
    /// An override like the threshold above, and [`DEFAULT_MOUSE`] is where the
    /// default and its reason live. Read it through
    /// [`mouse`](Self::mouse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<bool>,
}

/// The default [`ClientConfig::narrow_cols`], from 10 §D14.
pub const DEFAULT_NARROW_COLS: u16 = 60;

/// The default [`ClientConfig::mouse`]: **off**, from the M4 spike's outcome
/// (b) (`docs/notes/m4-mouse-path.md` §5).
///
/// Asking for mouse reporting costs the user their own terminal's selection,
/// measured rather than assumed: foot's `selection-override-modifiers` "are
/// used to enable selecting text with the mouse irrespective of whether a
/// client application currently has the mouse grabbed … Default: Shift", and
/// alacritty documents the same `Shift` suppression. So with tracking on, every
/// ordinary drag-select in the user's terminal becomes shift-drag.
///
/// And the cost is paid all the time rather than where it buys something: the
/// wheel exception exists for panes that did *not* ask for the mouse, so the
/// request cannot be scoped to the panes that did — which is exactly what a
/// multiplexer mirroring a pane's own request can do and this cannot (X01 §2.3
/// watched tmux hold no tracking at all until a pane asked). What it buys is
/// smaller than 10 §D14 first assumed, too: mode `1007` is set by default in
/// both terminals the spike measured, so the wheel already produces
/// cursor-up/down keys today and the exception buys an *unambiguous* wheel
/// rather than a working one.
///
/// The phone profile is where it is turned on, which is the right place: the
/// people who need touch-scroll are the people not selecting text with a mouse.
pub const DEFAULT_MOUSE: bool = false;

impl ClientConfig {
    /// The narrow threshold this configuration asks for.
    #[must_use]
    pub const fn narrow_cols(&self) -> u16 {
        match self.narrow_cols {
            Some(cols) => cols,
            None => DEFAULT_NARROW_COLS,
        }
    }

    /// Whether this configuration asks for mouse reporting.
    #[must_use]
    pub const fn mouse(&self) -> bool {
        match self.mouse {
            Some(on) => on,
            None => DEFAULT_MOUSE,
        }
    }
}

/// `[keys]`: the prefix key and the prefix table, as data (D-M4-8).
///
/// 04 §7 promised keybindings would be configurable and `amx-client` shipped
/// three milestones with a `const PREFIX` and a `match` on byte literals
/// instead. This is the shape that promise is kept in: a prefix, and a table
/// mapping the key pressed after it to the action it runs.
///
/// Both halves are overrides. An absent `prefix` is the shipped `ctrl+a`, and a
/// `bind` entry replaces exactly one row of the shipped table and leaves the
/// rest — so a phone profile that rebinds the prefix and nothing else is two
/// lines, and a malformed section keeps every shipped binding under the lenient
/// per-section rule this module opens with.
///
/// Key names are spelled the way `pane.send_keys` spells them — `ctrl+a`,
/// `esc`, or a bare character — so a user who has read one part of the docs can
/// write the other. Not every name that verb accepts survives here: a
/// prefix-layer key is one byte, so `f1`, `up` and `alt+x` are refused by name
/// (`amx-client/src/config/name.rs`, which says why). Action names are the
/// client's own verbs; `amx keys` prints
/// the resolved table, which is what makes them discoverable rather than
/// folklore.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct KeysConfig {
    /// The key that opens the prefix layer, ahead of the shipped `ctrl+a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Prefix-layer bindings: the key pressed after the prefix, to the action.
    ///
    /// Sorted rather than hashed so `amx keys` prints a stable table and a
    /// diff of two configurations is readable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bind: BTreeMap<String, String>,
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

    // Tier two: every section stands or falls alone. `next` starts at defaults
    // so a section the user deleted comes back as its default without anyone
    // writing that rule down twice.
    let mut next = Config::default();
    let mut diagnostics = Vec::new();
    section(
        &document,
        PERSIST_SECTION,
        &current.persist,
        &mut next.persist,
        &mut diagnostics,
    );
    section(
        &document,
        TERMINAL_SECTION,
        &current.terminal,
        &mut next.terminal,
        &mut diagnostics,
    );
    section(
        &document,
        UPDATE_SECTION,
        &current.update,
        &mut next.update,
        &mut diagnostics,
    );
    section(
        &document,
        WORK_SECTION,
        &current.work,
        &mut next.work,
        &mut diagnostics,
    );
    section(
        &document,
        CLIENT_SECTION,
        &current.client,
        &mut next.client,
        &mut diagnostics,
    );
    section(
        &document,
        KEYS_SECTION,
        &current.keys,
        &mut next.keys,
        &mut diagnostics,
    );
    (next, diagnostics)
}

/// Apply one section of `document` into `slot`, falling back to `current`.
///
/// Three outcomes, which are the three D-M1-8 clauses in one place: absent
/// leaves `slot` at the default it arrived with, valid overwrites it, invalid
/// copies the running values across and files a diagnostic naming the section.
/// Unknown keys inside the section never reach here — serde drops them, the
/// same tolerance the wire applies.
fn section<T>(
    document: &toml::Table,
    name: &'static str,
    current: &T,
    slot: &mut T,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) where
    T: Clone + serde::de::DeserializeOwned,
{
    let Some(value) = document.get(name) else {
        return;
    };
    match value.clone().try_into::<T>() {
        Ok(parsed) => *slot = parsed,
        Err(err) => {
            slot.clone_from(current);
            diagnostics.push(ConfigDiagnostic::section(name, err.to_string()));
        }
    }
}
