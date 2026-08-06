//! Session lifecycle: find one, start one, list them, stop them (04 §1).
//!
//! "One socket per session (`$XDG_RUNTIME_DIR/amx/<session>/sock`, 0600) ...
//! Stale-socket disambiguation by connect probe ... Named sessions with a
//! lifecycle, as in herdr: `--session work` / `AMX_SESSION` select a named
//! server instance (socket + state under its own directory); `amx session
//! list|attach|stop|delete` enumerate and manage running servers."
//!
//! Everything in this module answers one of four questions, and each has
//! exactly one answer here so the binary never invents a second:
//!
//! - *is this session running?* — [`probe`], a connect probe and nothing else.
//!   No lock file, no pid file: the only evidence that a server is alive is a
//!   server answering (herdr's lock-free single-instance trick, K9).
//! - *how do I start one?* — [`daemon`], which is `setsid` + null stdio +
//!   readiness polling, and [`serve`], which is what the daemonized process
//!   then runs.
//! - *which ones exist?* — [`registry`], which walks the runtime root and
//!   probes each session directory it finds.
//! - *how do I stop one?* — [`registry::stop`], which asks the kernel who is
//!   on the other end of the socket and signals that process.
//!
//! Nothing here reads the process environment. Paths come from an
//! [`Env`](amx_core::Env) the caller built and a [`Ctx`](amx_core::Ctx)
//! derived from it, which is what lets one test process run two fully isolated
//! sessions with no mutex (04 §2, fixes W9).

pub mod daemon;
pub mod probe;
pub mod registry;
pub mod serve;

pub use daemon::{DaemonError, READY_POLL, READY_TIMEOUT, await_ready, spawn_detached};
pub use probe::{Probe, ProbeError, clear_if_stale, probe};
pub use registry::{DeleteError, ListError, Running, StopError, StopOutcome, runtime_root};
pub use serve::{CORE_MAILBOX, ServeError, ServeReport, StopOn, serve};
