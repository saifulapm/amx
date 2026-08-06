//! `session.*` payloads.

use amx_core::{Seq, SessionId};
use serde::{Deserialize, Serialize};

use crate::hello::ServerInfo;

/// Parameters of `ping`.
///
/// Empty, and deliberately a struct rather than a unit: adding a field later
/// must not change the wire shape from `null` to an object.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PingParams {}

/// Reply to `ping`.
///
/// Carries the bus sequence, like every state-carrying reply (04 §2), so even
/// a liveness probe tells the caller where the event stream is.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PingReply {
    /// Who answered.
    pub server: ServerInfo,
    /// The session instance that answered.
    pub session: SessionId,
    /// The bus head at reply time.
    pub seq: Seq,
}
