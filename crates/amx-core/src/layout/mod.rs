//! The BSP layout algebra: one tree per workspace (D13, T07).
//!
//! `docs/04-architecture.md` §7 is the input model these ops serve: navigate
//! mode's `hjkl` focus movement, `HJKL` resize, splits, swap and close all
//! reduce to the pure tree operations here. None of them touch a [`PaneId`]
//! — rearranging panes never restarts the process behind one.

mod error;
mod rect;
mod tree;

pub use error::LayoutError;
pub use rect::{Axis, Direction, Rect};
pub use tree::Layout;
