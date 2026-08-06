//! The amx session server.
//!
//! 04 §2: "The server is a set of **tokio actors with typed mailboxes**,
//! supervised by a root task with `CancellationToken` + `JoinSet` (structured
//! shutdown; nothing detached, everything joined)."
//!
//! M0 builds three of the five actors in that table — `Core`, `PaneHost` and
//! `Gateway`; `Persist` and `AgentHub` arrive with persistence and the agent
//! layer. What is fixed here is the mailbox vocabulary those actors speak.

pub mod actor;
pub mod conn;
pub mod damage;
pub mod dispatch;
pub mod history;
pub mod platform;
pub mod pty;
pub mod runtime;
pub mod session;
