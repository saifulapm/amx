//! `agent.list`: the narrow, agent-only projection of the session
//! (`docs/10-attention-surfaces.md` §D15).
//!
//! Answered here, out of the state `Core` already holds plus the panes' own
//! published frames — one mailbox round trip for the whole reply, however many
//! panes there are (11-m4-plan D-M4-2). The alternative, a `StreamCall::Wiring`
//! per pane, is one round trip *each*, which at twenty-five agents is
//! twenty-five.
//!
//! # Why it is cheap, stated as the two things it does not do
//!
//! [`Core::agent_list`] is a plain synchronous method on the actor's own state,
//! reached from [`Core::absorb`](super::Core::absorb), which never awaits. So
//! it cannot ask a pane anything and it cannot ask the hub anything: everything
//! it reports is either in the state tree, in the hub's mirror
//! (`agent_status`), or one `Arc` clone out of a pane's lock-free frame slot.
//! `agent_list_answers_with_no_runtime_under_it` is the pin — a `Core` with no
//! Tokio runtime and no actor loop answers in full, which a fan-out could not.
//!
//! And [`Core::last_line`] walks the visible grid *from the bottom* and stops
//! at the first non-empty row, so the common case reads one row rather than the
//! grid. Nothing here touches scrollback, which is where the rows in a pane
//! actually are.
//!
//! X00's wave-1 baseline measured the surface this replaces: one
//! `session.state` plus one `pane.read` per pane, the only way to assemble
//! D15's table before this method existed, cost **161 ms** at 25 agents against
//! 8 ms for the state read alone (`docs/notes/m4-live-smoke.md` §1.4).

use amx_core::PaneId;
use amx_core::agent::{AgentWorkspace, EpochMillis};
use amx_proto::control::agent;
use amx_proto::rpc::RpcError;

use super::Core;

impl Core {
    /// Every tracked agent, one row each, captured at the current bus head.
    ///
    /// A pane is a row exactly when the hub has mirrored a status for it. That
    /// is the whole of "tracked": a pane the hub has never committed a
    /// transition for is *absent* rather than present with an invented state,
    /// because the three surfaces this feeds answer "which agents need me" and
    /// a row for every pane in the session is noise each of them would have to
    /// filter separately.
    ///
    /// A `workspace` that names no workspace is refused rather than answered
    /// with an empty list: a filter for a project that is not there and a
    /// project with no agents in it are different facts, and a caller holding a
    /// stale id learns which one it is holding.
    pub(super) fn agent_list(
        &self,
        params: &agent::ListParams,
    ) -> Result<agent::ListReply, RpcError> {
        if let Some(workspace) = params.workspace
            && self.state.workspace(workspace).is_none()
        {
            return Err(Self::no_such_workspace(workspace));
        }
        let mut agents = Vec::new();
        for ws in self.state.workspaces() {
            if params.workspace.is_some_and(|wanted| wanted != ws.id()) {
                continue;
            }
            let workspace = AgentWorkspace {
                id: ws.id(),
                name: ws.label().map(str::to_owned),
            };
            for pane in ws.layout().panes() {
                // The hub's mirror, which is also what `session.state`'s
                // per-pane `agent` block is served from: one fact, read twice,
                // so the agents view and the status line cannot disagree about
                // what a pane is doing.
                let Some(status) = self.agent_status.get(&pane) else {
                    continue;
                };
                agents.push(agent::AgentEntry {
                    workspace: workspace.clone(),
                    pane,
                    name: self
                        .state
                        .pane(pane)
                        .and_then(|pane| pane.label().map(str::to_owned)),
                    kind: status.kind.clone(),
                    status: status.state,
                    reason: status.reason.clone(),
                    since: status.since,
                    last_line: self.last_line(pane),
                });
            }
        }
        // The order `session.state` puts panes in, for the same reason it does:
        // shorts are issued in creation order and the state tree's own map is
        // not. Every display order D15 asks for — blocked-oldest-first, then
        // working, then idle — is a re-sort of this one, done where the
        // grouping is.
        agents.sort_by_key(|entry| self.short_of_pane(entry.pane).get());
        Ok(agent::ListReply {
            seq: self.ctx.bus.head(),
            now: wall_clock(),
            // Global and in queue order whatever the filter above did: a
            // narrowed queue would answer a different question than the one
            // `agent.next` acts on, and the two must never disagree.
            attention: self.attention.clone(),
            agents,
        })
    }

    /// The literal last non-empty visible row of `pane`, trailing blanks
    /// trimmed.
    ///
    /// Read off the pane's published frame — an `Arc` clone out of the
    /// lock-free slot that already serves `pane.read` and tier-2 detection — so
    /// the parser thread is not disturbed and there is no round trip. It is the
    /// same value `pane.read` puts at the bottom of its reply, because it is
    /// the same rows through the same `Row::line().trim_end()`; a pane's screen
    /// has one bottom line and amx must not have two answers for it.
    ///
    /// Never scrollback (which is client-side, 04 §3, and is `pane.history`'s
    /// business), never an interpretation, and never a generated summary —
    /// D15's fence, which exists because a model call per working session buys
    /// a token bill and a staleness window inside a monitor.
    ///
    /// The empty string for a pane that has printed nothing, and for a pane
    /// with no live process behind it: a restored pane whose process has not
    /// been started has no screen, and saying so with the same bytes a blank
    /// screen says is the honest answer, since neither has a bottom line.
    fn last_line(&self, pane: PaneId) -> String {
        let Some(host) = self.panes.get(&pane) else {
            return String::new();
        };
        let frame = host.frames().latest();
        // From the bottom, stopping at the first row with anything on it: a
        // pane painting a prompt at its last row costs one row, and the whole
        // grid is only walked for a screen that is entirely blank.
        frame
            .grid()
            .iter()
            .rev()
            .map(|row| row.line().trim_end())
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned()
    }
}

/// Now, in epoch milliseconds — the units D-M4-4 fixed, and the other half of
/// the pair a renderer computes an age from.
///
/// Every surface renders `now − since` from inside one reply and advances it
/// locally between refreshes, so a client whose clock disagrees with the
/// server's still shows the right age. That is the whole reason this field is
/// on the reply rather than left to the reader.
///
/// A clock set before 1970 is the only way `SystemTime::duration_since` can
/// fail, and zero is the honest floor for it rather than a lie: the same broken
/// clock leaves every `since` absent (the hub drops the stamp it could not
/// read), so there is nothing for a zero `now` to be subtracted from.
///
/// Deliberately a second copy of the hub's `wall_clock`
/// (`actor/agent_hub/commit.rs`) rather than a shared helper: that one is
/// private to an actor X06 owns and returns an `Option` because a *stamp* may
/// honestly be missing, while this one owes an answer. Widening someone else's
/// private function for a five-line body is a worse trade than two callers of
/// `SystemTime::now`.
fn wall_clock() -> EpochMillis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| EpochMillis::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}
