//! `workspace.*` payloads.

use amx_core::{Seq, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Parameters of `workspace.create`.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct CreateParams {
    /// Optional user-visible label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether to focus the new workspace.
    #[serde(default)]
    pub focus: bool,
}

/// Reply to `workspace.create`.
///
/// Returns both identities: the UUID everything is keyed by, and the short
/// number a human types. The short number is display sugar and is never what
/// the client sends back.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CreateReply {
    /// The new workspace.
    pub workspace: WorkspaceId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// The bus sequence at which the workspace existed.
    pub seq: Seq,
}
