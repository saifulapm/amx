//! M4's one new row, frozen: `agent.list` (`docs/11-m4-plan.md` §3).
//!
//! A child module of [`super`] on the same terms as [`m2`](super::m2): it
//! shares that file's `call_golden` and its fixture ids, because two golden
//! writers with two notions of what a pane id is would produce two sets of
//! bytes for one wire.
//!
//! Values here are documentation. The row is the one D15 draws in prose —
//! `api/backend ⚑ blocked permission 4m │ Allow Bash(git push origin main)?
//! (y/n)` — so a reader of the golden sees the shape the agents view renders
//! rather than a generator's zeros. The `reason` is the *shipped* manifest rule
//! name from `crates/amx-server/assets/manifests/claude.toml`, not a
//! translation of it: D-M4-3 makes a new rule self-describing on the wire the
//! day it is written, and a golden carrying an invented word would be the first
//! step towards the second vocabulary that decision refuses.

use amx_core::agent::{AgentKind, AgentState, AgentWorkspace, EpochMillis};
use amx_proto::control::{Call, agent};

use super::{call_golden, other_pane_id, pane_id, workspace_id};

/// The instant the goldens' agents entered their current state, and the `now`
/// they are read against: 2025-08-08T11:26:40Z, four minutes apart.
///
/// Two values rather than one, because the *relationship* is the field's whole
/// contract (D-M4-4): a renderer computes `now − since` from inside one reply,
/// and a golden where the two were equal would freeze a shape in which that
/// arithmetic is invisible.
const SINCE: EpochMillis = 1_754_650_000_000;
const NOW: EpochMillis = 1_754_650_240_000;

#[test]
fn the_agent_list_golden_freezes_d15s_data_source() {
    call_golden(
        "method_agent_list",
        // Unscoped, which is what the agents view and a bare `amx agents` send.
        // The scoped shape is pinned in `additive.rs`, where the point is that
        // its absence writes no key at all.
        Call::AgentList(agent::ListParams { workspace: None }),
        agent::ListReply {
            seq: 71,
            now: NOW,
            // Queue order, head first, and global: the same order
            // `session.state` reports and `agent.next` focuses the head of,
            // never narrowed by the params above.
            attention: vec![pane_id()],
            agents: vec![
                agent::AgentEntry {
                    workspace: AgentWorkspace {
                        id: workspace_id(),
                        name: Some("api".to_owned()),
                    },
                    pane: pane_id(),
                    name: Some("backend".to_owned()),
                    kind: Some(AgentKind::new("claude").expect("a shipped agent id")),
                    status: AgentState::Blocked,
                    reason: Some("permission_dialog".to_owned()),
                    since: Some(SINCE),
                    last_line: "Allow Bash(git push origin main)? (y/n)".to_owned(),
                },
                // The second row freezes the absences beside the presences: a
                // pane nobody named, running something identity has not
                // recognised, in a workspace with no label, whose screen is
                // blank. `last_line` is the empty string and not a missing key
                // — "this pane has printed nothing" and "this build does not
                // report screen contents" must not be the same bytes.
                agent::AgentEntry {
                    workspace: AgentWorkspace::unnamed(workspace_id()),
                    pane: other_pane_id(),
                    name: None,
                    kind: None,
                    status: AgentState::Quiet,
                    reason: None,
                    since: None,
                    last_line: String::new(),
                },
            ],
        },
    );
}
