//! `client.*` payloads: what a client declares about itself.
//!
//! 04 §3 splits authority: the server owns truth, the client owns presentation.
//! These types are that split written down — a client *declares* its viewport
//! and its keybinding authority; it does not ask the server to render for it.

use amx_core::PaneId;
use serde::{Deserialize, Serialize};

/// The panes a client can currently see, and at what size.
///
/// Viewport visibility is "defined by the client's own layout projection"
/// (04 §3): the client computes its chrome locally and tells the server which
/// panes it therefore needs deltas for. The server sends grid traffic for these
/// panes and no others.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Viewport {
    /// The client terminal's rows.
    pub rows: u16,
    /// The client terminal's columns.
    pub cols: u16,
    /// Panes visible in this client's projection.
    #[serde(default)]
    pub panes: Vec<PaneId>,
}

/// Where a client's keybindings come from.
///
/// A client may bring its own at handshake; chrome is drawn by the client in
/// its own theme, so its bindings are its own business too.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keybindings {
    /// Use the session's bindings.
    #[default]
    Server,
    /// The client interprets its own bindings and sends resulting calls.
    Local,
}

/// Whether this client's size drives the panes' PTY size.
///
/// The pane grid follows the most-recently-active client; every other client
/// letterboxes or clips the server-sized grid inside its own chrome rather than
/// forcing a resize (04 §3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeAuthority {
    /// This client's size sets pane grid sizes while it is the active one.
    #[default]
    Active,
    /// This client never drives pane sizes.
    Observer,
}
