//! The amx session server.
//!
//! 04 §2: "The server is a set of **tokio actors with typed mailboxes**,
//! supervised by a root task with `CancellationToken` + `JoinSet` (structured
//! shutdown; nothing detached, everything joined)."
//!
//! M0 built three of the five actors in that table — `Core`, `PaneHost` and
//! `Gateway`. M1 adds the fourth, `Persist`: [`persist`] is the on-disk format
//! it writes, and its mailbox joins the vocabulary in [`actor`]. `AgentHub`
//! arrives with the agent layer.
//!
//! The user's settings reach those actors through [`config_rt`]: the file is
//! read at startup, watched by [`platform::watch`], and published on a channel
//! every actor can read the current value from.

pub mod actor;
pub mod config_rt;
pub mod conn;
pub mod damage;
pub mod dispatch;
pub mod history;
pub mod persist;
pub mod platform;
pub mod pty;
pub mod runtime;
pub mod session;
