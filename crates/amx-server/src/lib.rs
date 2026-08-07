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

pub mod actor;
pub mod conn;
pub mod damage;
pub mod dispatch;
pub mod history;
pub mod persist;
pub mod platform;
pub mod pty;
pub mod runtime;
pub mod session;
