//! Typed failures for layout operations.

use thiserror::Error;

use crate::id::PaneId;

/// A layout operation was asked to act on a tree it could not.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LayoutError {
    /// The named pane is not a leaf of this layout.
    #[error("pane {0} is not in this layout")]
    NoSuchPane(PaneId),
    /// A split, swap or move named the same pane on both sides.
    #[error("pane {0} cannot be paired with itself")]
    SamePane(PaneId),
    /// A split target already exists in the tree.
    #[error("pane {0} is already in this layout")]
    AlreadyPresent(PaneId),
    /// An operation that requires at least one pane found an empty layout.
    #[error("layout is empty")]
    Empty,
}
