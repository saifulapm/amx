//! What an attention event says on its own.
//!
//! `docs/10-attention-surfaces.md` §D15 froze an identity block —
//! `workspace{id,name}`, `pane`, `name`, `reason`, `since` — onto
//! `attention_enqueued` and `attention_dequeued` for one stated reason: "so a
//! notifier can say *api/backend blocked (permission request)* without a
//! follow-up query". This module is that sentence, and
//! [`watch`](super::watch) puts it on the footer.
//!
//! # Why the table beside it is still re-queried
//!
//! The block names *one* agent and says nothing about the other twenty-four, so
//! it can answer "what just happened" and can never answer "what is on screen
//! now" — it carries no `status`, no `last_line` and no queue order, and a row
//! folded from it would be a row the server never sent. [`watch`](super::watch)
//! therefore keeps the re-query for the table and reads the block for the line
//! under it. The two answer different questions from the same delivery.
//!
//! And one of those questions has no query at all behind it:
//!
//! - a block that clears inside one refresh window never appears in any
//!   `agent.list` — the enqueue and the dequeue both land between two calls,
//!   and the table a reader sees is the same table before and after;
//! - a dequeue's subject may be gone. `AgentHub` forgets a pane's label
//!   *after* publishing the dequeue its exit causes
//!   (`crates/amx-server/src/actor/agent_hub/names.rs:63-70`), so the event
//!   names an agent that the next `agent.list` has no row for at all.
//!
//! Neither is recoverable by asking again, which is what makes this a reader
//! and not a shortcut.

use amx_core::agent::{AgentWorkspace, EpochMillis};
use amx_core::{Event, PaneId, WorkspaceId};

use super::age;

/// How many characters of a pane's uuid stand in for a name it has not got.
///
/// A prefix, said as one — not an identifier this surface invented. It is here
/// because a footer is a single line on a 45-column window and the whole uuid
/// is thirty-six characters of it.
const PREFIX: usize = 8;

/// The last attention transition this watch was told about.
///
/// Held rather than rendered on arrival because the footer is redrawn on every
/// refresh: the age below moves, and a reader who glances up a minute later has
/// to be able to tell an announcement that is a minute old from one that has
/// just landed.
#[derive(Debug)]
pub struct Announcement {
    /// The agent, the way a person names it.
    who: String,
    /// Which side of the queue it moved to.
    left: bool,
    /// The detector that named the state being announced.
    reason: Option<String>,
    /// The entry edge of that state.
    ///
    /// On an enqueue this is when the agent blocked, so the age is how long it
    /// has been waiting. On a dequeue it is when it entered the state it left
    /// the queue *in*, so the age is how long ago it stopped waiting. Both are
    /// "how old is this announcement", which is the question a line that stays
    /// on the footer has to answer.
    since: Option<EpochMillis>,
}

impl Announcement {
    /// What `event` announces, if it announces anything to a watch of `scope`.
    ///
    /// `None` for every event that is not an attention transition — this reads
    /// the block D15 froze and does not invent one for the rest of the bus.
    ///
    /// A scoped watch (`--workspace api`) announces only its own workspace, and
    /// only when the event says which workspace it belongs to: the block is
    /// optional on the wire, and a pre-M4 server's enqueue could belong to any
    /// project. Silence is the honest answer there, since the alternative is
    /// putting another project's agent on a screen that promised one.
    #[must_use]
    pub fn of(event: &Event, scope: Option<WorkspaceId>) -> Option<Self> {
        let (pane, workspace, name, reason, since, left) = match event {
            Event::AttentionEnqueued {
                pane,
                workspace,
                name,
                reason,
                since,
            } => (pane, workspace, name, reason, since, false),
            Event::AttentionDequeued {
                pane,
                workspace,
                name,
                reason,
                since,
            } => (pane, workspace, name, reason, since, true),
            _ => return None,
        };
        if let Some(scope) = scope
            && workspace.as_ref().map(|workspace| workspace.id) != Some(scope)
        {
            return None;
        }
        Some(Self {
            who: who(workspace.as_ref(), name.as_deref(), *pane),
            left,
            reason: reason.clone(),
            since: *since,
        })
    }

    /// The sentence, against the server's clock as this watch estimates it.
    #[must_use]
    pub fn line(&self, now: EpochMillis) -> String {
        let mut out = format!(
            "{} {}",
            self.who,
            if self.left { "cleared" } else { "blocked" }
        );
        let age = age(now, self.since);
        if !age.is_empty() {
            out.push(' ');
            out.push_str(&age);
        }
        if let Some(reason) = &self.reason {
            out.push_str(&format!(" ({reason})"));
        }
        out
    }
}

/// One agent, named the way the table names it, degrading a field at a time.
///
/// Every field of the block is optional because the hub sends what it has been
/// told and never asks a sibling for a label (`docs/11-m4-plan.md` D-M4-6), so
/// this falls back rather than printing a placeholder: `api/backend`, then
/// `backend` for an agent in a workspace nobody labelled, then the pane the
/// event always carried.
fn who(workspace: Option<&AgentWorkspace>, name: Option<&str>, pane: PaneId) -> String {
    let project = workspace.and_then(|workspace| workspace.name.as_deref());
    match (project, name) {
        (Some(project), Some(name)) => format!("{project}/{name}"),
        (None, Some(name)) => name.to_owned(),
        _ => {
            let id = pane.to_string();
            format!("pane {}…", &id[..PREFIX.min(id.len())])
        }
    }
}
