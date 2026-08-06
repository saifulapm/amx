//! The amx client.
//!
//! 04 §3: "**Client owns presentation**: it receives the layout tree + status
//! model as state (not pixels), draws its own chrome (borders, status line,
//! picker), and receives **per-pane grid deltas** (damage rectangles of cells,
//! cursor state) only for panes visible in *its* viewport."
//!
//! Everything the client does follows from that division. It never renders
//! another client's view, never asks the server what its chrome should look
//! like, and never round-trips to scroll: history is cached locally by stable
//! row id and scrolled at memory speed.

#![forbid(unsafe_code)]

pub mod app;
pub mod cache;
pub mod input;
pub mod model;
pub mod net;
pub mod render;
pub mod term;
