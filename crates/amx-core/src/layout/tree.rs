//! The BSP layout tree: one per workspace (D13).
//!
//! A workspace's panes tile a rectangle via a binary space partition. Every
//! internal node is a two-way split along one axis; every leaf is a pane.
//! Splits, closes, swaps and resizes are pure tree rewrites — none of them
//! touch a [`PaneId`], because rearranging panes never restarts the process
//! behind one (04 §7).

use crate::id::PaneId;
use crate::layout::error::LayoutError;
use crate::layout::rect::{Axis, Direction, Rect};

/// Resize is clamped to this band so a single `resize` call can never squeeze
/// a split fully shut; repeated calls can still approach it, which is the
/// point — resize is a lever, not a one-shot close.
const RATIO_MIN: f32 = 0.05;
const RATIO_MAX: f32 = 0.95;

/// One node of the BSP tree: a pane, or a two-way split of two subtrees.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Node {
    /// A pane occupying the whole of its slot.
    Leaf(PaneId),
    /// Two subtrees sharing a rect, divided along `axis` at `ratio`.
    Split {
        axis: Axis,
        /// Share of the divided dimension given to `first`, in `0.0..=1.0`.
        ratio: f32,
        /// The top/left subtree.
        first: Box<Node>,
        /// The bottom/right subtree.
        second: Box<Node>,
    },
}

/// A workspace's BSP layout: the pane tree plus the zoom projection over it.
///
/// Zoom is deliberately not a tree mutation (T07 acceptance): zooming a pane
/// records it in `zoomed` and [`rects`](Self::rects) special-cases it, so
/// unzooming restores the prior geometry exactly because the tree underneath
/// was never touched.
/// Serialized because `session.state` carries the tree to clients verbatim —
/// the mirror the client renders is the same algebra, so the wire form is the
/// tree itself rather than a projection of it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    root: Option<Node>,
    zoomed: Option<PaneId>,
}

/// Outcome of removing a pane from a subtree.
enum Removed {
    /// The subtree was exactly the leaf removed; nothing takes its place.
    Gone,
    /// The subtree's sibling collapses into the parent slot (the target was
    /// found one level down, in a two-leaf split).
    Collapsed(Node),
    /// The target was not found in this subtree; it is returned unchanged.
    Unchanged(Node),
}

impl Layout {
    /// An empty layout: no panes, nothing to render.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A layout containing exactly one pane, filling the whole workspace.
    #[must_use]
    pub fn with_root(pane: PaneId) -> Self {
        Self {
            root: Some(Node::Leaf(pane)),
            zoomed: None,
        }
    }

    /// Whether the layout has no panes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Whether `pane` is a leaf of this layout.
    #[must_use]
    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.as_ref().is_some_and(|root| contains(root, pane))
    }

    /// Every pane in the layout, in tree order.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect_panes(root, &mut out);
        }
        out
    }

    /// The pane currently zoomed, if any.
    #[must_use]
    pub fn zoomed(&self) -> Option<PaneId> {
        self.zoomed
    }

    /// Split `target`, giving `new_pane` the slot `dir` names.
    ///
    /// `ratio` (clamped to `0.0..=1.0`) is `target`'s share of the divided
    /// dimension after the split. Fails if `target` is not in the layout, if
    /// `new_pane` already is, or if the two are the same pane.
    pub fn split(
        &mut self,
        target: PaneId,
        dir: Direction,
        new_pane: PaneId,
        ratio: f32,
    ) -> Result<(), LayoutError> {
        if target == new_pane {
            return Err(LayoutError::SamePane(target));
        }
        if self.contains(new_pane) {
            return Err(LayoutError::AlreadyPresent(new_pane));
        }
        let root = self.root.take().ok_or(LayoutError::NoSuchPane(target))?;
        let (new_root, found) = insert(root, target, new_pane, dir, ratio);
        self.root = Some(new_root);
        if !found {
            return Err(LayoutError::NoSuchPane(target));
        }
        Ok(())
    }

    /// Remove `pane`, collapsing its sibling into the parent slot.
    ///
    /// If `pane` was the whole layout, the layout becomes empty. If `pane`
    /// was zoomed, the zoom is cleared along with it.
    pub fn close(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        let root = self.root.take().ok_or(LayoutError::NoSuchPane(pane))?;
        match remove(root, pane) {
            Removed::Gone => self.root = None,
            Removed::Collapsed(node) => self.root = Some(node),
            Removed::Unchanged(node) => {
                self.root = Some(node);
                return Err(LayoutError::NoSuchPane(pane));
            }
        }
        if self.zoomed == Some(pane) {
            self.zoomed = None;
        }
        Ok(())
    }

    /// Exchange the positions of `a` and `b` without touching tree shape.
    ///
    /// Neither pane's identity changes — only which leaf slot each occupies —
    /// so `a` and `b` keep whatever zoom state they individually had.
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> Result<(), LayoutError> {
        if a == b {
            return Err(LayoutError::SamePane(a));
        }
        if !self.contains(a) {
            return Err(LayoutError::NoSuchPane(a));
        }
        if !self.contains(b) {
            return Err(LayoutError::NoSuchPane(b));
        }
        if let Some(root) = &mut self.root {
            swap_leaves(root, a, b);
        }
        if self.zoomed == Some(a) {
            self.zoomed = Some(b);
        } else if self.zoomed == Some(b) {
            self.zoomed = Some(a);
        }
        Ok(())
    }

    /// Nudge the ratio of the split immediately containing `target` by
    /// `delta`, clamped to `[RATIO_MIN, RATIO_MAX]`.
    ///
    /// A positive `delta` grows `target`'s slot: the split's own ratio moves
    /// toward giving `target` more of the divided dimension, whether `target`
    /// is the first or second child. Returns whether a split was found to
    /// resize — `target` being the workspace's sole pane, with no parent
    /// split, is a legal no-op rather than an error.
    pub fn resize(&mut self, target: PaneId, delta: f32) -> Result<bool, LayoutError> {
        if !self.contains(target) {
            return Err(LayoutError::NoSuchPane(target));
        }
        let Some(root) = &mut self.root else {
            return Ok(false);
        };
        Ok(resize_in(root, target, delta))
    }

    /// Project `pane` to fill the whole workspace, without touching the tree.
    pub fn zoom(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if !self.contains(pane) {
            return Err(LayoutError::NoSuchPane(pane));
        }
        self.zoomed = Some(pane);
        Ok(())
    }

    /// Clear the zoom projection, restoring the tree's real geometry.
    pub fn unzoom(&mut self) {
        self.zoomed = None;
    }

    /// The concrete rect of every pane within `area`.
    ///
    /// While a pane is zoomed this returns that one pane at the full `area`
    /// and nothing else — the projection T07's acceptance requires. Every
    /// other pane's rect in the tree is unchanged underneath; unzooming makes
    /// this method see the tree again, exactly as it was.
    #[must_use]
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        if let Some(zoomed) = self.zoomed {
            return vec![(zoomed, area)];
        }
        self.rects_ignoring_zoom(area)
    }

    /// The real tiling, regardless of zoom — what focus movement navigates.
    fn rects_ignoring_zoom(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect_rects(root, area, &mut out);
        }
        out
    }

    /// The geometric neighbour of `from` in direction `dir`, if one exists.
    ///
    /// Computed over the real tiling (zoom never changes which pane `hjkl`
    /// would reach): the nearest rect whose near edge lies in `dir` from
    /// `from`'s, breaking ties by the smallest centre offset on the other
    /// axis. This is the standard tiling-window-manager neighbour rule.
    #[must_use]
    pub fn neighbour(&self, area: Rect, from: PaneId, dir: Direction) -> Option<PaneId> {
        let rects = self.rects_ignoring_zoom(area);
        let current = rects.iter().find(|(id, _)| *id == from)?.1;
        let (cx, cy) = current.center();

        let mut best: Option<(PaneId, i32, i32)> = None;
        for (id, r) in &rects {
            if *id == from {
                continue;
            }
            let in_direction = match dir {
                Direction::Left => r.x + r.w <= current.x,
                Direction::Right => r.x >= current.x + current.w,
                Direction::Up => r.y + r.h <= current.y,
                Direction::Down => r.y >= current.y + current.h,
            };
            if !in_direction {
                continue;
            }
            let gap = match dir {
                Direction::Left => cx - i32::from(r.x + r.w),
                Direction::Right => i32::from(r.x) - (cx + i32::from(current.w) / 2),
                Direction::Up => cy - i32::from(r.y + r.h),
                Direction::Down => i32::from(r.y) - (cy + i32::from(current.h) / 2),
            };
            let (rcx, rcy) = r.center();
            let cross = match dir {
                Direction::Left | Direction::Right => (rcy - cy).abs(),
                Direction::Up | Direction::Down => (rcx - cx).abs(),
            };
            if best.is_none_or(|(_, bg, bc)| (gap, cross) < (bg, bc)) {
                best = Some((*id, gap, cross));
            }
        }
        best.map(|(id, ..)| id)
    }
}

fn contains(node: &Node, pane: PaneId) -> bool {
    match node {
        Node::Leaf(id) => *id == pane,
        Node::Split { first, second, .. } => contains(first, pane) || contains(second, pane),
    }
}

fn collect_panes(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { first, second, .. } => {
            collect_panes(first, out);
            collect_panes(second, out);
        }
    }
}

fn collect_rects(node: &Node, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, area)),
        Node::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (a, b) = area.split(*axis, *ratio);
            collect_rects(first, a, out);
            collect_rects(second, b, out);
        }
    }
}

fn insert(
    node: Node,
    target: PaneId,
    new_pane: PaneId,
    dir: Direction,
    ratio: f32,
) -> (Node, bool) {
    match node {
        Node::Leaf(id) if id == target => {
            let existing = Node::Leaf(id);
            let incoming = Node::Leaf(new_pane);
            let (first, second) = if dir.new_pane_is_first() {
                (incoming, existing)
            } else {
                (existing, incoming)
            };
            let split = Node::Split {
                axis: dir.axis(),
                ratio: ratio.clamp(0.0, 1.0),
                first: Box::new(first),
                second: Box::new(second),
            };
            (split, true)
        }
        leaf @ Node::Leaf(_) => (leaf, false),
        Node::Split {
            axis,
            ratio: r,
            first,
            second,
        } => {
            let (new_first, found) = insert(*first, target, new_pane, dir, ratio);
            if found {
                (
                    Node::Split {
                        axis,
                        ratio: r,
                        first: Box::new(new_first),
                        second,
                    },
                    true,
                )
            } else {
                let (new_second, found) = insert(*second, target, new_pane, dir, ratio);
                (
                    Node::Split {
                        axis,
                        ratio: r,
                        first: Box::new(new_first),
                        second: Box::new(new_second),
                    },
                    found,
                )
            }
        }
    }
}

fn remove(node: Node, target: PaneId) -> Removed {
    match node {
        Node::Leaf(id) if id == target => Removed::Gone,
        leaf @ Node::Leaf(_) => Removed::Unchanged(leaf),
        Node::Split {
            axis,
            ratio,
            first,
            second,
        } => match remove(*first, target) {
            Removed::Gone => Removed::Collapsed(*second),
            Removed::Collapsed(new_first) => Removed::Collapsed(Node::Split {
                axis,
                ratio,
                first: Box::new(new_first),
                second,
            }),
            Removed::Unchanged(first) => match remove(*second, target) {
                Removed::Gone => Removed::Collapsed(first),
                Removed::Collapsed(new_second) => Removed::Collapsed(Node::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(new_second),
                }),
                Removed::Unchanged(second) => Removed::Unchanged(Node::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
            },
        },
    }
}

fn swap_leaves(node: &mut Node, a: PaneId, b: PaneId) {
    match node {
        Node::Leaf(id) if *id == a => *id = b,
        Node::Leaf(id) if *id == b => *id = a,
        Node::Leaf(_) => {}
        Node::Split { first, second, .. } => {
            swap_leaves(first, a, b);
            swap_leaves(second, a, b);
        }
    }
}

/// Whether `target` is a leaf directly under `node` (not further nested).
fn has_leaf(node: &Node, target: PaneId) -> bool {
    matches!(node, Node::Leaf(id) if *id == target)
}

fn resize_in(node: &mut Node, target: PaneId, delta: f32) -> bool {
    let Node::Split {
        ratio,
        first,
        second,
        ..
    } = node
    else {
        return false;
    };
    if has_leaf(first, target) {
        *ratio = (*ratio + delta).clamp(RATIO_MIN, RATIO_MAX);
        true
    } else if contains(first, target) {
        resize_in(first, target, delta)
    } else if has_leaf(second, target) {
        *ratio = (*ratio - delta).clamp(RATIO_MIN, RATIO_MAX);
        true
    } else if contains(second, target) {
        resize_in(second, target, delta)
    } else {
        false
    }
}
