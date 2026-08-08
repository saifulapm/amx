//! Scaffolding for the T17 CLI acceptance suite: the real binary, a real
//! terminal, a real socket.
//!
//! These tests are process-level on purpose. Daemonization, the bind race and
//! terminal restoration are all properties of *processes* — a `setsid` that is
//! never execed, a socket two processes contend for, a `termios` a third
//! process has to put back — and none of them can be observed from inside one
//! test process pretending to be several.
//!
//! Four responsibilities, one file each, because this file reached the soft
//! module budget and the split it wanted was already latent in it:
//!
//! - [`env`] — an isolated machine per test: temp roots, the binary under
//!   test, running it, waiting on it.
//! - [`tty`] — a pseudoterminal and a child on it, for everything about the
//!   terminal the client borrows and gives back.
//! - [`procs`] — the process table, which is the only witness to "exactly one
//!   server survived".
//! - [`rig`] — the same machine, for a binary at an **arbitrary path**. W10
//!   wrote it inline for `update.rs` and recorded that W11 and W12 would both
//!   want it; W11 lifted it here.
//!
//! Everything is re-exported from this module, so a suite writes
//! `use support::Env` and does not care which file it came from.

#![allow(
    dead_code,
    unused_imports,
    reason = "each test binary uses a subset of the harness, and the \
              re-exports below are the whole of it"
)]
#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

pub mod env;
pub mod procs;
pub mod rig;
pub mod tty;

pub use env::{
    Env, Output, PATIENCE, SUN_BUDGET, TICK, TempDir, assert_sun_path_fits, wait_until,
    wait_until_or, window,
};
pub use procs::server_processes;
pub use rig::{Done, Rig};
pub use tty::{ALT_ENTER, ALT_LEAVE, PREFIX, Pty, Terminal, open_pty, termios_of};
