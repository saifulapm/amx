//! Live binary-upgrade handoff: the successor takes the panes, not a copy.
//!
//! 04 §6, K3 kept whole: "quiesce PTY actors, pass fds + manifest over a
//! token-authenticated socket, staged ready/restored/committed/owned handshake,
//! strict abort on partial import." `docs/09-m3-plan.md` §3 is the protocol
//! with per-step ownership and the crash table; D-M3-3 through D-M3-6 are why
//! each piece is shaped the way it is.
//!
//! The two facts the whole design leans on, from unix(7): an SCM_RIGHTS
//! transfer is "a reference to an open file description … semantically
//! equivalent to `dup(2)` into the file descriptor table of another process".
//! So the importer's descriptor and the exporter's are the *same* description —
//! same position, same flags — and the pty is torn down only when the last
//! reference closes. The exporter's ordinary shutdown after commit therefore
//! costs the child nothing, and no `preserve_for_handoff` contortion is needed.
//!
//! # Task ownership
//!
//! W03 plants this tree whole so that no two wave-2 tasks share a file
//! (`docs/09-m3-plan.md` §6). Each submodule below is an empty shell with the
//! task that fills it named on it, and neither of those tasks edits this file.
//!
//! | Module | Carries | Owed by |
//! |---|---|---|
//! | [`manifest`] | what crosses, as bytes | **W04** |
//! | [`grid`] | the styled screen, as a replay | **W04** |
//! | [`fd`] | the descriptor transfer | **W05** |
//! | [`protocol`] | the staged commit, as two state machines | **W05** |
//!
//! `export.rs` (the orchestrator) is **W06**'s and lands as its own file; the
//! import half is an alternate session assembly and lives in `session/`, which
//! is **W07**'s.

pub mod export;
pub mod fd;
pub mod grid;
pub mod manifest;
pub mod protocol;
