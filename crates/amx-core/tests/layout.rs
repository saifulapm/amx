//! Acceptance tests for the BSP layout algebra (T07).

mod support;

use amx_core::{Direction, Layout, PaneId, Rect};
use proptest::prelude::*;

use support::assert_tiles_exactly;

const AREA: Rect = Rect::new(0, 0, 80, 24);

#[test]
fn split_produces_two_panes_whose_rects_tile_the_parent_exactly() {
    let root = PaneId::new_v4();
    let mut layout = Layout::with_root(root);
    let new_pane = PaneId::new_v4();
    layout.split(root, Direction::Right, new_pane, 0.5).unwrap();

    let rects = layout.rects(AREA);
    assert_eq!(rects.len(), 2);
    assert_tiles_exactly(&rects, AREA);

    let ids: Vec<_> = rects.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&root));
    assert!(ids.contains(&new_pane));
}

#[test]
fn close_collapses_the_sibling_into_the_parent_slot() {
    let root = PaneId::new_v4();
    let mut layout = Layout::with_root(root);
    let child = PaneId::new_v4();
    layout.split(root, Direction::Right, child, 0.5).unwrap();
    let grandchild = PaneId::new_v4();
    layout
        .split(child, Direction::Down, grandchild, 0.5)
        .unwrap();

    layout.close(child).unwrap();

    let rects = layout.rects(AREA);
    assert_eq!(rects.len(), 2);
    let ids: Vec<_> = rects.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&root));
    assert!(ids.contains(&grandchild));
    assert!(!ids.contains(&child));

    // `grandchild` takes over the whole slot that the (child, grandchild)
    // split used to occupy — the right half of the workspace — not just the
    // smaller share it had as grandchild before the close.
    let grandchild_rect = rects.iter().find(|(id, _)| *id == grandchild).unwrap().1;
    assert_eq!(grandchild_rect, Rect::new(40, 0, 40, 24));
}

#[test]
fn swap_exchanges_panes_without_touching_pane_identity() {
    let root = PaneId::new_v4();
    let mut layout = Layout::with_root(root);
    let right = PaneId::new_v4();
    layout.split(root, Direction::Right, right, 0.5).unwrap();

    let before = layout.rects(AREA);
    let root_rect_before = before.iter().find(|(id, _)| *id == root).unwrap().1;
    let right_rect_before = before.iter().find(|(id, _)| *id == right).unwrap().1;

    layout.swap(root, right).unwrap();

    let after = layout.rects(AREA);
    assert_eq!(after.len(), 2);
    let root_rect_after = after.iter().find(|(id, _)| *id == root).unwrap().1;
    let right_rect_after = after.iter().find(|(id, _)| *id == right).unwrap().1;

    // Identity: both UUIDs are still exactly the panes in the layout.
    let mut ids_before: Vec<_> = before.iter().map(|(id, _)| *id).collect();
    let mut ids_after: Vec<_> = after.iter().map(|(id, _)| *id).collect();
    ids_before.sort();
    ids_after.sort();
    assert_eq!(ids_before, ids_after);

    // Position: the two panes traded rects.
    assert_eq!(root_rect_after, right_rect_before);
    assert_eq!(right_rect_after, root_rect_before);
}

#[test]
fn zoom_projects_one_pane_to_the_full_workspace_and_restores_exactly() {
    let root = PaneId::new_v4();
    let mut layout = Layout::with_root(root);
    let right = PaneId::new_v4();
    layout.split(root, Direction::Right, right, 0.5).unwrap();

    let before = layout.rects(AREA);

    layout.zoom(root).unwrap();
    assert_eq!(layout.rects(AREA), vec![(root, AREA)]);

    layout.unzoom();
    assert_eq!(layout.rects(AREA), before);
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Split {
        target: usize,
        new: usize,
        dir: Direction,
        ratio: f32,
    },
    Close {
        target: usize,
    },
    Swap {
        a: usize,
        b: usize,
    },
    Resize {
        target: usize,
        delta: f32,
    },
    Zoom {
        target: usize,
    },
    Unzoom,
}

const POOL_SIZE: usize = 6;

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Left),
        Just(Direction::Down),
        Just(Direction::Up),
        Just(Direction::Right),
    ]
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let idx = 0..POOL_SIZE;
    prop_oneof![
        (idx.clone(), idx.clone(), direction_strategy(), 0.0f32..1.0).prop_map(
            |(target, new, dir, ratio)| Op::Split {
                target,
                new,
                dir,
                ratio
            }
        ),
        idx.clone().prop_map(|target| Op::Close { target }),
        (idx.clone(), idx.clone()).prop_map(|(a, b)| Op::Swap { a, b }),
        (idx.clone(), -1.0f32..1.0).prop_map(|(target, delta)| Op::Resize { target, delta }),
        idx.clone().prop_map(|target| Op::Zoom { target }),
        Just(Op::Unzoom),
    ]
}

proptest! {
    /// Applies arbitrary op sequences (including ones whose preconditions
    /// fail, which are simply no-ops here — the precondition checks
    /// themselves are covered by `LayoutError` unit tests elsewhere) and
    /// asserts the tiling invariant after every single op, not just at the
    /// end: a bug that only breaks tiling transiently would survive a
    /// final-state-only check.
    #[test]
    fn layout_ops_never_produce_overlapping_or_gapped_rects(ops in proptest::collection::vec(op_strategy(), 0..40)) {
        let pool: Vec<PaneId> = (0..POOL_SIZE).map(|_| PaneId::new_v4()).collect();
        let mut layout = Layout::with_root(pool[0]);

        for op in ops {
            match op {
                Op::Split { target, new, dir, ratio } => {
                    let _ = layout.split(pool[target], dir, pool[new], ratio);
                }
                Op::Close { target } => {
                    let _ = layout.close(pool[target]);
                }
                Op::Swap { a, b } => {
                    let _ = layout.swap(pool[a], pool[b]);
                }
                Op::Resize { target, delta } => {
                    let _ = layout.resize(pool[target], delta);
                }
                Op::Zoom { target } => {
                    let _ = layout.zoom(pool[target]);
                }
                Op::Unzoom => layout.unzoom(),
            }
            // An empty layout (every pane closed) has no rects to check —
            // vacuously fine, nothing left to overlap or gap.
            if !layout.is_empty() {
                assert_tiles_exactly(&layout.rects(AREA), AREA);
            }
        }
    }
}
