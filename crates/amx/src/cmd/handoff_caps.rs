//! `amx _handoff-caps` — what this binary can be handed (D-M3-6 point 2).
//!
//! **W06** fills this. One exec, JSON on stdout: `{version, handoff:
//! [min,max], proto: [min,max]}`. It exists so an exporter can refuse a wrong
//! successor *before* it quiesces a single pane — herdr validates the successor
//! after pausing everything and pays a full quiesce and rollback for a binary
//! it could have rejected for free.
//!
//! The orchestrator is its only caller and the windows it prints are the ones
//! that orchestrator checks, so the two land together rather than one guessing
//! at the other's format.
//!
//! Planted by W03 with its clap tree, so the task that fills it writes a body
//! and touches neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).

use std::process::ExitCode;

/// Run `amx _handoff-caps`.
///
/// Takes no context at all: the answer is a fact about the binary, not about
/// any session, which is exactly what makes it safe to exec before a handoff
/// has touched anything.
///
/// # Errors
///
/// Always, today: the verb is routed but not yet built, and saying so is
/// better than printing a window this build cannot honour. **W06** owes it.
pub async fn run() -> anyhow::Result<ExitCode> {
    anyhow::bail!("`amx _handoff-caps` is not wired yet; W06 owes it")
}
