//! The `amx agents` table: one `agent.list` reply, rendered for a person.
//!
//! D15 surface 3 (`docs/10-attention-surfaces.md`), and D-M4-11 is why it is a
//! hand-written verb rather than a method-table row: `amx agent list` is the
//! machine surface the table generates for free, and this is the same reply
//! read by a human, with `--json` printing it verbatim so nobody has to know
//! there are two spellings.
//!
//! Both forms of the command render through here — the one-shot table and
//! `--watch`'s live screen are the same [`table::render`] over the same
//! [`ordered`] rows, so a column that moves moves in both.
//!
//! # What the order is, and why it is not the server's
//!
//! `agent.list` answers in `session.state`'s pane order — by short number,
//! which is creation order (X10's wave outcome). That is the only order that is
//! a *fact*; every display order D15 asks for is a re-sort of it, done where
//! the display is. [`ordered`] is this surface's re-sort: blocked first in the
//! attention queue's own order, then working, then the tier-3 `busy`, then
//! idle, then `quiet`, with a project's rows kept together inside each band.
//!
//! The blocked band is keyed on [`ListReply::attention`] rather than on
//! `since`, deliberately. The queue is what `agent.next` walks, and a table
//! whose top row is not the pane `amx agent next` would jump to is a table that
//! disagrees with the key beside it — the same trap X11 pinned on the status
//! line, where the count a user reads and the queue the jump walks are one
//! number.
//!
//! # Ages, and the three things that can make one misleading
//!
//! Every age is `now − since` **from inside one reply** (D-M4-4): the server's
//! own wall clock against the server's own stamp, so an `amx agents` typed on
//! the other end of an SSH link reads the same age as one typed on the server's
//! console. Nothing here calls [`std::time::SystemTime::now`], and the
//! `--watch` loop advances the reply's `now` by *monotonic* elapsed time rather
//! than by a second clock.
//!
//! Three honest caveats, none of which this renderer can fix and all of which a
//! reader should know:
//!
//! - A pane whose status was re-derived rather than observed entering carries
//!   no `since` at all, and its age column is blank rather than zero.
//! - After a cold restore, `since` is the restore — the persist snapshot
//!   deliberately carries no status, so a restored agent's first transition in
//!   the new server is a real observation and is dated from it (X06's wave
//!   outcome). An agent idle all night reads as idle for four seconds.
//! - `reason` is the detector's own name and there are two vocabularies:
//!   `permission_dialog` from a manifest rule, `PermissionRequest` from the
//!   hook that usually wins the same edge by 8–14 ms. Both are printed
//!   verbatim. Nothing here switches on a known set, because a manifest rule
//!   written tomorrow is a `reason` tomorrow (D-M4-3).
//!
//! # Sharing with the agents view (X00's seam 5)
//!
//! X00 holds `crates/amx` against `amx-client` as one seam, so that
//! `amx agents --watch` and the client's agents view (X14) do not become two
//! implementations of one table. They do not share code, and the reason is a
//! dependency direction rather than a preference: `amx-client` is *below* this
//! crate and cannot reach into it, so shared code would have to be a new module
//! inside `amx-client` — a file in neither task's scope, landed mid-wave into a
//! crate whose owner is already building against something else. What the two
//! surfaces share instead is everything that is a *fact*: the reply, the queue
//! order it carries, and the rules stated above. The full account is in
//! `docs/notes/m4-wave-outcomes.md`.

pub mod table;

use std::collections::HashMap;

use amx_core::agent::{AgentState, EpochMillis};
use amx_core::{PaneId, WorkspaceId};
use amx_proto::control::agent::{AgentEntry, ListReply};

/// `reply`'s rows in the order this surface shows them.
///
/// Blocked first, in [`ListReply::attention`]'s own order; then working, busy,
/// idle and quiet, each band keeping a workspace's rows together and, inside a
/// workspace, the server's creation order. See the module documentation for
/// why the blocked band is keyed on the queue and not on `since`.
#[must_use]
pub fn ordered(reply: &ListReply) -> Vec<&AgentEntry> {
    let queue: HashMap<PaneId, usize> = reply
        .attention
        .iter()
        .enumerate()
        .map(|(at, pane)| (*pane, at))
        .collect();
    // First appearance, which is the server's workspace order: the point is
    // only that one project's rows land together, not that projects are
    // alphabetical — a session's workspaces have an order their owner chose.
    let mut order: Vec<WorkspaceId> = Vec::new();
    for entry in &reply.agents {
        if !order.contains(&entry.workspace.id) {
            order.push(entry.workspace.id);
        }
    }
    let workspace_at = |id: WorkspaceId| order.iter().position(|seen| *seen == id).unwrap_or(0);

    let mut rows: Vec<(u8, usize, usize, &AgentEntry)> = reply
        .agents
        .iter()
        .enumerate()
        .map(|(at, entry)| {
            let rank = rank(entry.status);
            // A blocked row that is somehow not on the queue sorts after the
            // ones that are, rather than ahead of them: the queue is the fact,
            // and a row it does not name is the one to distrust.
            let within = if entry.status.wants_attention() {
                queue.get(&entry.pane).copied().unwrap_or(usize::MAX)
            } else {
                workspace_at(entry.workspace.id)
            };
            (rank, within, at, entry)
        })
        .collect();
    rows.sort_by_key(|(rank, within, at, _)| (*rank, *within, *at));
    rows.into_iter().map(|(_, _, _, entry)| entry).collect()
}

/// Which band a state sorts into.
///
/// D15's "blocked (oldest first) → working → idle", with the two tier-3 states
/// placed around idle: `busy` is a program producing output and belongs beside
/// working, `quiet` is one that is not and belongs last. Neither is an agent
/// (`AgentState::is_agent`), so neither may ever outrank one that is.
const fn rank(state: AgentState) -> u8 {
    match state {
        AgentState::Blocked => 0,
        AgentState::Working => 1,
        AgentState::Busy => 2,
        AgentState::Idle => 3,
        AgentState::Quiet => 4,
    }
}

/// `since` as an age against `now`, in the coarsest unit that still says
/// something.
///
/// The empty string when the entry carries no `since`: a pane whose status was
/// re-derived rather than observed entering has no edge to date, and `0s` would
/// be a measurement nobody made.
///
/// A `since` in the future is `0s` rather than a negative age. It should not
/// happen — both halves come from one reply — but a saturating answer is better
/// than a panic in a monitor.
#[must_use]
pub fn age(now: EpochMillis, since: Option<EpochMillis>) -> String {
    let Some(since) = since else {
        return String::new();
    };
    let seconds = now.saturating_sub(since) / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}
