//! The prefix table: what a key does after the prefix, and where the row came
//! from.
//!
//! The key *names* are [`super::name`]'s; this is the table they index. The
//! division is the one the design draws — a grammar that says which bytes a
//! user can bind, and a table that says what those bytes then do — and keeping
//! them apart is what lets `amx keys` print a row without knowing how it was
//! spelled.

use std::collections::BTreeMap;

use amx_core::ConfigDiagnostic;
use amx_core::config::{KEYS_SECTION, KeysConfig};

use super::name::{key_byte, key_name};

/// One verb reachable from the prefix layer.
///
/// The client's own vocabulary, not the wire's: these are the things
/// `input::Input` does when a key is pressed after the prefix, and their names
/// are what a `[keys] bind` entry writes. `amx keys` prints them, which is what
/// makes them discoverable rather than folklore.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PrefixAction {
    /// Forward the key to the pane uninterpreted.
    ///
    /// The prefix key is always bound to this, which *is* the prefix-twice
    /// escape: the byte that would have opened the layer goes to the pane like
    /// any other. Binding a second key to it gives that key the same escape.
    Literal,
    /// Enter the sticky navigate layer.
    Navigate,
    /// Split the focused pane left/right.
    SplitHorizontal,
    /// Split the focused pane top/bottom.
    SplitVertical,
    /// Toggle zoom on the focused pane.
    Zoom,
    /// Detach this client, leaving the session running.
    Detach,
    /// Open the picker.
    Picker,
    /// Focus the head of the attention queue.
    NextAttention,
}

impl PrefixAction {
    /// Every action, in the order `amx keys` lists them.
    pub const ALL: &'static [Self] = &[
        Self::Literal,
        Self::Navigate,
        Self::SplitHorizontal,
        Self::SplitVertical,
        Self::Zoom,
        Self::Detach,
        Self::Picker,
        Self::NextAttention,
    ];

    /// The name a config file spells this action with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Navigate => "navigate",
            Self::SplitHorizontal => "split-horizontal",
            Self::SplitVertical => "split-vertical",
            Self::Zoom => "zoom",
            Self::Detach => "detach",
            Self::Picker => "picker",
            Self::NextAttention => "next-attention",
        }
    }

    /// Read an action name, case-insensitively.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        Self::ALL.iter().copied().find(|it| it.name() == lower)
    }
}

/// Where a resolved binding came from.
///
/// Carried per row because a table that cannot show you what your own config
/// did is a table you debug by guessing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// The binding amx ships.
    Shipped,
    /// A `[keys]` row in the user's `config.toml`.
    Config,
    /// The prefix key's own row, which follows the prefix wherever it is bound
    /// and is not overridable.
    Escape,
}

impl Source {
    /// How `amx keys` names this origin.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Shipped => "shipped",
            Self::Config => "config.toml",
            Self::Escape => "prefix escape",
        }
    }
}

/// One row of the resolved prefix table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Binding {
    /// What the key does.
    pub action: PrefixAction,
    /// Where the row came from.
    pub source: Source,
}

/// The prefix key and the prefix table, resolved.
///
/// [`Default`] is what amx ships, so a client that never reads a file and a
/// client whose file has no `[keys]` section run the same machine — which is
/// the whole of the "an unset section changes nothing" rule, expressed in a
/// type rather than in a convention.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bindings {
    prefix: u8,
    prefix_source: Source,
    table: BTreeMap<u8, Binding>,
}

/// The prefix amx ships: `ctrl+a` (04 §7).
pub const SHIPPED_PREFIX: u8 = 0x01;

/// The prefix table amx ships, in key order.
const SHIPPED: &[(u8, PrefixAction)] = &[
    (b'a', PrefixAction::NextAttention),
    (b'd', PrefixAction::Detach),
    (b'p', PrefixAction::Picker),
    (b'v', PrefixAction::SplitVertical),
    (b'w', PrefixAction::Navigate),
    (b'x', PrefixAction::SplitHorizontal),
    (b'z', PrefixAction::Zoom),
];

impl Default for Bindings {
    fn default() -> Self {
        let mut bindings = Self {
            prefix: SHIPPED_PREFIX,
            prefix_source: Source::Shipped,
            table: shipped_table(),
        };
        let _ = bindings.seat_escape();
        bindings
    }
}

/// The shipped table, *without* the escape row.
///
/// The escape is not in here because it is not a row about a key: it is a row
/// about wherever the prefix ended up, and [`Bindings::seat_escape`] is what
/// puts it there. Keeping the two separate is what makes rebinding the prefix
/// move the escape rather than leave a stale one behind on `ctrl+a`.
fn shipped_table() -> BTreeMap<u8, Binding> {
    SHIPPED
        .iter()
        .map(|&(key, action)| {
            (
                key,
                Binding {
                    action,
                    source: Source::Shipped,
                },
            )
        })
        .collect()
}

impl Bindings {
    /// Put the prefix-twice escape on the current prefix, returning whatever
    /// row it displaced.
    ///
    /// It goes on last and wins, so pressing the prefix twice always reaches
    /// the pane. It is the one row a file cannot take away: a user who has
    /// bound the prefix to a key their keyboard can emit still needs a way to
    /// send that key, and a table that could remove the way out would let a
    /// config file lock a program out of its own escape hatch.
    fn seat_escape(&mut self) -> Option<Binding> {
        self.table.insert(
            self.prefix,
            Binding {
                action: PrefixAction::Literal,
                source: Source::Escape,
            },
        )
    }

    /// The byte that opens the prefix layer.
    #[must_use]
    pub const fn prefix(&self) -> u8 {
        self.prefix
    }

    /// Where the prefix came from.
    #[must_use]
    pub const fn prefix_source(&self) -> Source {
        self.prefix_source
    }

    /// What `key` does in the prefix layer, if anything.
    #[must_use]
    pub fn action(&self, key: u8) -> Option<PrefixAction> {
        self.table.get(&key).map(|binding| binding.action)
    }

    /// Every row, in key order.
    pub fn rows(&self) -> impl Iterator<Item = (u8, Binding)> + '_ {
        self.table.iter().map(|(&key, &binding)| (key, binding))
    }
}

/// Resolve `[keys]` into the table the input machine runs on.
///
/// Every failure is per row: a prefix this build cannot read keeps the shipped
/// prefix, a `bind` entry naming an unreadable key or an unknown action loses
/// that entry, and everything else in the section still applies. The rule is
/// the config module's own, one level further down than it can enforce it.
#[must_use]
pub fn bindings_of(keys: &KeysConfig) -> (Bindings, Vec<ConfigDiagnostic>) {
    // Started from the shipped table with no escape row: the escape is seated
    // at the end, on whatever the prefix resolved to.
    let mut bindings = Bindings {
        prefix: SHIPPED_PREFIX,
        prefix_source: Source::Shipped,
        table: shipped_table(),
    };
    let mut diagnostics = Vec::new();

    if let Some(name) = &keys.prefix {
        match key_byte(name) {
            Ok(byte) => {
                bindings.prefix = byte;
                bindings.prefix_source = Source::Config;
            }
            Err(err) => diagnostics.push(rejected(format!("prefix = {name:?}: {err}"))),
        }
    }

    for (name, action) in &keys.bind {
        let key = match key_byte(name) {
            Ok(key) => key,
            Err(err) => {
                diagnostics.push(rejected(format!("bind.{name:?}: {err}")));
                continue;
            }
        };
        let Some(action) = PrefixAction::parse(action) else {
            diagnostics.push(rejected(format!(
                "bind.{name:?} = {action:?}: no such action. The actions are {}",
                actions()
            )));
            continue;
        };
        bindings.table.insert(
            key,
            Binding {
                action,
                source: Source::Config,
            },
        );
    }

    if let Some(displaced) = bindings.seat_escape()
        && displaced.source == Source::Config
    {
        diagnostics.push(rejected(format!(
            "bind.{:?} = {:?}: the prefix key always sends itself to the pane, \
             so this row cannot take effect",
            key_name(bindings.prefix),
            displaced.action.name(),
        )));
    }

    (bindings, diagnostics)
}

/// A `[keys]` row that did not resolve.
fn rejected(message: String) -> ConfigDiagnostic {
    ConfigDiagnostic::section(KEYS_SECTION, message)
}

/// The action names, for an error that has to list them.
fn actions() -> String {
    PrefixAction::ALL
        .iter()
        .map(|action| action.name())
        .collect::<Vec<_>>()
        .join(", ")
}
