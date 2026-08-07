//! Resume: a captured ref back into a running conversation.
//!
//! 04 §5 keeps herdr's rigor wholesale (K5) — "refs validated, argv as data,
//! allowlisted sources, dedupe reservations with rollback — but the tables come
//! from the registry". D-M2-7 is the whole of it, and the parts land in three
//! places: the ref types are in
//! [`amx_core::agent::refs`](amx_core::agent::refs) (shape validation, on
//! construction *and* on deserialization), the templates and the allowed ref
//! kinds are stanza data in [`registry`](super::registry), and the planning,
//! reservation and injection are here.
//!
//! What this module owes, per D-M2-7:
//!
//! - **Source allowlisting, at all three gates.** A ref is only accepted from
//!   the hook path of the agent it claims — `source == "amx:<agent-id>"`,
//!   cross-checked against the stanza — and the check runs at report time, at
//!   snapshot-read time, and again at plan time. Three, because a `session.json`
//!   is user-editable and V15's acceptance test hand-edits one.
//! - **argv is data.** `plan()` substitutes the ref into the stanza template's
//!   single `{ref}` slot and returns a `Vec<String>`. Nothing is interpolated
//!   into a shell string, ever; quoting happens only at the injection boundary.
//! - **Dedupe reservations with rollback.** Restore reserves
//!   `source\0agent\0kind\0value` before spawning; a second pane claiming the
//!   same conversation restores as a plain shell; a failed spawn releases the
//!   reservation so a later pane can claim it.
//! - **Launch is type-in, not exec.** Restore spawns the pane's saved shell,
//!   waits (bounded by a condition, never a sleep) for the shell's first
//!   damage — the prompt painting *is* the readiness signal — then injects the
//!   planned argv through the same submit path `pane.run` uses. The pane stays
//!   a shell, so it survives the agent's eventual exit, and the user sees what
//!   was run in their own history.
//!
//! One thing the plan did not know and V01 §3 M8 measured: **the ref must be
//! taken from every `SessionStart`, not just the first.** `/clear` mints a new
//! conversation inside one process, `SessionStart.source` is the only warning,
//! and a ref captured before it resumes the wrong conversation.
//!
//! Every degraded outcome goes through the restore report. A conversation that
//! did not resume is a loss the user is told about, never a log line (04 §6).
//!
//! # Task ownership
//!
//! **V15** fills this, together with the capture side in `core/persist.rs` and
//! the restore side in `core/restore.rs`.
//!
//! V02 planted the file, froze the two additive `PaneSnapshot` fields the
//! capture writes, and implemented the ref shape validation — a validating
//! constructor left as `todo!()` would have panicked in three earlier waves.
