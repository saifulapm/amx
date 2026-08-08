//! `amx _handoff-caps` — what this binary can be handed (D-M3-6 point 2).
//!
//! One exec, JSON on stdout: `{version, handoff: [min,max], proto: [min,max]}`.
//! It exists so an exporter can refuse a wrong successor *before* it quiesces a
//! single pane — herdr validates the successor after pausing everything and
//! pays a full quiesce and rollback for a binary it could have rejected for
//! free.
//!
//! The document is [`Caps`] itself, and the windows it prints are the ones the
//! orchestrator checks, because the orchestrator reads this same type back.
//! There is no second spelling of the format to drift from the first.
//!
//! Planted by W03 with its clap tree, so this task writes a body and touches
//! neither `cli.rs` nor `cmd/mod.rs` (`docs/09-m3-plan.md` §6).

use std::process::ExitCode;

use amx_server::handoff::export::Caps;

/// Run `amx _handoff-caps`.
///
/// Takes no context at all: the answer is a fact about the binary, not about
/// any session, which is exactly what makes it safe to exec before a handoff
/// has touched anything. It reads no configuration, opens no socket and starts
/// no runtime work, so a staged binary that cannot serve a session still
/// answers this honestly.
///
/// # Errors
///
/// Only if the document cannot be written to standard output, which is a pipe
/// the exporter opened and is reading.
pub async fn run() -> anyhow::Result<ExitCode> {
    let caps = Caps::of_this_build();
    println!("{}", serde_json::to_string(&caps)?);
    Ok(ExitCode::SUCCESS)
}
