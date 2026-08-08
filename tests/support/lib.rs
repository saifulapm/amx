//! The T18 harness: a real `amx` binary, a real socket, a real terminal.
//!
//! Everything here exists to let a suite say "spawn the server this session's
//! user would get, and talk to it the way their client would" without owning
//! any of the machinery it is testing. Three layers:
//!
//! - [`env`] — an isolated machine per test: temp runtime and state roots,
//!   the binary under test, process-level helpers. No test touches the
//!   process environment; every root travels on the spawned command.
//! - [`agent`] — scripted stand-in agents for M2's exit suite, planted through
//!   the registry override the design calls the test seam. Its module docs say
//!   which parts of an agent are real there and which stand in.
//! - [`wire`] — a raw protocol client over the session socket. It speaks
//!   frames, not a client API, so a suite can send half a frame or a method
//!   from the future.
//! - [`term`] and [`screen`] — a pseudoterminal to run the real client on,
//!   and a rasterizer to compare what two clients painted.
//! - [`golden`] — the shared golden-file mechanic, same directory tree and
//!   same `AMX_UPDATE_GOLDENS=1` regeneration path T04 established.
//!
//! Waiting is always a condition plus a deadline ([`env::wait_until`],
//! [`term::Terminal::wait_for`], [`wire::Wire::request`]'s read timeout) —
//! never a bare nap whose expiry is load-bearing. `hygiene.rs` holds the
//! suites to that.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "test harness: failing loudly is the product"
)]

pub mod agent;
pub mod env;
pub mod golden;
pub mod platform;
pub mod screen;
pub mod server;
pub mod term;
pub mod wire;

pub use agent::FakeAgents;
pub use env::{
    Env, Output, PATIENCE, TICK, TempDir, pids_with_arg, tick, wait_until, wait_until_or,
};
pub use golden::{check_bytes_golden, check_json_golden, goldens_dir};
pub use screen::{Screen, StyledScreen, rasterize, rasterize_styled, shows, styled_run};
pub use server::ServerChild;
pub use term::{ALT_ENTER, ALT_LEAVE, PREFIX, Terminal};
pub use wire::{Wire, read_frame, result_of};
