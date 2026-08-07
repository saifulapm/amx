//! V17: M2's seams, and the milestone's exit criterion, over the real binary.
//!
//! Two jobs in one suite, for the reason `seams.rs` states for M1: exclusive
//! file ownership is what makes waves parallel, and it is also why nobody owns
//! the places tasks meet. M2's seams are wider than M1's — five subsystems meet
//! in `AgentHub` — and the exit criterion spans all of them, so the same rig
//! proves both.
//!
//! Every test here drives a **real `amx server` process over its real socket**,
//! with a real client on a real pseudoterminal wherever a person would be
//! looking at one. That is not ceremony. `crates/amx-server/tests/` assembles a
//! session in-process and is green; it was green while the hub's mailbox and
//! its `StatusView` stopped at the gateway and never reached a connection, so
//! every `agent.*` row and every wait answered a real client with a refusal.
//! One process' distance is what makes the difference visible. (The standing
//! lesson, twice paid for: a green suite has hidden a non-working feature.)
//!
//! # The seams, one module each
//!
//! - [`edges`] — hook → hub → fusion → status, over the wire, through the four
//!   silences V01 measured.
//! - [`attention`] — the queue → `agent.next` → the client's one-key cycle and
//!   its `⚑N`.
//! - [`explain`] — registry → spawn identity → manifest → tier-2 status →
//!   `agent explain`, and the waits that ride the same read model.
//! - [`resume`] — hook-reported refs → the mirror → the snapshot → a restarted
//!   session typing every conversation back.
//!
//! **Where the emitter is isolated.** Most transitions here have two witnesses
//! — the scripted agent emits a hook *and* paints the matching screen, which is
//! what a real agent does — so a status reaching `working` proves only that one
//! of the two tiers worked. The `session_ref` is the exception and therefore
//! the load-bearing assertion: a conversation id can only have come from a hook
//! payload, through the shipped `amx _hook` binary, over the pane's own socket,
//! into the hub, out through `Core`'s mirror and onto the wire. Every test that
//! asserts one has proven that whole path end to end.
//!
//! # The exit test
//!
//! The scripted pass at the bottom of this file is `docs/05-roadmap.md`'s M2
//! criterion, in one session: five named agents, statuses correct through the
//! spike's measured edge cases, one key cycling the blocked set, and a restart
//! that resumes every conversation.
//!
//! # What stands in
//!
//! The agents. CI runners have no agent tooling (R-M2-8b), so they are POSIX-sh
//! scripts — and `tests/support/agent.rs` documents, layer by layer, which
//! parts of them are the real thing: the hook payload shapes are V01's
//! recordings field for field, the transport is the shipped `amx _hook`, the
//! screen rules are the shipped `claude.toml` rather than a manifest written to
//! be passed, and the registry stanza arrives through the override path the
//! design nominates as the test seam. What is scripted is the process in the
//! pane and the exact pixels of its screens. The other half of the criterion is
//! the by-hand live smoke against real Claude Code, recorded with its date and
//! versions in `docs/notes/hook-coverage.md` §11 — green tests alone do not
//! exit this milestone.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

// A test crate root's module directory is the package root, so each module of
// this suite needs its path spelled out to live beside it.
#[path = "agents/attention.rs"]
mod attention;
#[path = "agents/edges.rs"]
mod edges;
#[path = "agents/explain.rs"]
mod explain;
#[path = "agents/fixtures.rs"]
mod fixtures;
#[path = "agents/resume.rs"]
mod resume;

use rig::agent;

use fixtures::{NAMES, Rig};

/// The M2 exit criterion, in one session.
///
/// Five named agents; status correct through every edge case the spike
/// measured; one key cycling the blocked set in block order; a restart that
/// brings every conversation back. The modules above prove each seam on its
/// own — this proves they compose, which is the thing a milestone exits on.
#[tokio::test]
async fn five_named_fake_agents_report_correct_status_through_the_spike_edge_cases() {
    let mut rig = Rig::start("exit").await;

    // ------------------------------------------------------- five named agents
    let mut agents = Vec::new();
    for name in NAMES {
        agents.push(rig.start_agent(name).await);
    }
    for a in &agents {
        assert_eq!(
            rig.status(a.pane).await.as_deref(),
            Some("idle"),
            "{} came up ready but does not read idle",
            a.name
        );
        let entry = rig.pane_state(a.pane).await;
        assert_eq!(entry["label"], a.name, "the agent's name is its pane label");
        assert_eq!(entry["agent"]["kind"], agent::KIND, "{entry}");
    }

    // --------------------------------------------- a clean turn, hook-driven
    rig.drive(&agents[0].name, agent::PROMPT).await;
    rig.wait_status(agents[0].pane, "working").await;
    rig.drive(&agents[0].name, agent::STOP).await;
    rig.wait_status(agents[0].pane, "idle").await;

    // ----------------------------------- V01 §7.1: Esc during generation, silent
    rig.drive(&agents[1].name, agent::PROMPT).await;
    rig.wait_status(agents[1].pane, "working").await;
    rig.drive(&agents[1].name, agent::ESC).await;
    rig.wait_status(agents[1].pane, "idle").await;

    // ------------------------------------ V01 §7.2: Esc during a tool call, silent
    rig.drive(&agents[2].name, agent::TOOL).await;
    rig.wait_status(agents[2].pane, "working").await;
    rig.drive(&agents[2].name, agent::ESC).await;
    rig.wait_status(agents[2].pane, "idle").await;

    // ------------------- V01 §7.3–4: the dialog, answered "No" or cancelled with
    // Esc — indistinguishable, and both silent. Two agents block, in order.
    rig.block(&agents[3]).await;
    rig.block(&agents[4]).await;
    assert_eq!(
        rig.attention().await,
        vec![agents[3].pane, agents[4].pane],
        "the queue is in block order (D-M2-8)"
    );

    // ------------------------------------------- one key cycles the blocked set
    let head = rig.call("agent.next", serde_json::json!({})).await;
    assert_eq!(
        head["pane"],
        agents[3].pane.to_string(),
        "the head of the queue is the pane that blocked first: {head}"
    );
    assert_eq!(head["waiting"], 2, "{head}");
    assert_eq!(
        head["workspace"],
        agents[3].workspace.to_string(),
        "the head is in another workspace, and the reply says which: {head}"
    );

    // --------------------------------- V01 §7.5: the anonymous late SubagentStop
    // after a parent turn. It reaches the hub and changes nothing.
    rig.drive(&agents[0].name, agent::LATE_SUBAGENT).await;
    let settled = rig.transition_seq(agents[0].pane).await;
    assert_eq!(rig.status(agents[0].pane).await.as_deref(), Some("idle"));
    assert_eq!(
        rig.transition_seq(agents[0].pane).await,
        settled,
        "a subagent event moved the parent's status"
    );

    // ------------------------------ the dialogs are answered, and the queue empties
    for a in &agents[3..] {
        rig.drive(&a.name, agent::CANCEL).await;
        rig.wait_status(a.pane, "idle").await;
    }
    assert!(rig.attention().await.is_empty(), "the queue emptied");

    // ---------------------------------------------- every conversation is known
    let state = rig.state().await;
    for a in &agents {
        let entry = fixtures::pane_entry(&state, a.pane);
        assert_eq!(
            entry["agent"]["session_ref"]["value"],
            a.conversation(),
            "{}'s conversation reached the wire: {entry}",
            a.name
        );
    }

    // -------------------------------- the snapshot has it too, then the restart
    let wanted: Vec<String> = agents.iter().map(fixtures::Agent::conversation).collect();
    rig.wait_snapshot_holds(&wanted).await;
    rig.shutdown_within(rig::PATIENCE);
    rig.restart().await;

    let mut resumed = rig.wait_resumed(wanted.len()).await;
    resumed.sort();
    let mut expected = wanted;
    expected.sort();
    assert_eq!(
        resumed, expected,
        "every one of the five conversations was typed back into its restored pane"
    );
    rig.shutdown_within(rig::PATIENCE);
}
