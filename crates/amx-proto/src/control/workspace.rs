//! `workspace.*` payloads.

use amx_core::{PaneId, Seq, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Parameters of `workspace.create`.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct CreateParams {
    /// Optional user-visible label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether to focus the new workspace.
    ///
    /// Honoured by the create itself, which publishes the same `FocusChanged`
    /// a `workspace.switch` would — so a caller that wants to land in what it
    /// just made says so once instead of making two calls with a window
    /// between them. Absent means `false`: a workspace created by a tool in
    /// the background does not steal the screen from whoever is at the
    /// keyboard.
    #[serde(default)]
    pub focus: bool,
    /// The git worktree this workspace belongs to, if `amx work` is the caller
    /// (D-M3-10).
    ///
    /// Additive and optional, so a caller built before `amx work` existed sends
    /// exactly the bytes it always did (R-M1-8) — and absent is the ordinary
    /// case, since every other way of making a workspace has no worktree to
    /// name.
    ///
    /// The block does two things at once, which is why it is one field and not
    /// two: it is the membership `amx work done` resolves a workspace by and
    /// restore validates, *and* it is where the new workspace's root pane
    /// starts. A workspace on a worktree whose shell opened in the server's own
    /// directory would be a workspace on a worktree in name only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<amx_core::Worktree>,
}

/// Reply to `workspace.create`.
///
/// Returns both identities: the UUID everything is keyed by, and the short
/// number a human types. The short number is display sugar and is never what
/// the client sends back.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CreateReply {
    /// The new workspace.
    pub workspace: WorkspaceId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// The bus sequence at which the workspace existed.
    pub seq: Seq,
}

/// Parameters of `workspace.rename`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RenameParams {
    /// The workspace to rename.
    pub workspace: WorkspaceId,
    /// Its new label.
    pub label: String,
}

/// Reply to `workspace.rename`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RenameReply {
    /// The bus sequence at which the rename took effect.
    pub seq: Seq,
}

/// Parameters of `workspace.kill`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KillParams {
    /// The workspace to kill, with every pane it holds.
    pub workspace: WorkspaceId,
}

/// Reply to `workspace.kill`.
///
/// Reports which panes it took with it (T16's
/// `workspace_kill_reports_which_panes_it_took_with_it`) rather than leaving
/// the caller to infer the loss from separate `pane.exited` events.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KillReply {
    /// Every pane the killed workspace took with it.
    pub panes: Vec<PaneId>,
    /// The bus sequence at which the workspace was gone.
    pub seq: Seq,
}

/// Parameters of `workspace.switch`.
///
/// Focus is workspace-scoped, not client-scoped (04 §2's `FocusChanged` event
/// carries no client id), so this call has none either.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwitchParams {
    /// The workspace to focus.
    pub workspace: WorkspaceId,
}

/// Reply to `workspace.switch`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwitchReply {
    /// The pane focused inside the now-focused workspace, if it has one.
    pub focused_pane: Option<PaneId>,
    /// The bus sequence at which the switch took effect.
    pub seq: Seq,
}
