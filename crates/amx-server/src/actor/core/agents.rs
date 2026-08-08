//! `agent.list`: the narrow, agent-only projection of the session
//! (`docs/10-attention-surfaces.md` §D15).
//!
//! Answered here, out of the state `Core` already holds plus the panes' own
//! published frames — one mailbox round trip for the whole reply, however many
//! panes there are (11-m4-plan D-M4-2). The alternative, a `StreamCall::Wiring`
//! per pane, is one round trip *each*, which at twenty-five agents is
//! twenty-five.
//!
//! Empty on purpose. **X10** fills it; the module is planted here so that
//! `actor/core/mod.rs` — whose short-number fields X05 rewrote in the same wave
//! — is edited by one task and not two.
