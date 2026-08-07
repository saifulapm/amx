//! Resume refs: the conversation a pane can be restarted into.
//!
//! 04 §5 keeps herdr's resume rigor wholesale: "refs validated, argv as data,
//! allowlisted sources, dedupe reservations with rollback — but the tables come
//! from the registry." `docs/08-m2-plan.md` D-M2-7 spells out what that means
//! for these types:
//!
//! > **Refs are shape-validated data**: `{kind: id|path, value}`, non-empty,
//! > length-capped (512/4096), no control characters, `path` must be absolute,
//! > `path` only for agents whose stanza says so. Constructors return `Option`;
//! > **there is no other way to build one**.
//!
//! "No other way" includes serde. [`SessionRef`] and [`RefSource`] deserialize
//! through their own validation, so the gate D-M2-7 puts at snapshot-read time
//! is not a call somebody has to remember to make — a `session.json` a user
//! hand-edited to hold a relative path, a control character or a 9 KiB blob
//! fails to parse into a ref at all, and the pane restores as a plain shell.
//!
//! Two of D-M2-7's three checks are here; the third is not, and cannot be.
//! Shape is context-free and lives on the type. **Source allowlisting** —
//! "a ref is only accepted from the hook path of the agent it claims
//! (`source == "amx:<agent-id>"` cross-checked against the stanza)" — needs the
//! registry, so [`RefSource`] validates its *shape* here and the cross-check
//! against a real stanza is the registry's (V03's) to run at each of D-M2-7's
//! three gates.
//!
//! # Task ownership
//!
//! V02 froze these types and implemented their validation, because a validating
//! constructor that panicked would be worse than no constructor at all — three
//! later tasks build refs before the one that consumes them exists. **V09**
//! mints refs from hook reports, **V15** reads them back, plans argv from the
//! registry template, and owns the reservation and rollback.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::AgentKind;

/// What kind of thing a [`SessionRef`]'s value is.
///
/// Which kinds an agent accepts is stanza data (`ref_kind` in D-M2-2), so a
/// [`Path`](Self::Path) ref for an agent whose stanza says `id` is rejected by
/// the registry even though it is well-formed here.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// An opaque session identifier the agent hands back on `--resume`.
    ///
    /// Both agents amx ships are this: V01 §3 M8 measured Claude Code's as a
    /// v4-shaped UUID and Codex's as a UUIDv7, and in both cases the resume
    /// ref *is* the `session_id` the hook payload carries.
    Id,
    /// An absolute path to a conversation file.
    Path,
}

/// A conversation an agent can be resumed into.
///
/// Shape-validated on construction and on deserialization alike; see the module
/// documentation for why that matters and for what it deliberately does *not*
/// check.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "RawRef", into = "RawRef")]
pub struct SessionRef {
    kind: RefKind,
    value: String,
}

impl SessionRef {
    /// The longest an [`RefKind::Id`] value may be.
    pub const MAX_ID_LEN: usize = 512;

    /// The longest a [`RefKind::Path`] value may be.
    ///
    /// `PATH_MAX` on Linux; a value longer than the kernel would accept cannot
    /// be a path this machine can open.
    pub const MAX_PATH_LEN: usize = 4096;

    /// The ref `value` names, if it is well-formed for `kind`.
    ///
    /// Well-formed is: non-empty; within the kind's length cap; free of control
    /// characters (which is what keeps a ref out of a terminal's own escape
    /// vocabulary when it is printed, and out of an argv splitter's); and, for
    /// [`RefKind::Path`], absolute.
    ///
    /// # Errors
    ///
    /// [`RefError`] naming which rule it broke — the restore report shows the
    /// reason to the user, so "invalid ref" would not be enough (04 §6: a
    /// conversation that did not resume is a loss the user is told about).
    pub fn new(kind: RefKind, value: impl Into<String>) -> Result<Self, RefError> {
        let value = value.into();
        if value.is_empty() {
            return Err(RefError::Empty);
        }
        let max = match kind {
            RefKind::Id => Self::MAX_ID_LEN,
            RefKind::Path => Self::MAX_PATH_LEN,
        };
        if value.len() > max {
            return Err(RefError::TooLong {
                len: value.len(),
                max,
            });
        }
        if value.chars().any(char::is_control) {
            return Err(RefError::ControlCharacter);
        }
        if kind == RefKind::Path && !Path::new(&value).is_absolute() {
            return Err(RefError::RelativePath);
        }
        Ok(Self { kind, value })
    }

    /// What kind of ref this is.
    #[must_use]
    pub const fn kind(&self) -> RefKind {
        self.kind
    }

    /// The ref itself.
    ///
    /// Substituted into the stanza's `resume` template's single `{ref}` slot
    /// and returned as one element of a `Vec<String>` — never interpolated into
    /// a shell string (D-M2-7: "argv is data").
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// The unvalidated shape [`SessionRef`] serializes as.
///
/// Private: its only purpose is to give serde something to deserialize *into*
/// before [`SessionRef::new`] gets a say, which is what makes the read gate
/// unskippable.
#[derive(Serialize, Deserialize)]
struct RawRef {
    kind: RefKind,
    value: String,
}

impl TryFrom<RawRef> for SessionRef {
    type Error = RefError;

    fn try_from(raw: RawRef) -> Result<Self, Self::Error> {
        Self::new(raw.kind, raw.value)
    }
}

impl From<SessionRef> for RawRef {
    fn from(session_ref: SessionRef) -> Self {
        Self {
            kind: session_ref.kind,
            value: session_ref.value,
        }
    }
}

/// Why a value is not a [`SessionRef`].
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum RefError {
    /// The value was empty.
    #[error("a session ref cannot be empty")]
    Empty,
    /// The value was longer than its kind's cap.
    #[error("a session ref of this kind is at most {max} bytes, got {len}")]
    TooLong {
        /// How long it was.
        len: usize,
        /// The cap for its kind.
        max: usize,
    },
    /// The value held a control character.
    #[error("a session ref cannot hold control characters")]
    ControlCharacter,
    /// A [`RefKind::Path`] value was not absolute.
    #[error("a path session ref must be absolute")]
    RelativePath,
    /// The source was not `amx:<agent-id>`.
    #[error("a ref source is `amx:<agent-id>`, got {found:?}")]
    BadSource {
        /// What was there instead.
        found: String,
    },
}

/// Where a [`SessionRef`] came from.
///
/// D-M2-7's allowlist: "a ref is only accepted from the hook path of the agent
/// it claims (`source == "amx:<agent-id>"` cross-checked against the stanza),
/// checked at report time, at snapshot-read time, and again at plan time — a
/// hand-edited `session.json` cannot inject an argv".
///
/// This type carries the `amx:<agent-id>` shape and nothing more. The
/// cross-check — that the id names a stanza, and the *same* stanza as the ref's
/// claimed agent — is the registry's, because only the registry knows what
/// stanzas exist. [`agent`](Self::agent) is what it takes to run it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RefSource(String);

impl RefSource {
    /// The prefix that marks a source as one of amx's own hook paths.
    pub const PREFIX: &'static str = "amx:";

    /// The source a report from `kind`'s hook path carries.
    #[must_use]
    pub fn for_agent(kind: &AgentKind) -> Self {
        Self(format!("{}{kind}", Self::PREFIX))
    }

    /// The source `text` names, if it is one amx would ever write.
    ///
    /// # Errors
    ///
    /// [`RefError::BadSource`] for anything that is not `amx:` followed by a
    /// well-formed [`AgentKind`]. A foreign source is not an error to shout
    /// about — it is a ref amx refuses to plan an argv from, reported as a
    /// degraded restore.
    pub fn new(text: impl Into<String>) -> Result<Self, RefError> {
        let text = text.into();
        let bad = || RefError::BadSource {
            found: text.clone(),
        };
        let id = text.strip_prefix(Self::PREFIX).ok_or_else(bad)?;
        AgentKind::new(id).map_err(|_| bad())?;
        Ok(Self(text))
    }

    /// The agent id this source claims.
    ///
    /// Infallible: the only constructors validate it.
    #[must_use]
    pub fn agent(&self) -> AgentKind {
        let id = self.0.strip_prefix(Self::PREFIX).unwrap_or_default();
        // The invariant: every constructor parsed this suffix as an
        // `AgentKind` before building the value, and the field is private.
        #[allow(clippy::expect_used, reason = "invariant: validated on construction")]
        AgentKind::new(id).expect("a RefSource's suffix is a valid agent id")
    }

    /// The source, as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RefSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RefSource {
    type Error = RefError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

impl From<RefSource> for String {
    fn from(source: RefSource) -> Self {
        source.0
    }
}

/// How the agent in a pane got there.
///
/// Persisted beside the ref (D-M2-7) because restore treats the two differently:
/// a pane amx started for the user is one amx may start again, and a pane the
/// user typed an agent into by hand is one whose argv amx never recorded.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartSource {
    /// `agent start` spawned it, so its argv is amx's own and is recorded.
    Started,
    /// The user typed the agent at the pane's shell; amx identified it after
    /// the fact. Attribution is identical either way (D-M2-4) — only the
    /// recorded argv differs.
    Detected,
    /// A restore typed it back into a respawned shell.
    Restored,
}
