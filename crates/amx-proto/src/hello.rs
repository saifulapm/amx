//! Handshake: `Hello` → `Welcome`, and the resume state a reattach carries.

use std::borrow::Cow;
use std::collections::BTreeSet;

use amx_core::{GridGeneration, PaneId, Seq, SessionId};
use serde::{Deserialize, Serialize};

use crate::error::NegotiationError;

/// A named capability.
///
/// A newtype over a string rather than an enum, deliberately: 04 §4 negotiates
/// "capabilities, not equality", so a peer must be able to *name* a feature
/// this build has never heard of, round-trip it, and simply not have it in the
/// intersection. An enum would make an unknown feature a deserialization
/// failure — the strictness that strands peers.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Feature(Cow<'static, str>);

impl Feature {
    /// Per-pane grid deltas and keyframes on a bound binary stream.
    pub const GRID_STREAM: Self = Self(Cow::Borrowed("grid.stream"));
    /// Chunked history range transfer by stable row id.
    pub const HISTORY_RANGES: Self = Self(Cow::Borrowed("history.ranges"));
    /// Raw pane I/O streams (observe and control).
    pub const RAW_PANE_IO: Self = Self(Cow::Borrowed("pane.raw_io"));
    /// The client brings its own keybindings at handshake.
    pub const LOCAL_KEYBINDINGS: Self = Self(Cow::Borrowed("client.local_keybindings"));

    /// Name a feature this build does not have a constant for.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self(Cow::Owned(name.into()))
    }

    /// The feature's wire name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the client says about itself.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Program name, for `amx session list` and logs.
    pub name: String,
    /// Program version.
    pub version: String,
    /// The `TERM` the client's own terminal advertises, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
}

/// What the server says about itself.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server program name.
    pub name: String,
    /// Server version.
    pub version: String,
}

/// State a reattaching client already holds.
///
/// Reattach is a resume, not a fresh attach: the client says how far it read
/// the event stream and which grid generation it has per pane, and the server
/// answers with events after `last_seq` plus a keyframe for every pane whose
/// generation moved on (04 §4).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Resume {
    /// The last bus sequence the client consumed.
    pub last_seq: Seq,
    /// The grid generation the client holds for each pane it is caching.
    #[serde(default)]
    pub generations: Vec<(PaneId, GridGeneration)>,
}

/// The client's opening frame.
///
/// **No `deny_unknown_fields` here or on anything reachable from here.** 04 §4:
/// "Unknown JSON fields are ignored by contract." A newer client sending a
/// field this build has never seen must still connect — that is what makes the
/// N/N−1 window mean anything in practice.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Hello {
    /// The inclusive version window the client speaks, `[min, max]`.
    pub proto: (u16, u16),
    /// Features the client offers. Unknown names survive the round trip.
    #[serde(default)]
    pub features: BTreeSet<Feature>,
    /// Who the client is.
    pub client: ClientInfo,
    /// Present when this is a reattach rather than a first attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Resume>,
}

/// The server's answer.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Welcome {
    /// The single version chosen: the highest both sides speak.
    pub proto: u16,
    /// The feature **intersection** — never an equality check.
    #[serde(default)]
    pub features: BTreeSet<Feature>,
    /// Who the server is.
    pub server: ServerInfo,
    /// The bus sequence this welcome was captured at.
    ///
    /// Everything the client subsequently learns is either "state as of this
    /// seq" or an event after it, so there is no window between handshake and
    /// subscription in which a transition can be missed.
    pub seq: Seq,
    /// The server instance's identity, so a client can tell a restarted server
    /// from the one it was talking to.
    pub session: SessionId,
}

impl Hello {
    /// Negotiate a `Welcome` for this `Hello`.
    ///
    /// Picks the highest common version and echoes the feature intersection.
    /// Features the server does not know are dropped from the intersection
    /// silently; they are not an error, because "I do not have that" and "I
    /// have never heard of that" must be the same outcome for skew to work.
    pub fn accept(
        &self,
        server: ServerInfo,
        supported: &BTreeSet<Feature>,
        seq: Seq,
        session: SessionId,
    ) -> Result<Welcome, NegotiationError> {
        let proto = crate::version::negotiate(self.proto, crate::version::window())?;
        let features = self.features.intersection(supported).cloned().collect();
        Ok(Welcome {
            proto,
            features,
            server,
            seq,
            session,
        })
    }
}
