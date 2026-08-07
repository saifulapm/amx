//! Typed failures for the session state tree.

use thiserror::Error;

use crate::id::{PaneId, WorkspaceId};
use crate::layout::LayoutError;

/// A state operation named a workspace or pane that could not be acted on.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    /// No workspace with this id exists in the session.
    #[error("no such workspace: {0}")]
    NoSuchWorkspace(WorkspaceId),
    /// No pane with this id exists in the session's pane registry.
    #[error("no such pane: {0}")]
    NoSuchPane(PaneId),
    /// A workspace with this id is already in the session.
    ///
    /// Only [`adopt_workspace`](crate::SessionState::adopt_workspace) can
    /// produce this: every other way to make a workspace mints its own id.
    #[error("workspace {0} is already in this session")]
    WorkspaceExists(WorkspaceId),
    /// Moving a pane into a non-empty workspace named no existing target to
    /// split against.
    #[error("move needs a target pane already in the destination workspace")]
    MoveTargetRequired,
    /// The workspace's layout rejected the operation.
    #[error(transparent)]
    Layout(#[from] LayoutError),
}
