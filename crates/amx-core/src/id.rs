//! Identity.
//!
//! Every pane, workspace, client and session is identified by a UUID minted at
//! creation and stable for the lifetime of the object — across restore, across
//! moves between workspaces, across renames. Short numbers are display sugar
//! layered on top; nothing in the protocol or the state tree is keyed by
//! position (D7, D13).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::IdParseError;

macro_rules! uuid_id {
    ($(#[$attr:meta])* $name:ident, $label:literal) => {
        $(#[$attr])*
        ///
        /// Wire form is the hyphenated UUID string; `FromStr` and `Display` are
        /// exact inverses, which is what makes ids usable as CLI arguments.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[doc = concat!("Mint a fresh `", $label, "`.")]
            ///
            /// This is the only way to create an identity that did not already
            /// exist: there is no `from_index`, because an identity derived
            /// from position is an identity that changes when the tree does.
            #[must_use]
            pub fn new_v4() -> Self {
                Self(Uuid::new_v4())
            }

            #[doc = concat!("Rebuild a `", $label, "` that already exists, from the wire or from a snapshot.")]
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            #[doc = concat!("Rebuild a `", $label, "` from its raw UUID bytes, e.g. off a binary stream.")]
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }

            /// The raw UUID bytes, for binary stream encodings.
            #[must_use]
            pub const fn into_bytes(self) -> [u8; 16] {
                self.0.into_bytes()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", $label, self.0.hyphenated())
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|source| IdParseError { kind: $label, source })
            }
        }
    };
}

uuid_id!(
    /// Identity of a pane: one PTY, one terminal grid, one process tree.
    ///
    /// Stable across moves between workspaces and across swaps — rearranging
    /// panes never restarts the process in the pane, so it never changes the
    /// pane's identity either.
    PaneId,
    "PaneId"
);

uuid_id!(
    /// Identity of a workspace: one BSP layout tree over a set of panes.
    ///
    /// The tree is two levels — workspaces → panes; tabs are deliberately
    /// flattened away (D13).
    WorkspaceId,
    "WorkspaceId"
);

uuid_id!(
    /// Identity of an attached client connection.
    ///
    /// Clients own presentation only (04 §3); the id exists so the server can
    /// keep per-client viewport, damage and flow-control bookkeeping.
    ClientId,
    "ClientId"
);

uuid_id!(
    /// Identity of a server instance (one named session, one socket).
    ///
    /// Carried in `Welcome` so a reattaching client can tell "the same server I
    /// was talking to" from "a new server at the same socket path".
    SessionId,
    "SessionId"
);

/// Monotonic grid generation for one pane.
///
/// Bumped on every resize and on every reset — the events that make previously
/// sent cells meaningless. 04 §4: the per-client accumulated dirty set is
/// "rects + grid generation", and a generation mismatch on reconnect is one of
/// the three triggers for a keyframe (`GridMessage::Reset`), the others being a
/// damage threshold crossing and an explicit resync.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GridGeneration(u64);

impl GridGeneration {
    /// The generation a pane's grid starts at.
    pub const FIRST: Self = Self(0);

    /// Rebuild a generation from the wire or from a snapshot.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next generation.
    ///
    /// Saturating: the counter is monotonic by contract, and wrapping it would
    /// make a stale client look current.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for GridGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A user-visible small integer standing in for a UUID.
///
/// Short numbers exist so a human can type `2` instead of a UUID. They are a
/// display mapping and never an identity: nothing addresses state by short
/// number internally, and a short number can be re-used after the object it
/// pointed at is gone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShortNumber(u32);

impl ShortNumber {
    /// The first number handed out.
    pub const FIRST: Self = Self(1);

    /// Wrap a raw number.
    #[must_use]
    pub const fn new(number: u32) -> Self {
        Self(number)
    }

    /// The raw number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ShortNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The display mapping from identity to short number, for one kind of object.
///
/// The mapping is *state*, not a derivation from position: it is assigned at
/// creation, serialized with the session, and therefore stable across restarts
/// — a workspace that was `2` yesterday is `2` after a restore, even if the
/// tree order changed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShortNumbers<K: Ord> {
    assigned: BTreeMap<K, ShortNumber>,
}

impl<K: Ord> Default for ShortNumbers<K> {
    fn default() -> Self {
        Self {
            assigned: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Copy> ShortNumbers<K> {
    /// An empty mapping.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Assign the lowest unused short number to `key`, or return the number it
    /// already holds.
    pub fn assign(&mut self, key: K) -> ShortNumber {
        let _ = key;
        todo!("assign the lowest free short number")
    }

    /// The short number currently displayed for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<ShortNumber> {
        self.assigned.get(key).copied()
    }

    /// The identity a user-typed short number refers to, if any.
    ///
    /// Resolution is the only direction that may fail ambiguously — a number
    /// whose object is gone resolves to `None` rather than to whatever took the
    /// slot next, because the mapping is only reused after an explicit release.
    #[must_use]
    pub fn resolve(&self, number: ShortNumber) -> Option<K> {
        let _ = number;
        todo!("reverse the mapping")
    }

    /// Drop the mapping for `key`, freeing its number for reuse.
    pub fn release(&mut self, key: &K) -> Option<ShortNumber> {
        self.assigned.remove(key)
    }

    /// How many objects currently hold a short number.
    #[must_use]
    pub fn len(&self) -> usize {
        self.assigned.len()
    }

    /// Whether no object holds a short number.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assigned.is_empty()
    }
}
