//! `--workspace`: what the user typed, and the id the wire wants.
//!
//! X02 froze `ListParams.workspace` as a [`WorkspaceId`], like every other
//! `workspace` parameter on the method table, and recorded the consequence for
//! this task: "X16's `--workspace api` therefore resolves client-side". A
//! `WorkspaceTarget` would have put a second name resolver in the server for a
//! namespace that has exactly one way of being resolved, and the CLI is already
//! holding `session.state` — which carries every workspace's label — whenever
//! it needs one.
//!
//! Resolution is deliberately *not* done once and kept. `--watch` re-resolves
//! on every fresh connection, because a cold restart is a new session whose
//! workspace ids are new too, and a watch that kept the old id would ask the
//! successor about a workspace it has never heard of. A handoff keeps ids, so
//! the re-resolution costs one extra `session.state` per dial and answers the
//! same id it had.

use amx_client::net::Session;
use amx_core::WorkspaceId;
use amx_proto::control::agent::ListParams;
use amx_proto::control::session::StateReply;
use anyhow::Context as _;
use serde_json::{Value, json};

/// The `--workspace` argument, as written.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    wanted: Option<String>,
}

impl Scope {
    /// The scope `--workspace` selects, or the whole session.
    #[must_use]
    pub fn new(wanted: Option<String>) -> Self {
        Self { wanted }
    }

    /// What the user typed, for a message that has to quote it.
    #[must_use]
    pub fn wanted(&self) -> Option<&str> {
        self.wanted.as_deref()
    }

    /// The `agent.list` parameters this scope means, resolving a label against
    /// the session if that is what it is.
    ///
    /// A value that parses as a workspace id is used as one and never looked
    /// up: an id is what the wire takes, and a session where somebody has
    /// labelled a workspace with another workspace's id is not a session this
    /// command should be inventing an opinion about.
    pub async fn params(&self, session: &mut Session) -> anyhow::Result<Value> {
        let Some(wanted) = &self.wanted else {
            return Ok(json!({}));
        };
        if let Ok(id) = wanted.parse::<WorkspaceId>() {
            return params_for(id);
        }
        let state = session
            .call("session.state", json!({}))
            .await
            .context("read the session's state to resolve --workspace")?;
        let state: StateReply =
            serde_json::from_value(state).context("decode the session's state")?;
        let found = state
            .workspaces
            .iter()
            .find(|workspace| workspace.label.as_deref() == Some(wanted.as_str()))
            .map(|workspace| workspace.workspace);
        let Some(id) = found else {
            let known: Vec<&str> = state
                .workspaces
                .iter()
                .filter_map(|workspace| workspace.label.as_deref())
                .collect();
            anyhow::bail!(
                "no workspace named {wanted} in this session{}",
                if known.is_empty() {
                    String::new()
                } else {
                    format!("; it has {}", known.join(", "))
                }
            );
        };
        params_for(id)
    }
}

/// One workspace's `agent.list` parameters.
fn params_for(id: WorkspaceId) -> anyhow::Result<Value> {
    serde_json::to_value(ListParams {
        workspace: Some(id),
    })
    .context("encode the agent.list parameters")
}
