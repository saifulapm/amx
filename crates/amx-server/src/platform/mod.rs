//! The Unix implementations behind `amx_core::platform` (04 §9).
//!
//! The seam is where amx touches the operating system, and this is the one
//! side of it that exists today. Everything Unix-shaped — descriptors, poll,
//! the process table (`/proc` on Linux, libproc on macOS) — is in here so
//! that a future port implements the same traits against the same
//! conformance tests and nothing above this module changes.

pub mod fd;
pub mod process;
pub mod pty;

pub use process::UnixProcessTree;
pub use pty::{UnixPty, UnixPtySession};
