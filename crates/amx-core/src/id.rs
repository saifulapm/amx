//! Identity.
//!
//! Every pane, workspace, client and session is identified by a UUID minted at
//! creation and stable for the lifetime of the object — across restore, across
//! moves between workspaces, across renames. Short numbers are display sugar
//! layered on top; nothing in the protocol or the state tree is keyed by
//! position (D7, D13).

use std::cmp::Ordering;
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

    /// Read a user-typed short number, if that is what this string is.
    ///
    /// Digits and nothing else: whoever resolves a target checks short numbers
    /// before labels, so "is this a short number" has to be a question about
    /// the string alone. Answering it from what happens to be in the tree is
    /// how `7` would name a pane one minute and a label the next.
    ///
    /// So no sign, no spaces, no `0x`, and a number too large for the counter
    /// is not one: `"+7"`, `" 7"` and `"7 "` are labels.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() || !s.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        s.parse::<u32>().ok().map(Self)
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
    ///
    /// Lowest free rather than next highest: the numbers are what a user
    /// types, and a session that has opened and closed forty panes over a day
    /// should still be offering `1`–`4` rather than `37`–`40`. The cost is
    /// that a number means something different after a release, which is why
    /// [`ShortNumbers::resolve`] is only ever a display lookup and never an
    /// identity (04 §6).
    pub fn assign(&mut self, key: K) -> ShortNumber {
        if let Some(held) = self.assigned.get(&key) {
            return *held;
        }
        let number = self.lowest_free();
        self.assigned.insert(key, number);
        number
    }

    /// Take `number` for `key` — restore and handoff import putting back the
    /// numbers a previous server handed out — or the lowest free number if
    /// another key already holds it.
    ///
    /// 04 §6 requires short numbers "stable across restarts", so the recorded
    /// number wins whenever it can. Two objects claiming one number is a
    /// rewritten file rather than a session, and the answer is to keep the
    /// mapping one-to-one anyway: [`ShortNumbers::resolve`] over a duplicated
    /// number would answer with whichever key the walk reached first, which is
    /// a number that names a different pane depending on nothing.
    ///
    /// Returns the number `key` ended up with.
    pub fn adopt(&mut self, key: K, number: ShortNumber) -> ShortNumber {
        let taken = self
            .assigned
            .iter()
            .any(|(held, held_number)| *held_number == number && *held != key);
        if taken {
            return self.assign(key);
        }
        self.assigned.insert(key, number);
        number
    }

    /// The lowest number no key holds.
    ///
    /// Sorts on every call. The map holds one entry per live pane or
    /// workspace — tens, in the session this is built for — and the
    /// alternative is a reverse index that has to be rebuilt after every
    /// deserialize, for a walk nothing measures.
    fn lowest_free(&self) -> ShortNumber {
        let mut taken: Vec<u32> = self.assigned.values().map(|number| number.get()).collect();
        taken.sort_unstable();
        let mut free = ShortNumber::FIRST.get();
        for used in taken {
            match used.cmp(&free) {
                // Below the first number ever handed out: only an adopted
                // number can be down here, and it blocks nothing.
                Ordering::Less => {}
                // Saturating for the same reason every other counter here is,
                // and unreachable for the ordinary one: `u32::MAX` live
                // objects is four billion panes.
                Ordering::Equal => free = free.saturating_add(1),
                Ordering::Greater => break,
            }
        }
        ShortNumber::new(free)
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
        self.assigned
            .iter()
            .find(|(_, held)| **held == number)
            .map(|(key, _)| *key)
    }

    /// Drop the mapping for `key`, freeing its number for reuse.
    pub fn release(&mut self, key: &K) -> Option<ShortNumber> {
        self.assigned.remove(key)
    }

    /// Release every mapping `keep` rejects.
    ///
    /// Bulk [`ShortNumbers::release`], for the caller that holds the list of
    /// objects that still exist rather than the list of ones that went: a
    /// number outliving its object is a number that resolves to something the
    /// user cannot see.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.assigned.retain(|key, _| keep(key));
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
