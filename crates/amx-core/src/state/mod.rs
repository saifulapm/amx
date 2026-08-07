//! Session state: workspaces → panes, one BSP layout tree each (D13, T07).
//!
//! `Core` (T09) owns one [`SessionState`] per session; everything here is a
//! pure data model with no I/O and no actor plumbing of its own. Every
//! mutating method returns an [`Effect`](crate::effect::Effect) describing
//! what it changed, per the structural-dirtiness contract in 04 §2.

mod error;
mod pane;
mod session;
mod workspace;

pub use error::StateError;
pub use pane::Pane;
pub use session::SessionState;
pub use workspace::{Workspace, Worktree};
