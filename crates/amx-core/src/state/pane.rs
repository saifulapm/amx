//! A pane: one leaf of a workspace's layout tree.

use std::path::{Path, PathBuf};

use crate::agent::HookToken;
use crate::id::PaneId;

/// A pane's session-state metadata.
///
/// The pane's UUID is its identity for the pane's whole lifetime (`id.rs`);
/// everything here is display sugar layered on top and never consulted by
/// layout operations, which is what lets splits, closes, swaps and moves
/// leave it untouched.
#[derive(Clone, Debug)]
pub struct Pane {
    id: PaneId,
    label: Option<String>,
    cwd: Option<PathBuf>,
    argv: Option<Vec<String>>,
    hook_token: Option<HookToken>,
}

impl Pane {
    /// A freshly minted pane with no label, no recorded cwd and no process
    /// behind it yet.
    #[must_use]
    pub(crate) fn new(id: PaneId) -> Self {
        Self {
            id,
            label: None,
            cwd: None,
            argv: None,
            hook_token: None,
        }
    }

    /// This pane's stable identity.
    #[must_use]
    pub const fn id(&self) -> PaneId {
        self.id
    }

    /// The pane's user-visible label, if one was set.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Set the pane's label.
    pub(crate) fn set_label(&mut self, label: Option<String>) {
        self.label = label;
    }

    /// The directory the pane's process was started in.
    ///
    /// For a split this is the *foreground process* cwd of the source pane,
    /// not the source pane's own directory, with a defined fallback when that
    /// is unreadable (04 §7) — `foreground_cwd` as pane API state.
    #[must_use]
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Record the directory the pane's process was started in.
    pub(crate) fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = Some(cwd);
    }

    /// The argv the pane's process was spawned with, if amx spawned one.
    ///
    /// The *effective* argv: the command a `pane.split` asked for, or the one
    /// element naming the configured shell when it asked for nothing. `None`
    /// only for a pane that has state but no process — one minted through the
    /// synchronous `absorb` path, which spawns nothing.
    ///
    /// D-M2-7's "argv is data" applies here first: this is a vector of
    /// elements, never a command line, and nothing that reads it may join it
    /// with spaces and hand the result to a shell.
    #[must_use]
    pub fn argv(&self) -> Option<&[String]> {
        self.argv.as_deref()
    }

    /// The token this pane's child was spawned with, injected as
    /// `AMX_HOOK_TOKEN`.
    ///
    /// D-M2-4's misattribution guard, held here because the pane is the thing
    /// it identifies: `AgentHub` reads it to check an incoming `agent.report`,
    /// and a report whose token does not match is dropped and counted.
    ///
    /// **It is never rendered onto the wire.** `session.state`'s `PaneState` is
    /// built field by field in the server's `core/view.rs` and has no slot for
    /// it, and the persisted `PaneSnapshot` has no field for it either, so the
    /// value cannot reach a client or a file without somebody adding one on
    /// purpose.
    #[must_use]
    pub fn hook_token(&self) -> Option<&HookToken> {
        self.hook_token.as_ref()
    }

    /// Record what the pane's process was started as.
    ///
    /// The two land together because they are minted together, once, by the one
    /// spawn path every pane goes through — a pane with an argv and no token
    /// would be a pane whose hooks report into the void.
    pub(crate) fn set_spawn(&mut self, argv: Vec<String>, token: HookToken) {
        self.argv = Some(argv);
        self.hook_token = Some(token);
    }
}
