//! The agent layer: registry, fusion, manifests, identity, addressing, resume.
//!
//! 04 §5's three tiers, as six modules. The shared vocabulary they all speak —
//! [`AgentState`](amx_core::agent::AgentState),
//! [`CoverageClass`](amx_core::agent::CoverageClass),
//! [`SessionRef`](amx_core::agent::SessionRef) — lives in `amx-core`, because
//! the client and the wire speak it too. What is here is everything that has no
//! client.
//!
//! ```text
//! tier 1  hooks     registry (what a stanza promises)  ─┐
//! tier 2  screen    manifest (what the grid says)      ─┼─→ fusion ─→ AgentHub
//! tier 3  process   identity (what is in the fg job)   ─┘
//!                                                          address, resume
//! ```
//!
//! Nothing here does I/O or owns a task. `AgentHub` (`actor/agent_hub/`, V08)
//! is the only thing that runs, and it is assembled out of these: a registry
//! parsed once, a `Tracker` per pane, a compiled manifest engine, an identity
//! probe, and the queue. Keeping the parts pure is what lets V04 property-test
//! the state machine over arbitrary interleavings and V06 test the rule engine
//! against fixture text, neither of them needing a pty.
//!
//! # Task ownership
//!
//! V02 planted this file whole and stubbed every module below it, so no two
//! wave tasks ever edit `agent/mod.rs`:
//!
//! | Module | Filled by | Wave |
//! |---|---|---|
//! | [`registry`] | V03 — the embedded `agents.toml`, parsed and override-merged | 2 |
//! | [`manifest`] | V06 — the tier-2 rule grammar and `agent.explain` | 2 |
//! | [`fusion`] | V04 — the typed, property-tested tracker | 3 |
//! | [`identity`] | V07 — the process-tree walk and wrapper unwrapping | 3 |
//! | [`address`] | V13 — UUID-then-unique-label resolution | 5 |
//! | [`resume`] | V15 — refs to argv, deduped, with rollback | 6 |

pub mod address;
pub mod fusion;
pub mod identity;
pub mod manifest;
pub mod registry;
pub mod resume;
